//! Strict JSON schemas for providers that require closed objects (TS `models/strict-schema.ts`).

use std::collections::{BTreeSet, HashSet};

use crate::logging::warn_once::warn_once;
use crate::types::json::{JsonObject, JsonValue};

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

/// One step from a schema node to a child schema.
#[derive(Debug, Clone)]
enum Step {
    Key(String),
    Index(usize),
}

/// Paths (relative to `schema`) of the child schemas, in TS `childSchemas` order.
fn child_paths(schema: &JsonObject) -> Vec<[Step; 2]> {
    let mut paths = Vec::new();
    for keyword in SCHEMA_MAP_KEYWORDS {
        if let Some(JsonValue::Object(map)) = schema.get(keyword) {
            paths.extend(
                map.iter()
                    .filter(|(_, child)| child.is_object())
                    .map(|(name, _)| [Step::Key(keyword.to_owned()), Step::Key(name.clone())]),
            );
        }
    }
    for keyword in SCHEMA_LIST_KEYWORDS {
        if let Some(JsonValue::Array(list)) = schema.get(keyword) {
            paths.extend(
                list.iter()
                    .enumerate()
                    .filter(|(_, child)| child.is_object())
                    .map(|(index, _)| [Step::Key(keyword.to_owned()), Step::Index(index)]),
            );
        }
    }
    paths
}

/// The child schemas of `schema` (TS `childSchemas`).
pub(crate) fn child_schemas(schema: &JsonObject) -> Vec<&JsonObject> {
    let mut children = Vec::new();
    for keyword in SCHEMA_MAP_KEYWORDS {
        if let Some(JsonValue::Object(map)) = schema.get(keyword) {
            children.extend(map.values().filter_map(JsonValue::as_object));
        }
    }
    for keyword in SCHEMA_LIST_KEYWORDS {
        if let Some(JsonValue::Array(list)) = schema.get(keyword) {
            children.extend(list.iter().filter_map(JsonValue::as_object));
        }
    }
    children.extend(
        SCHEMA_VALUE_KEYWORDS
            .iter()
            .filter_map(|keyword| schema.get(*keyword).and_then(JsonValue::as_object)),
    );
    children
}

fn node_mut<'a>(root: &'a mut JsonValue, path: &[Step]) -> Option<&'a mut JsonObject> {
    let mut node = root;
    for step in path {
        node = match step {
            Step::Key(key) => node.as_object_mut()?.get_mut(key)?,
            Step::Index(index) => node.as_array_mut()?.get_mut(*index)?,
        };
    }
    node.as_object_mut()
}

fn is_object_type(schema: &JsonObject) -> bool {
    match schema.get("type") {
        Some(JsonValue::String(kind)) => kind == "object",
        Some(JsonValue::Array(kinds)) => kinds.iter().any(|kind| kind == "object"),
        _ => false,
    }
}

/// A copy of `schema` with `additionalProperties: false` on every object node that doesn't set it,
/// and every `$ref` with sibling keys inlined (TS `ensureStrictJsonSchema`).
#[allow(dead_code, reason = "used by model providers")]
pub(crate) fn ensure_strict_json_schema(schema: &JsonValue) -> JsonValue {
    let mut root = schema.clone();
    apply_strict(&mut root, &mut Vec::new(), &HashSet::new());
    root
}

fn all_child_paths(schema: &JsonObject) -> Vec<Vec<Step>> {
    let mut paths: Vec<Vec<Step>> = child_paths(schema).into_iter().map(Vec::from).collect();
    paths.extend(
        SCHEMA_VALUE_KEYWORDS
            .iter()
            .filter(|keyword| schema.get(**keyword).is_some_and(JsonValue::is_object))
            .map(|keyword| vec![Step::Key((*keyword).to_owned())]),
    );
    paths
}

fn apply_strict(root: &mut JsonValue, path: &mut Vec<Step>, inlining: &HashSet<String>) {
    let Some(schema) = node_mut(root, path) else {
        return;
    };
    if is_object_type(schema) && !schema.contains_key("additionalProperties") && !schema.contains_key("$ref")
    {
        schema.insert("additionalProperties".to_owned(), JsonValue::Bool(false));
    }
    let depth = path.len();
    for child in all_child_paths(schema) {
        path.extend(child);
        apply_strict(root, path, inlining);
        path.truncate(depth);
    }
    inline_ref(root, path, inlining);
}

/// Inlines a `$ref` that has sibling keys; existing keys win over the resolved schema.
fn inline_ref(root: &mut JsonValue, path: &mut Vec<Step>, inlining: &HashSet<String>) {
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
    let Some(mut merged) = resolve_ref(root, &reference) else {
        return;
    };
    let Some(schema) = node_mut(root, path) else {
        return;
    };
    for (key, value) in schema.iter() {
        merged.insert(key.clone(), value.clone());
    }
    merged.shift_remove("$ref");
    *schema = merged;
    let mut inlining = inlining.clone();
    inlining.insert(reference);
    apply_strict(root, path, &inlining);
}

fn resolve_ref(root: &JsonValue, reference: &str) -> Option<JsonObject> {
    let Some(pointer) = reference.strip_prefix("#/") else {
        tracing::warn!("ref=<{reference}> | unexpected $ref format, skipping resolution");
        return None;
    };
    let mut current = root;
    for key in pointer.split('/') {
        let Some(next) = current.as_object().and_then(|map| map.get(key)) else {
            tracing::warn!("ref=<{reference}> | failed to resolve $ref path");
            return None;
        };
        current = next;
    }
    let resolved = current.as_object().cloned();
    if resolved.is_none() {
        tracing::warn!("ref=<{reference}> | resolved to non-dict value");
    }
    resolved
}

/// The keywords strict tool use rejects anywhere in `schema`, sorted and deduplicated
/// (TS `findUnsupportedStrictKeywords`).
#[allow(dead_code, reason = "used by model providers")]
pub(crate) fn find_unsupported_strict_keywords(schema: &JsonValue) -> Vec<String> {
    let mut found = BTreeSet::new();
    if let Some(schema) = schema.as_object() {
        collect_unsupported_keywords(schema, &mut found);
    }
    found.into_iter().map(str::to_owned).collect()
}

fn collect_unsupported_keywords(schema: &JsonObject, found: &mut BTreeSet<&'static str>) {
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
