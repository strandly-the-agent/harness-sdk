//! Strict JSON schemas for providers that enforce them (TS `models/strict-schema.ts`, internal).
#![cfg_attr(
    not(test),
    expect(dead_code, reason = "used by the Bedrock provider and native output")
)]

use std::collections::BTreeSet;

use serde_json::Map;

use crate::logging::warn_once;
use crate::types::json::{JsonValue, js_key_order};

const SCHEMA_MAP_KEYWORDS: [&str; 3] = ["$defs", "definitions", "properties"];
const SCHEMA_LIST_KEYWORDS: [&str; 3] = ["anyOf", "allOf", "oneOf"];
const SCHEMA_VALUE_KEYWORDS: [&str; 2] = ["items", "additionalProperties"];
const UNSUPPORTED_STRICT_KEYWORDS: [&str; 7] = [
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "multipleOf",
    "minLength",
    "maxLength",
];

/// The subschemas of `schema` (TS `childSchemas`): the values of `$defs`, `definitions` and
/// `properties`, the entries of `anyOf`, `allOf` and `oneOf`, and `items` and
/// `additionalProperties` when they are schemas.
pub(crate) fn child_schemas(schema: &Map<String, JsonValue>) -> Vec<&Map<String, JsonValue>> {
    let maps = SCHEMA_MAP_KEYWORDS
        .iter()
        .filter_map(|keyword| schema.get(*keyword)?.as_object())
        .flat_map(Map::values);
    let lists = SCHEMA_LIST_KEYWORDS
        .iter()
        .filter_map(|keyword| schema.get(*keyword)?.as_array())
        .flatten();
    let values = SCHEMA_VALUE_KEYWORDS
        .iter()
        .filter_map(|keyword| schema.get(*keyword));
    maps.chain(lists)
        .chain(values)
        .filter_map(JsonValue::as_object)
        .collect()
}

/// A step from a schema to one of its subschemas.
enum Step {
    Key(String),
    Index(usize),
}

/// The steps from `schema` to each of its subschemas, in TS `childSchemas` order.
fn child_steps(schema: &Map<String, JsonValue>) -> Vec<Vec<Step>> {
    let key = |keyword: &str| Step::Key(keyword.to_owned());
    let mut steps = Vec::new();
    for keyword in SCHEMA_MAP_KEYWORDS {
        if let Some(JsonValue::Object(map)) = schema.get(keyword) {
            let children = js_key_order(map.keys().map(String::as_str));
            steps.extend(
                children
                    .into_iter()
                    .filter(|child| map.get(*child).is_some_and(JsonValue::is_object))
                    .map(|child| vec![key(keyword), key(child)]),
            );
        }
    }
    for keyword in SCHEMA_LIST_KEYWORDS {
        if let Some(JsonValue::Array(list)) = schema.get(keyword) {
            steps.extend(
                (0..list.len())
                    .filter(|index| list[*index].is_object())
                    .map(|index| vec![key(keyword), Step::Index(index)]),
            );
        }
    }
    steps.extend(
        SCHEMA_VALUE_KEYWORDS
            .into_iter()
            .filter(|keyword| schema.get(*keyword).is_some_and(JsonValue::is_object))
            .map(|keyword| vec![key(keyword)]),
    );
    steps
}

fn node_mut<'a>(root: &'a mut JsonValue, path: &[Step]) -> Option<&'a mut Map<String, JsonValue>> {
    let mut current = root;
    for step in path {
        current = match step {
            Step::Key(key) => current.as_object_mut()?.get_mut(key)?,
            Step::Index(index) => current.as_array_mut()?.get_mut(*index)?,
        };
    }
    current.as_object_mut()
}

fn is_object_type(schema: &Map<String, JsonValue>) -> bool {
    match schema.get("type") {
        Some(JsonValue::String(kind)) => kind == "object",
        Some(JsonValue::Array(kinds)) => kinds.iter().any(|kind| kind == "object"),
        _ => false,
    }
}

