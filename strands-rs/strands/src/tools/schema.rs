//! Tool input schemas from Rust types (TS `tools/zod-utils.ts`).
//!
//! TS derives schemas with Zod's `z.toJSONSchema` and strips `$schema`. Rust derives them with
//! `schemars` and normalizes the output to Zod's shape: subschemas inlined unless recursive, no
//! `$schema` or `title`, only standard formats, `Option<T>` as an optional non-null property,
//! objects closed with `additionalProperties: false`, unions of plain types as a `type` list, and
//! `description` last. Property order is kept.

use schemars::JsonSchema;
use schemars::generate::SchemaSettings;
use serde_json::Map;

use crate::types::json::JsonValue;

/// The formats JSON Schema 2020-12 defines; `schemars` also writes Rust ones (`uint32`, `double`).
const STANDARD_FORMATS: &[&str] = &[
    "date-time",
    "date",
    "time",
    "duration",
    "email",
    "idn-email",
    "hostname",
    "idn-hostname",
    "ipv4",
    "ipv6",
    "uri",
    "uri-reference",
    "iri",
    "iri-reference",
    "uuid",
    "uri-template",
    "json-pointer",
    "relative-json-pointer",
    "regex",
];

/// Keywords whose value is one subschema.
const SCHEMA_KEYWORDS: &[&str] = &[
    "items",
    "additionalItems",
    "additionalProperties",
    "unevaluatedProperties",
    "unevaluatedItems",
    "propertyNames",
    "contains",
    "not",
    "if",
    "then",
    "else",
];

/// Keywords whose value is an array of subschemas.
const SCHEMA_LIST_KEYWORDS: &[&str] = &["prefixItems", "allOf", "anyOf", "oneOf"];

/// Keywords whose value maps names to subschemas.
const SCHEMA_MAP_KEYWORDS: &[&str] = &[
    "properties",
    "patternProperties",
    "$defs",
    "definitions",
    "dependentSchemas",
];

/// The input schema a tool built from `I` advertises.
pub(crate) fn tool_input_schema<I: JsonSchema>() -> JsonValue {
    let mut settings = SchemaSettings::draft2020_12();
    settings.inline_subschemas = true;
    normalize(settings.into_generator().into_root_schema_for::<I>().to_value())
}

/// The schema a tool built from `I` validates input against: the `schemars` output as is, which
/// follows serde (unknown fields and `null` for `Option` are accepted).
pub(crate) fn validation_schema<I: JsonSchema>() -> JsonValue {
    SchemaSettings::draft2020_12()
        .into_generator()
        .into_root_schema_for::<I>()
        .to_value()
}

/// Rewrites `schemars` output into Zod's shape.
pub(crate) fn normalize(mut schema: JsonValue) -> JsonValue {
    if let Some(root) = schema.as_object_mut() {
        root.shift_remove("$schema");
    }
    normalize_node(&mut schema);
    schema
}

fn normalize_node(node: &mut JsonValue) {
    let Some(map) = node.as_object_mut() else {
        return;
    };
    map.shift_remove("title");
    if map
        .get("format")
        .and_then(JsonValue::as_str)
        .is_some_and(|format| !STANDARD_FORMATS.contains(&format))
    {
        map.shift_remove("format");
    }
    make_optional_properties_non_null(map);
    for_each_subschema(map, normalize_node);
    collapse_type_union(map);
    close_object(map);
    move_to_end(map, "description");
}

fn for_each_subschema(map: &mut Map<String, JsonValue>, mut visit: impl FnMut(&mut JsonValue)) {
    for (key, value) in map.iter_mut() {
        let key = key.as_str();
        if SCHEMA_KEYWORDS.contains(&key) {
            visit(value);
        } else if SCHEMA_LIST_KEYWORDS.contains(&key) {
            value.as_array_mut().into_iter().flatten().for_each(&mut visit);
        } else if SCHEMA_MAP_KEYWORDS.contains(&key) {
            value
                .as_object_mut()
                .into_iter()
                .flat_map(Map::values_mut)
                .for_each(&mut visit);
        }
    }
}

/// `Option<T>` fields are optional; Zod's `.optional()` doesn't also allow `null`.
fn make_optional_properties_non_null(map: &mut Map<String, JsonValue>) {
    let required: Vec<String> = map
        .get("required")
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten()
        .filter_map(|name| name.as_str().map(str::to_owned))
        .collect();
    let Some(properties) = map.get_mut("properties").and_then(JsonValue::as_object_mut) else {
        return;
    };
    for (name, property) in properties.iter_mut() {
        if !required.contains(name)
            && let Some(property) = property.as_object_mut()
        {
            remove_null(property);
        }
    }
}

fn remove_null(schema: &mut Map<String, JsonValue>) {
    if let Some(JsonValue::Array(types)) = schema.get_mut("type") {
        types.retain(|ty| ty != "null");
        if let [single] = types.as_slice() {
            let single = single.clone();
            schema.insert("type".to_owned(), single);
        }
    }
    if let Some(JsonValue::Array(values)) = schema.get_mut("enum") {
        values.retain(|value| !value.is_null());
    }
    if schema.get("default").is_some_and(JsonValue::is_null) {
        schema.shift_remove("default");
    }
    for keyword in ["anyOf", "oneOf"] {
        unwrap_null_variant(schema, keyword);
    }
}

/// Replaces `anyOf: [T, {"type": "null"}]` with `T`.
fn unwrap_null_variant(schema: &mut Map<String, JsonValue>, keyword: &str) {
    let Some(JsonValue::Array(variants)) = schema.get_mut(keyword) else {
        return;
    };
    variants.retain(|variant| variant != &serde_json::json!({ "type": "null" }));
    let [JsonValue::Object(_)] = variants.as_slice() else {
        return;
    };
    let Some(JsonValue::Array(mut variants)) = schema.shift_remove(keyword) else {
        return;
    };
    let Some(JsonValue::Object(variant)) = variants.pop() else {
        return;
    };
    for (key, value) in variant {
        schema.entry(key).or_insert(value);
    }
}

/// Zod writes a union of plain types as `type: [..]` (`z.union([z.string(), z.number()])`).
fn collapse_type_union(map: &mut Map<String, JsonValue>) {
    let Some(JsonValue::Array(variants)) = map.get("anyOf") else {
        return;
    };
    let types: Option<Vec<JsonValue>> = variants
        .iter()
        .map(|variant| match variant.as_object() {
            Some(variant) if variant.len() == 1 => variant.get("type").filter(|ty| ty.is_string()).cloned(),
            _ => None,
        })
        .collect();
    let Some(types) = types.filter(|types| types.len() > 1 && !map.contains_key("type")) else {
        return;
    };
    map.shift_remove("anyOf");
    map.insert("type".to_owned(), JsonValue::Array(types));
}

/// Zod's `z.object` lists its properties and rejects others; maps keep their
/// `additionalProperties` schema.
fn close_object(map: &mut Map<String, JsonValue>) {
    if map.get("type").and_then(JsonValue::as_str) != Some("object")
        || map.contains_key("additionalProperties")
    {
        return;
    }
    if !map.contains_key("properties") {
        map.insert("properties".to_owned(), JsonValue::Object(Map::new()));
    }
    map.insert("additionalProperties".to_owned(), JsonValue::Bool(false));
}

fn move_to_end(map: &mut Map<String, JsonValue>, key: &str) {
    if let Some(value) = map.shift_remove(key) {
        map.insert(key.to_owned(), value);
    }
}

#[cfg(test)]
mod tests;
