//! Strict JSON schemas for providers' strict tool and output modes (TS `models/strict-schema.ts`).
#![cfg_attr(not(test), expect(dead_code, reason = "used by the model providers"))]

use std::collections::{BTreeSet, HashSet};

use crate::logging::warn_once;
use crate::types::json::JsonValue;

type SchemaNode = serde_json::Map<String, JsonValue>;

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

/// The JSON pointer segments of the subschemas of `schema`, in TS `childSchemas` order.
fn child_paths(schema: &SchemaNode) -> Vec<Vec<String>> {
    let mut paths = Vec::new();
    for keyword in SCHEMA_MAP_KEYWORDS {
        if let Some(JsonValue::Object(map)) = schema.get(keyword) {
            for (key, value) in map {
                if value.is_object() {
                    paths.push(vec![keyword.to_owned(), key.clone()]);
                }
            }
        }
    }
    for keyword in SCHEMA_LIST_KEYWORDS {
        if let Some(JsonValue::Array(list)) = schema.get(keyword) {
            for (index, value) in list.iter().enumerate() {
                if value.is_object() {
                    paths.push(vec![keyword.to_owned(), index.to_string()]);
                }
            }
        }
    }
    for keyword in SCHEMA_VALUE_KEYWORDS {
        if schema.get(keyword).is_some_and(JsonValue::is_object) {
            paths.push(vec![keyword.to_owned()]);
        }
    }
    paths
}

/// The subschemas of `schema` (TS `childSchemas`).
pub(crate) fn child_schemas(schema: &SchemaNode) -> Vec<&SchemaNode> {
    let maps = SCHEMA_MAP_KEYWORDS
        .iter()
        .filter_map(|keyword| schema.get(*keyword).and_then(JsonValue::as_object))
        .flat_map(|map| map.values());
    let lists = SCHEMA_LIST_KEYWORDS
        .iter()
        .filter_map(|keyword| schema.get(*keyword).and_then(JsonValue::as_array))
        .flatten();
    let values = SCHEMA_VALUE_KEYWORDS
        .iter()
        .filter_map(|keyword| schema.get(*keyword));
    maps.chain(lists)
        .chain(values)
        .filter_map(JsonValue::as_object)
        .collect()
}

fn pointer(path: &[String]) -> String {
    path.iter()
        .map(|segment| format!("/{}", segment.replace('~', "~0").replace('/', "~1")))
        .collect()
}

fn is_object_type(schema: &SchemaNode) -> bool {
    match schema.get("type") {
        Some(JsonValue::String(kind)) => kind == "object",
        Some(JsonValue::Array(kinds)) => kinds.iter().any(|kind| kind == "object"),
        _ => false,
    }
}

/// A copy of `schema` with `additionalProperties: false` on every object node that sets neither
/// it nor `$ref`, and every `$ref` that has sibling keys inlined (its own keys winning). A
/// recursive `$ref` is left unresolved at the cycle point (TS `ensureStrictJsonSchema`).
pub(crate) fn ensure_strict_json_schema(schema: &JsonValue) -> JsonValue {
    let mut root = schema.clone();
    apply_strict(&mut root, &mut Vec::new(), &HashSet::new());
    root
}

/// Works on the node at `path` inside `root`, as TS mutates its copy in place and resolves refs
/// against the partly processed root.
fn apply_strict(root: &mut JsonValue, path: &mut Vec<String>, inlining: &HashSet<String>) {
    let at = pointer(path);
    let Some(JsonValue::Object(schema)) = root.pointer_mut(&at) else {
        return;
    };
    if is_object_type(schema) && !schema.contains_key("additionalProperties") && !schema.contains_key("$ref")
    {
        schema.insert("additionalProperties".into(), JsonValue::Bool(false));
    }
    for child in child_paths(schema) {
        let depth = path.len();
        path.extend(child);
        apply_strict(root, path, inlining);
        path.truncate(depth);
    }
    let Some(JsonValue::Object(schema)) = root.pointer(&at) else {
        return;
    };
    let Some(reference) = schema.get("$ref").and_then(JsonValue::as_str).map(str::to_owned) else {
        return;
    };
    if schema.len() <= 1 {
        return;
    }
    if inlining.contains(&reference) {
        warn_once(&format!(
            "ref=<{reference}> | recursive $ref cannot be inlined, leaving it unresolved"
        ));
        return;
    }
    let Some(mut merged) = resolve_ref(root, &reference).cloned() else {
        return;
    };
    for (key, value) in schema {
        merged.insert(key.clone(), value.clone());
    }
    merged.shift_remove("$ref");
    if let Some(node) = root.pointer_mut(&at) {
        *node = JsonValue::Object(merged);
    }
    let mut inlining = inlining.clone();
    inlining.insert(reference);
    apply_strict(root, path, &inlining);
}

/// The node `reference` (`#/a/b`) names in `root`, split on `/` without unescaping, as TS does.
fn resolve_ref<'a>(root: &'a JsonValue, reference: &str) -> Option<&'a SchemaNode> {
    let Some(path) = reference.strip_prefix("#/") else {
        tracing::warn!("ref=<{reference}> | unexpected $ref format, skipping resolution");
        return None;
    };
    let mut current = root;
    for key in path.split('/') {
        let Some(next) = current.as_object().and_then(|node| node.get(key)) else {
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

/// The keywords strict modes reject that `schema` uses anywhere, sorted (TS
/// `findUnsupportedStrictKeywords`).
pub(crate) fn find_unsupported_strict_keywords(schema: &JsonValue) -> Vec<String> {
    let mut found = BTreeSet::new();
    if let Some(schema) = schema.as_object() {
        collect_unsupported_keywords(schema, &mut found);
    }
    found.into_iter().map(str::to_owned).collect()
}

fn collect_unsupported_keywords(schema: &SchemaNode, found: &mut BTreeSet<&'static str>) {
    for keyword in UNSUPPORTED_STRICT_KEYWORDS {
        if schema.contains_key(keyword) {
            found.insert(keyword);
        }
    }
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