/// A copy of `schema` with every object closed (`additionalProperties: false` unless set) and
/// every `$ref` that has sibling keys inlined, siblings winning (TS `ensureStrictJsonSchema`). A
/// recursive `$ref` is left unresolved with a one-time warning.
pub(crate) fn ensure_strict_json_schema(schema: &JsonValue) -> JsonValue {
    let mut copy = schema.clone();
    if copy.is_object() {
        apply_strict(&mut copy, &mut Vec::new(), &[]);
    }
    copy
}

/// Makes the subschema at `path` strict. Refs resolve against `root` as rewritten so far, so a
/// target visited earlier is inlined in its strict form, as in TS.
fn apply_strict(root: &mut JsonValue, path: &mut Vec<Step>, inlining: &[String]) {
    let Some(schema) = node_mut(root, path) else {
        return;
    };
    if is_object_type(schema) && !schema.contains_key("additionalProperties") && !schema.contains_key("$ref")
    {
        schema.insert("additionalProperties".to_owned(), JsonValue::Bool(false));
    }
    for steps in child_steps(schema) {
        let depth = path.len();
        path.extend(steps);
        apply_strict(root, path, inlining);
        path.truncate(depth);
    }
    let Some(schema) = node_mut(root, path) else {
        return;
    };
    let Some(JsonValue::String(reference)) = schema.get("$ref") else {
        return;
    };
    if schema.len() <= 1 {
        return;
    }
    let reference = reference.clone();
    if inlining.contains(&reference) {
        warn_once(&format!(
            "ref=<{reference}> | recursive $ref cannot be inlined, leaving it unresolved"
        ));
        return;
    }
    let Some(mut merged) = resolve_ref(root, &reference).cloned() else {
        return;
    };
    let Some(schema) = node_mut(root, path) else {
        return;
    };
    for (key, value) in std::mem::take(schema) {
        merged.insert(key, value);
    }
    merged.shift_remove("$ref");
    *schema = merged;
    let mut inlining = inlining.to_vec();
    inlining.push(reference);
    apply_strict(root, path, &inlining);
}

fn resolve_ref<'a>(root: &'a JsonValue, reference: &str) -> Option<&'a Map<String, JsonValue>> {
    let Some(path) = reference.strip_prefix("#/") else {
        tracing::warn!("ref=<{reference}> | unexpected $ref format, skipping resolution");
        return None;
    };
    let mut current = root;
    for key in path.split('/') {
        let Some(next) = current.as_object().and_then(|map| map.get(key)) else {
            tracing::warn!("ref=<{reference}> | failed to resolve $ref path");
            return None;
        };
        current = next;
    }
    let resolved = current.as_object();
    if resolved.is_none() {
        tracing::warn!("ref=<{reference}> | resolved to non-dict value");
    }
    resolved
}

/// The keywords in `schema` that strict mode rejects: numeric and length bounds, and an
/// `additionalProperties` other than `false`; sorted and deduplicated (TS
/// `findUnsupportedStrictKeywords`).
pub(crate) fn find_unsupported_strict_keywords(schema: &JsonValue) -> Vec<String> {
    let mut found = BTreeSet::new();
    if let JsonValue::Object(map) = schema {
        collect_unsupported_keywords(map, &mut found);
    }
    found.into_iter().map(str::to_owned).collect()
}

fn collect_unsupported_keywords(schema: &Map<String, JsonValue>, found: &mut BTreeSet<&'static str>) {
    found.extend(
        UNSUPPORTED_STRICT_KEYWORDS
            .iter()
            .filter(|keyword| schema.contains_key(**keyword)),
    );
    if schema
        .get("additionalProperties")
        .is_some_and(|value| value != &JsonValue::Bool(false))
    {
        found.insert("additionalProperties");
    }
    for child in child_schemas(schema) {
        collect_unsupported_keywords(child, found);
    }
}

#[cfg(test)]
mod tests;
