//! Schema normalization: schemars output shaped like Zod's `z.toJSONSchema` (section 8.8 and
//! the `tools/tool-spec.json` fixture).

use serde_json::json;

use super::*;

#[derive(schemars::JsonSchema)]
#[expect(dead_code, reason = "schema only")]
struct GetWeather {
    /// City name
    city: String,
    days: Option<i64>,
}

#[derive(schemars::JsonSchema)]
#[expect(dead_code, reason = "schema only")]
enum Unit {
    Celsius,
    Fahrenheit,
}

#[derive(schemars::JsonSchema)]
#[expect(dead_code, reason = "schema only")]
struct Nested {
    zeta: u32,
    alpha: f64,
    unit: Unit,
    point: Point,
    tags: Vec<String>,
}

#[derive(schemars::JsonSchema)]
#[expect(dead_code, reason = "schema only")]
struct Point {
    x: i32,
    y: i32,
}

#[derive(schemars::JsonSchema)]
#[expect(dead_code, reason = "schema only")]
struct Tree {
    children: Vec<Tree>,
}

#[test]
fn matches_the_zod_fixture_shape_for_an_equivalent_type() {
    let fixture: JsonValue = serde_json::from_str(
        &std::fs::read_to_string(format!(
            "{}/../conformance/fixtures/tools/tool-spec.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
    )
    .unwrap();
    let mut expected = fixture["inputSchema"].clone();
    // Zod's `.min(1)` has no Rust type equivalent; everything else matches.
    expected["properties"]["days"] = json!({ "type": "integer" });
    assert_eq!(schema_for::<GetWeather>(), expected);
}

#[test]
fn inlines_subschemas_keeps_field_order_and_drops_non_standard_formats() {
    assert_eq!(
        schema_for::<Nested>(),
        json!({
            "type": "object",
            "properties": {
                "zeta": { "type": "integer", "minimum": 0 },
                "alpha": { "type": "number" },
                "unit": { "type": "string", "enum": ["Celsius", "Fahrenheit"] },
                "point": {
                    "type": "object",
                    "properties": { "x": { "type": "integer" }, "y": { "type": "integer" } },
                    "required": ["x", "y"],
                    "additionalProperties": false,
                },
                "tags": { "type": "array", "items": { "type": "string" } },
            },
            "required": ["zeta", "alpha", "unit", "point", "tags"],
            "additionalProperties": false,
        })
    );
}

#[test]
fn recursive_types_keep_their_reference() {
    let schema = schema_for::<Tree>();
    assert!(schema.to_string().contains("$ref"), "{schema}");
    assert!(schema.get("$schema").is_none());
}
