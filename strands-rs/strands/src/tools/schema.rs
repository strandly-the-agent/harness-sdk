//! JSON schemas for typed tool input: schemars output normalized to the shape Zod's
//! `z.toJSONSchema` produces for the equivalent TS schema.

use schemars::JsonSchema;
use schemars::generate::SchemaSettings;
use serde_json::{Map, Value};

use crate::types::JsonValue;

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
    "uri-template",
    "uuid",
    "json-pointer",
    "relative-json-pointer",
    "regex",
];

const SCHEMA_ARRAYS: &[&str] = &["anyOf", "oneOf", "allOf", "prefixItems"];
const SCHEMA_VALUES: &[&str] = &[
    "items",
    "additionalProperties",
    "not",
    "if",
    "then",
    "else",
    "contains",
    "propertyNames",
];

/// The normalized JSON schema of `T`.
///
/// Subschemas are inlined unless the type is recursive; `$schema`, `title` and formats JSON
/// Schema doesn't define (`uint32`, `double`, ...) are dropped; an `Option` property is optional
/// rather than nullable; objects get `"additionalProperties": false`. Property order follows
/// the type's field order.
pub fn schema_for<T: JsonSchema>() -> JsonValue {
    let generator = SchemaSettings::draft2020_12()
        .with(|settings| {
            settings.inline_subschemas = true;
            settings.meta_schema = None;
        })
        .into_generator();
    let mut schema = generator.into_root_schema_for::<T>().to_value();
    normalize(&mut schema);
    schema
}

/// Normalizes a schema in place (see [`schema_for`]).
pub(crate) fn normalize(schema: &mut Value) {
    let Value::Object(map) = schema else {
        return;
    };
    map.remove("$schema");
    map.remove("title");
    if map
        .get("format")
        .and_then(Value::as_str)
        .is_some_and(|format| !STANDARD_FORMATS.contains(&format))
    {
        map.remove("format");
    }
    for key in SCHEMA_VALUES {
        if let Some(value) = map.get_mut(*key) {
            normalize(value);
        }
    }
    for key in SCHEMA_ARRAYS {
        if let Some(Value::Array(items)) = map.get_mut(*key) {
            items.iter_mut().for_each(normalize);
        }
    }
    if let Some(Value::Object(defs)) = map.get_mut("$defs") {
        defs.values_mut().for_each(normalize);
    }
    normalize_object(map);
}

fn normalize_object(map: &mut Map<String, Value>) {
    let required: Vec<String> = map
        .get("required")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let Some(Value::Object(properties)) = map.get_mut("properties") else {
        return;
    };
    for (name, property) in properties.iter_mut() {
        normalize(property);
        if !required.contains(name) {
            strip_null(property);
        }
    }
    map.entry("additionalProperties").or_insert(Value::Bool(false));
}

fn strip_null(schema: &mut Value) {
    let Value::Object(map) = schema else {
        return;
    };
    if let Some(Value::Array(types)) = map.get_mut("type") {
        types.retain(|t| t != "null");
        if let [single] = types.as_slice() {
            let single = single.clone();
            map.insert("type".into(), single);
        }
    }
    let Some(Value::Array(variants)) = map.get_mut("anyOf") else {
        return;
    };
    variants.retain(|variant| variant.get("type").and_then(Value::as_str) != Some("null"));
    if variants.len() != 1 {
        return;
    }
    if let Some(Value::Object(variant)) = variants.pop() {
        map.remove("anyOf");
        for (key, value) in variant {
            map.entry(key).or_insert(value);
        }
    }
}

#[cfg(test)]
mod tests;
