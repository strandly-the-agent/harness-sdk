//! JSON schemas for typed tools (the role of TS `tools/zod-utils.ts`).
//!
//! schemars output is normalized toward the shape Zod's `z.toJSONSchema` produces: subschemas
//! are inlined unless recursive, `$schema` and `title` are dropped, non-standard formats
//! (`uint32`, `double`, ...) are dropped, optional properties lose their `null` branch, and
//! objects with properties default to `additionalProperties: false`. Key order is kept.

use schemars::JsonSchema;

use crate::types::json::{JsonObject, JsonValue};

/// The `format` values JSON Schema defines; schemars' Rust-specific ones are dropped.
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

/// Keywords whose value is a single subschema.
const SUBSCHEMA_KEYS: &[&str] = &[
    "items",
    "additionalProperties",
    "not",
    "if",
    "then",
    "else",
    "contains",
    "propertyNames",
    "unevaluatedProperties",
    "unevaluatedItems",
];

/// Keywords whose value is a list of subschemas.
const SUBSCHEMA_LIST_KEYS: &[&str] = &["anyOf", "oneOf", "allOf", "prefixItems"];

/// The normalized input schema for `T`.
pub(crate) fn input_schema_for<T: JsonSchema>() -> JsonValue {
    let schema = schemars::SchemaGenerator::default()
        .into_root_schema_for::<T>()
        .to_value();
    normalize_schema(schema)
}

/// Normalizes a schemars root schema (see the module docs).
pub(crate) fn normalize_schema(schema: JsonValue) -> JsonValue {
    let JsonValue::Object(mut root) = schema else {
        return schema;
    };
    let defs = match root.shift_remove("$defs") {
        Some(JsonValue::Object(defs)) => defs,
        _ => JsonObject::new(),
    };
    let mut inliner = Inliner {
        defs,
        stack: Vec::new(),
        kept: JsonObject::new(),
    };
    root.shift_remove("$schema");
    let mut normalized = inliner.object(root);
    if !inliner.kept.is_empty() {
        normalized.insert("$defs".to_owned(), JsonValue::Object(inliner.kept));
    }
    JsonValue::Object(normalized)
}

struct Inliner {
    defs: JsonObject,
    stack: Vec<String>,
    kept: JsonObject,
}

impl Inliner {
    fn value(&mut self, schema: JsonValue) -> JsonValue {
        match schema {
            JsonValue::Object(object) => self.resolve(object),
            other => other,
        }
    }

    fn resolve(&mut self, object: JsonObject) -> JsonValue {
        let Some(name) = def_name(&object) else {
            return JsonValue::Object(self.object(object));
        };
        let Some(def) = self.defs.get(&name).cloned() else {
            return JsonValue::Object(object);
        };
        if self.stack.contains(&name) {
            self.keep(&name, def);
            return JsonValue::Object(object);
        }
        self.stack.push(name);
        let mut inlined = match def {
            JsonValue::Object(def) => self.object(def),
            other => return other,
        };
        self.stack.pop();
        for (key, value) in object {
            if key != "$ref" {
                let value = self.value(value);
                inlined.insert(key, value);
            }
        }
        JsonValue::Object(inlined)
    }

    fn keep(&mut self, name: &str, def: JsonValue) {
        if self.kept.contains_key(name) {
            return;
        }
        self.kept.insert(name.to_owned(), JsonValue::Null);
        let normalized = self.value(def);
        self.kept.insert(name.to_owned(), normalized);
    }

    fn object(&mut self, object: JsonObject) -> JsonObject {
        let mut out = JsonObject::new();
        for (key, value) in object {
            match key.as_str() {
                "title" | "$schema" | "$defs" | "definitions" => {}
                "format" if !is_standard_format(&value) => {}
                "properties" => {
                    out.insert(key, self.properties(value));
                }
                k if SUBSCHEMA_KEYS.contains(&k) => {
                    let value = self.value(value);
                    out.insert(key, value);
                }
                k if SUBSCHEMA_LIST_KEYS.contains(&k) => {
                    out.insert(key, self.list(value));
                }
                _ => {
                    out.insert(key, value);
                }
            }
        }
        if out.contains_key("properties") && !out.contains_key("additionalProperties") {
            out.insert("additionalProperties".to_owned(), JsonValue::Bool(false));
        }
        strip_optional_nulls(&mut out);
        out
    }

    fn properties(&mut self, properties: JsonValue) -> JsonValue {
        let JsonValue::Object(properties) = properties else {
            return properties;
        };
        JsonValue::Object(
            properties
                .into_iter()
                .map(|(name, schema)| (name, self.value(schema)))
                .collect(),
        )
    }

    fn list(&mut self, list: JsonValue) -> JsonValue {
        let JsonValue::Array(items) = list else {
            return list;
        };
        JsonValue::Array(items.into_iter().map(|item| self.value(item)).collect())
    }
}

fn def_name(object: &JsonObject) -> Option<String> {
    let reference = object.get("$ref")?.as_str()?;
    reference
        .strip_prefix("#/$defs/")
        .or_else(|| reference.strip_prefix("#/definitions/"))
        .map(str::to_owned)
}

fn is_standard_format(value: &JsonValue) -> bool {
    value
        .as_str()
        .is_some_and(|format| STANDARD_FORMATS.contains(&format))
}

/// Optional properties (absent from `required`) lose their `null` branch, matching Zod
/// `.optional()`.
fn strip_optional_nulls(object: &mut JsonObject) {
    let required: Vec<String> = object
        .get("required")
        .and_then(JsonValue::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(|n| n.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let Some(JsonValue::Object(properties)) = object.get_mut("properties") else {
        return;
    };
    for (name, schema) in properties.iter_mut() {
        if required.contains(name) {
            continue;
        }
        if let JsonValue::Object(schema) = schema {
            strip_null(schema);
        }
    }
}

fn strip_null(schema: &mut JsonObject) {
    if let Some(JsonValue::Array(types)) = schema.get_mut("type") {
        types.retain(|t| t.as_str() != Some("null"));
        if let [single] = types.as_slice() {
            let single = single.clone();
            schema.insert("type".to_owned(), single);
        }
    }
    if let Some(JsonValue::Array(values)) = schema.get_mut("enum") {
        values.retain(|v| !v.is_null());
    }
    let Some(JsonValue::Array(branches)) = schema.get("anyOf") else {
        return;
    };
    let non_null: Vec<JsonValue> = branches
        .iter()
        .filter(|branch| branch.get("type").and_then(JsonValue::as_str) != Some("null"))
        .cloned()
        .collect();
    if non_null.len() == branches.len() {
        return;
    }
    schema.shift_remove("anyOf");
    match <[JsonValue; 1]>::try_from(non_null) {
        Ok([JsonValue::Object(only)]) => {
            for (key, value) in only {
                schema.entry(key).or_insert(value);
            }
        }
        Ok([other]) => {
            schema.insert("anyOf".to_owned(), JsonValue::Array(vec![other]));
        }
        Err(rest) => {
            schema.insert("anyOf".to_owned(), JsonValue::Array(rest));
        }
    }
}

#[cfg(test)]
mod tests;
