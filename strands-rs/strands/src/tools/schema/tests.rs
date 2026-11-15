//! Schema normalization checked against the Zod output TS sends
//! (`conformance/fixtures/schemas/tool_input_schemas.json`, `tool_specs.json`), with Rust types
//! equivalent to the Zod schemas in `conformance/src/schemas.ts`. Intended differences are
//! asserted explicitly and listed in `docs/DIVERGENCES.md`.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::*;

fn fixture(name: &str) -> JsonValue {
    let fixtures: JsonValue = serde_json::from_str(include_str!(
        "../../../../conformance/fixtures/schemas/tool_input_schemas.json"
    ))
    .unwrap();
    fixtures[name].clone()
}

fn without(mut schema: JsonValue, pointer: &str, key: &str) -> JsonValue {
    schema
        .pointer_mut(pointer)
        .unwrap()
        .as_object_mut()
        .unwrap()
        .shift_remove(key);
    schema
}

#[derive(Deserialize, JsonSchema)]
#[expect(dead_code, reason = "schema only")]
struct StringWithDescription {
    /// City name
    city: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Unit {
    Celsius,
    Fahrenheit,
}

#[derive(Deserialize, JsonSchema)]
#[expect(dead_code, reason = "schema only")]
struct Enums {
    /// Temperature unit
    unit: Unit,
}

/// Coordinates
#[derive(Deserialize, JsonSchema)]
#[expect(dead_code, reason = "schema only")]
struct Location {
    lat: f64,
    lon: f64,
    label: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[expect(dead_code, reason = "schema only")]
struct NestedObjects {
    location: Location,
}

#[derive(Deserialize, JsonSchema)]
#[expect(dead_code, reason = "schema only")]
struct TreeNode {
    name: String,
    children: Vec<TreeNode>,
}

#[derive(Deserialize, JsonSchema)]
struct EmptyObject {}

/// Search parameters
#[derive(Deserialize, JsonSchema)]
#[expect(dead_code, reason = "schema only")]
struct DescribedRoot {
    q: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[expect(dead_code, reason = "schema only")]
struct StrictObject {
    a: String,
}

#[derive(Deserialize, JsonSchema)]
#[expect(dead_code, reason = "schema only")]
struct OptionalFields {
    required: String,
    optional: Option<String>,
    #[serde(default = "three", rename = "withDefault")]
    with_default: f64,
}

fn three() -> f64 {
    3.0
}

#[derive(Deserialize, JsonSchema)]
#[expect(dead_code, reason = "schema only")]
struct Item {
    id: i64,
    done: bool,
}

#[derive(Deserialize, JsonSchema)]
#[expect(dead_code, reason = "schema only")]
struct Arrays {
    tags: Vec<String>,
    matrix: Vec<Vec<f64>>,
    #[schemars(length(min = 1))]
    items: Vec<Item>,
}

#[derive(Deserialize, JsonSchema)]
#[expect(dead_code, reason = "schema only")]
struct Numbers {
    float: f64,
    int: i64,
    #[schemars(range(min = 0, max = 100))]
    bounded: i64,
    #[schemars(range(min = 0))]
    small: u32,
}

#[derive(Deserialize, JsonSchema)]
#[serde(untagged)]
#[expect(dead_code, reason = "schema only")]
enum StringOrNumber {
    String(String),
    Number(f64),
}

#[derive(Deserialize, JsonSchema)]
#[expect(dead_code, reason = "schema only")]
struct Unions {
    value: StringOrNumber,
}

#[derive(Deserialize, JsonSchema)]
#[expect(dead_code, reason = "schema only")]
struct Records {
    labels: BTreeMap<String, String>,
}

#[derive(Deserialize, JsonSchema)]
#[expect(dead_code, reason = "schema only")]
struct StringFormats {
    #[schemars(email)]
    email: String,
    #[schemars(url)]
    url: String,
    #[schemars(regex(pattern = "^[a-z]+$"))]
    pattern: String,
    #[schemars(length(min = 1, max = 10))]
    lengths: String,
}

#[test]
fn matches_zod_for_a_described_string() {
    assert_eq!(
        tool_input_schema::<StringWithDescription>(),
        fixture("stringWithDescription")
    );
}

#[test]
fn matches_zod_for_a_described_enum() {
    assert_eq!(
        tool_input_schema::<Enums>(),
        json!({
            "type": "object",
            "properties": fixture("enums")["properties"].as_object().unwrap().clone().into_iter().take(1).collect::<Map<_, _>>(),
            "required": ["unit"],
            "additionalProperties": false,
        })
    );
}

#[test]
fn matches_zod_for_nested_objects_with_an_optional_field() {
    assert_eq!(tool_input_schema::<NestedObjects>(), fixture("nestedObjects"));
}

#[test]
fn matches_zod_for_a_recursive_type() {
    assert_eq!(tool_input_schema::<TreeNode>(), fixture("recursive"));
}

#[test]
fn matches_zod_for_an_empty_object() {
    assert_eq!(tool_input_schema::<EmptyObject>(), fixture("emptyObject"));
}

#[test]
fn matches_zod_for_a_described_root() {
    assert_eq!(tool_input_schema::<DescribedRoot>(), fixture("describedRoot"));
}

#[test]
fn matches_zod_for_a_strict_object() {
    assert_eq!(tool_input_schema::<StrictObject>(), fixture("strictObject"));
}

#[test]
fn matches_zod_for_a_union_of_plain_types() {
    let expected = without(fixture("unions"), "/properties", "maybe");
    let mut expected = expected;
    expected["required"] = json!(["value"]);
    assert_eq!(tool_input_schema::<Unions>(), expected);
}

#[test]
fn defaulted_fields_are_optional_and_keep_the_default_after_the_type() {
    let mut expected = fixture("optionalFields");
    expected = without(expected, "/properties", "nullable");
    expected["properties"]["withDefault"] = json!({ "type": "number", "default": 3.0 });
    expected["required"] = json!(["required"]);
    assert_eq!(tool_input_schema::<OptionalFields>(), expected);
}

#[test]
fn integers_have_no_safe_integer_bounds_and_lengths_follow_the_type() {
    assert_eq!(
        tool_input_schema::<Arrays>(),
        json!({
            "type": "object",
            "properties": {
                "tags": { "type": "array", "items": { "type": "string" } },
                "matrix": { "type": "array", "items": { "type": "array", "items": { "type": "number" } } },
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": { "id": { "type": "integer" }, "done": { "type": "boolean" } },
                        "required": ["id", "done"],
                        "additionalProperties": false,
                    },
                    "minItems": 1,
                },
            },
            "required": ["tags", "matrix", "items"],
            "additionalProperties": false,
        })
    );
    assert_eq!(
        tool_input_schema::<Numbers>()["properties"],
        json!({
            "float": { "type": "number" },
            "int": { "type": "integer" },
            "bounded": { "type": "integer", "minimum": 0, "maximum": 100 },
            "small": { "type": "integer", "minimum": 0 },
        })
    );
}

#[test]
fn maps_have_no_property_names_schema() {
    assert_eq!(
        tool_input_schema::<Records>(),
        without(fixture("records"), "/properties/labels", "propertyNames")
    );
}

#[test]
fn standard_formats_are_kept_without_zod_patterns() {
    assert_eq!(
        tool_input_schema::<StringFormats>()["properties"],
        json!({
            "email": { "type": "string", "format": "email" },
            "url": { "type": "string", "format": "uri" },
            "pattern": { "type": "string", "pattern": "^[a-z]+$" },
            "lengths": { "type": "string", "maxLength": 10, "minLength": 1 },
        })
    );
}

#[test]
fn a_property_named_title_is_kept() {
    #[derive(Deserialize, JsonSchema)]
    #[expect(dead_code, reason = "schema only")]
    struct Book {
        title: String,
        format: Option<String>,
    }
    assert_eq!(
        tool_input_schema::<Book>(),
        json!({
            "type": "object",
            "properties": { "title": { "type": "string" }, "format": { "type": "string" } },
            "required": ["title"],
            "additionalProperties": false,
        })
    );
}

#[test]
fn optional_enums_and_structs_lose_their_null_variant() {
    #[derive(Deserialize, JsonSchema)]
    #[expect(dead_code, reason = "schema only")]
    struct Settings {
        unit: Option<Unit>,
        location: Option<Location>,
        #[serde(default)]
        count: Option<u8>,
    }
    let schema = tool_input_schema::<Settings>();
    assert_eq!(
        schema["properties"]["unit"],
        json!({ "type": "string", "enum": ["celsius", "fahrenheit"] })
    );
    assert_eq!(
        schema["properties"]["location"],
        fixture("nestedObjects")["properties"]["location"]
    );
    assert_eq!(
        schema["properties"]["count"],
        json!({ "type": "integer", "minimum": 0, "maximum": 255 })
    );
    assert!(schema.get("required").is_none());
}

#[test]
fn normalize_drops_schema_and_title_at_every_level() {
    let schema = normalize(json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "Root",
        "type": "object",
        "properties": { "a": { "title": "A", "type": "number", "format": "double" } },
        "required": ["a"],
    }));
    assert_eq!(
        schema,
        json!({
            "type": "object",
            "properties": { "a": { "type": "number" } },
            "required": ["a"],
            "additionalProperties": false,
        })
    );
}

#[test]
fn the_validation_schema_follows_serde() {
    let schema = validation_schema::<Location>();
    assert_eq!(schema["properties"]["label"]["type"], json!(["string", "null"]));
    assert!(schema.get("additionalProperties").is_none());
}
