//! Schema normalization against the Zod output in
//! `strands-rs/conformance/fixtures/tools/zod_tool_spec.json`.
//!
//! Intended differences from Zod: a field with a serde default is optional (schemars), where Zod's
//! output-mode schema lists defaulted fields in `required`; unsigned integers keep schemars'
//! `minimum: 0`.

use serde::Deserialize;
use serde_json::json;

use super::*;

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Guide,
    Api,
}

fn guide() -> Kind {
    Kind::Guide
}

#[derive(Deserialize, JsonSchema)]
#[allow(dead_code, reason = "schema-only test type")]
struct SearchDocs {
    /// Search terms
    query: String,
    /// Maximum results
    #[schemars(range(min = 1, max = 50))]
    limit: Option<u32>,
    #[serde(default = "guide")]
    #[schemars(default = "guide_value")]
    kind: Kind,
    filters: Option<Vec<String>>,
}

fn guide_value() -> &'static str {
    "guide"
}

#[test]
fn matches_the_zod_fixture_up_to_the_logged_differences() {
    let fixture: JsonValue = serde_json::from_str(include_str!(
        "../../../../conformance/fixtures/tools/zod_tool_spec.json"
    ))
    .unwrap();
    let mut expected = fixture["inputSchema"].clone();
    expected["required"] = json!(["query"]);
    assert_eq!(input_schema_for::<SearchDocs>(), expected);
}

#[test]
fn keeps_property_order() {
    let schema = input_schema_for::<SearchDocs>();
    let keys: Vec<&String> = schema["properties"].as_object().unwrap().keys().collect();
    assert_eq!(keys, ["query", "limit", "kind", "filters"]);
}

#[derive(Deserialize, JsonSchema)]
struct Node {
    #[allow(dead_code, reason = "schema-only test type")]
    value: i64,
    #[allow(dead_code, reason = "schema-only test type")]
    children: Vec<Node>,
}

#[derive(Deserialize, JsonSchema)]
struct Tree {
    #[allow(dead_code, reason = "schema-only test type")]
    root: Node,
}

#[test]
fn recursive_types_keep_their_reference() {
    let schema = input_schema_for::<Tree>();
    assert_eq!(
        schema["properties"]["root"]["properties"]["children"]["items"],
        json!({"$ref": "#/$defs/Node"})
    );
    assert_eq!(
        schema["$defs"]["Node"]["properties"]["value"],
        json!({"type": "integer"})
    );
    assert!(schema.get("title").is_none());
    assert!(schema.get("$schema").is_none());
}

#[test]
fn drops_non_standard_formats_and_keeps_standard_ones() {
    let schema = normalize_schema(json!({
        "type": "object",
        "properties": {
            "n": {"type": "number", "format": "double"},
            "at": {"type": "string", "format": "date-time"}
        },
        "required": ["n", "at"]
    }));
    assert_eq!(
        schema,
        json!({
            "type": "object",
            "properties": {"n": {"type": "number"}, "at": {"type": "string", "format": "date-time"}},
            "required": ["n", "at"],
            "additionalProperties": false
        })
    );
}

#[test]
fn optional_references_lose_the_null_branch() {
    let schema = normalize_schema(json!({
        "type": "object",
        "properties": {"kind": {"anyOf": [{"$ref": "#/$defs/Kind"}, {"type": "null"}]}},
        "$defs": {"Kind": {"type": "string", "enum": ["a", "b"]}}
    }));
    assert_eq!(
        schema,
        json!({
            "type": "object",
            "properties": {"kind": {"type": "string", "enum": ["a", "b"]}},
            "additionalProperties": false
        })
    );
}

#[test]
fn required_nullable_properties_keep_null() {
    let schema = normalize_schema(json!({
        "type": "object",
        "properties": {"v": {"type": ["string", "null"]}},
        "required": ["v"]
    }));
    assert_eq!(schema["properties"]["v"]["type"], json!(["string", "null"]));
}
