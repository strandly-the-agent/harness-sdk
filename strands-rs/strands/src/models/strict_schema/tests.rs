//! Ported from strands-ts/src/models/__tests__/strict-schema.test.ts

use serde_json::{Value, json};

use super::*;
use crate::test_support::capture_logs;

#[test]
fn adds_additional_properties_false_to_a_basic_object_and_does_not_mutate_the_original() {
    let original = json!({ "type": "object", "properties": { "x": { "type": "string" } } });
    let result = ensure_strict_json_schema(&original);
    assert_eq!(
        result,
        json!({ "type": "object", "properties": { "x": { "type": "string" } }, "additionalProperties": false })
    );
    assert!(original.get("additionalProperties").is_none());
}

#[test]
fn recurses_into_nested_objects() {
    let result = ensure_strict_json_schema(&json!({
        "type": "object",
        "properties": { "outer": { "type": "object", "properties": { "inner": { "type": "integer" } } } },
    }));
    assert_eq!(
        result,
        json!({
            "type": "object",
            "properties": {
                "outer": { "type": "object", "properties": { "inner": { "type": "integer" } }, "additionalProperties": false },
            },
            "additionalProperties": false,
        })
    );
}

fn defs_case(keyword: &str) {
    let reference = format!("#/{keyword}/MyItem");
    let result = ensure_strict_json_schema(&json!({
        "type": "object",
        "properties": { "item": { "$ref": reference } },
        keyword: { "MyItem": { "type": "object", "properties": { "name": { "type": "string" } } } },
    }));
    assert_eq!(
        result,
        json!({
            "type": "object",
            "properties": { "item": { "$ref": reference } },
            keyword: { "MyItem": { "type": "object", "properties": { "name": { "type": "string" } }, "additionalProperties": false } },
            "additionalProperties": false,
        })
    );
}

#[test]
fn processes_defs_blocks() {
    defs_case("$defs");
}

#[test]
fn processes_definitions_blocks() {
    defs_case("definitions");
}

#[test]
fn inlines_a_ref_that_has_sibling_keys_existing_keys_winning() {
    let result = ensure_strict_json_schema(&json!({
        "type": "object",
        "properties": { "item": { "$ref": "#/$defs/MyItem", "description": "An item" } },
        "$defs": { "MyItem": { "type": "object", "description": "from $defs", "properties": { "name": { "type": "string" } } } },
    }));
    assert_eq!(
        result["properties"]["item"],
        json!({
            "type": "object",
            "description": "An item",
            "properties": { "name": { "type": "string" } },
            "additionalProperties": false,
        })
    );
}

#[test]
fn deep_copies_on_inline_so_repeated_refs_are_independent() {
    let result = ensure_strict_json_schema(&json!({
        "type": "object",
        "properties": {
            "a": { "$ref": "#/$defs/Shared", "description": "first" },
            "b": { "$ref": "#/$defs/Shared", "description": "second" },
        },
        "$defs": { "Shared": { "type": "object", "properties": { "val": { "type": "string" } } } },
    }));
    assert_eq!(result["properties"]["a"]["description"], "first");
    assert_eq!(result["properties"]["b"]["description"], "second");
}

fn closed(properties: Value) -> Value {
    let mut schema = json!({ "type": "object", "additionalProperties": false });
    schema
        .as_object_mut()
        .unwrap()
        .shift_insert(1, "properties".into(), properties);
    schema
}

fn open(properties: Value) -> Value {
    let mut schema = json!({ "type": "object" });
    schema
        .as_object_mut()
        .unwrap()
        .shift_insert(1, "properties".into(), properties);
    schema
}

#[test]
fn recurses_into_array_items_any_of_and_all_of() {
    let result = ensure_strict_json_schema(&open(json!({
        "items": { "type": "array", "items": open(json!({ "a": { "type": "string" } })) },
        "union": { "anyOf": [open(json!({ "b": { "type": "string" } })), { "type": "null" }] },
        "intersection": { "allOf": [open(json!({ "c": { "type": "string" } }))] },
    })));
    assert_eq!(
        result,
        closed(json!({
            "items": { "type": "array", "items": closed(json!({ "a": { "type": "string" } })) },
            "union": { "anyOf": [closed(json!({ "b": { "type": "string" } })), { "type": "null" }] },
            "intersection": { "allOf": [closed(json!({ "c": { "type": "string" } }))] },
        }))
    );
}

#[test]
fn recurses_into_one_of() {
    let result = ensure_strict_json_schema(&open(json!({
        "value": { "oneOf": [open(json!({ "a": { "type": "string" } })), open(json!({ "b": { "type": "integer" } }))] },
    })));
    assert_eq!(
        result,
        closed(json!({
            "value": { "oneOf": [closed(json!({ "a": { "type": "string" } })), closed(json!({ "b": { "type": "integer" } }))] },
        }))
    );
}

#[test]
fn leaves_required_unchanged() {
    let result = ensure_strict_json_schema(&json!({
        "type": "object",
        "properties": { "required_field": { "type": "string" }, "optional_field": { "type": "string" } },
        "required": ["required_field"],
    }));
    assert_eq!(result["required"], json!(["required_field"]));
}

#[test]
fn preserves_an_existing_additional_properties_true() {
    let schema = json!({ "type": "object", "properties": { "x": { "type": "string" } }, "additionalProperties": true });
    assert_eq!(ensure_strict_json_schema(&schema), schema);
}

#[test]
fn preserves_an_existing_additional_properties_false() {
    let schema = closed(json!({ "x": { "type": "string" } }));
    assert_eq!(ensure_strict_json_schema(&schema), schema);
}

#[test]
fn leaves_a_non_object_type_unchanged() {
    assert_eq!(
        ensure_strict_json_schema(&json!({ "type": "string" })),
        json!({ "type": "string" })
    );
}

#[test]
fn ignores_a_ref_that_does_not_start_with_but_still_closes_the_root() {
    let (result, logs) = capture_logs(|| {
        ensure_strict_json_schema(&json!({
            "type": "object",
            "properties": { "item": { "$ref": "external.json#/Foo", "description": "ext" } },
        }))
    });
    assert_eq!(result["additionalProperties"], false);
    assert_eq!(result["properties"]["item"]["$ref"], "external.json#/Foo");
    assert_eq!(
        logs,
        ["WARN ref=<external.json#/Foo> | unexpected $ref format, skipping resolution"]
    );
}

#[test]
fn ignores_a_ref_pointing_at_a_missing_path_but_still_closes_the_root() {
    let (result, logs) = capture_logs(|| {
        ensure_strict_json_schema(&json!({
            "type": "object",
            "properties": { "item": { "$ref": "#/$defs/Missing", "description": "gone" } },
            "$defs": {},
        }))
    });
    assert_eq!(result["additionalProperties"], false);
    assert!(result["properties"]["item"].get("$ref").is_some());
    assert_eq!(logs, ["WARN ref=<#/$defs/Missing> | failed to resolve $ref path"]);
}

#[test]
fn terminates_on_a_recursive_ref_with_sibling_keys_leaving_the_cycle_point_unresolved() {
    let (result, logs) = capture_logs(|| {
        ensure_strict_json_schema(&json!({
            "type": "object",
            "properties": { "filter": { "$ref": "#/definitions/Filter", "description": "root filter" } },
            "definitions": {
                "Filter": {
                    "type": "object",
                    "properties": { "and": { "type": "array", "items": { "$ref": "#/definitions/Filter", "description": "sub" } } },
                },
            },
        }))
    });
    let filter = &result["properties"]["filter"];
    assert_eq!(filter["description"], "root filter");
    assert_eq!(filter["additionalProperties"], false);
    let mut node = filter;
    while node.get("$ref").is_none() {
        node = &node["properties"]["and"]["items"];
    }
    assert_eq!(
        node,
        &json!({ "$ref": "#/definitions/Filter", "description": "sub" })
    );
    // Warn-once: the warning is logged here unless an earlier test in this process logged it.
    assert!(
        logs.iter()
            .all(|log| log.contains("ref=<#/definitions/Filter> | recursive $ref")),
        "{logs:?}"
    );
}

#[test]
fn does_not_mutate_nested_objects_or_defs_of_the_original() {
    let original = json!({
        "type": "object",
        "properties": {
            "outer": { "type": "object", "properties": { "inner": { "type": "string" } } },
            "item": { "$ref": "#/$defs/Item", "description": "an item" },
        },
        "$defs": { "Item": { "type": "object", "properties": { "name": { "type": "string" } } } },
    });
    let snapshot = original.clone();
    ensure_strict_json_schema(&original);
    assert_eq!(original, snapshot);
}

#[test]
fn closes_an_object_whose_type_is_a_union_including_object() {
    let result = ensure_strict_json_schema(&open(json!({
        "addr": { "type": ["object", "null"], "properties": { "city": { "type": "string" } } },
    })));
    assert_eq!(
        result,
        closed(json!({
            "addr": { "type": ["object", "null"], "properties": { "city": { "type": "string" } }, "additionalProperties": false },
        }))
    );
}

#[test]
fn closes_objects_inside_a_schema_valued_additional_properties() {
    let result = ensure_strict_json_schema(&json!({
        "type": "object",
        "additionalProperties": open(json!({ "v": { "type": "string" } })),
    }));
    assert_eq!(
        result,
        json!({ "type": "object", "additionalProperties": closed(json!({ "v": { "type": "string" } })) })
    );
}

#[test]
fn keeps_a_ref_target_s_additional_properties_when_the_ref_node_also_declares_type_object() {
    let result = ensure_strict_json_schema(&json!({
        "type": "object",
        "properties": { "m": { "$ref": "#/$defs/Map", "type": "object" } },
        "$defs": { "Map": { "type": "object", "additionalProperties": { "type": "string" } } },
    }));
    assert_eq!(
        result["properties"]["m"],
        json!({ "type": "object", "additionalProperties": { "type": "string" } })
    );
}

#[test]
fn does_not_resolve_a_ref_through_the_prototype_chain() {
    let result = ensure_strict_json_schema(&json!({
        "type": "object",
        "properties": { "x": { "$ref": "#/$defs/__proto__", "description": "d" } },
        "$defs": {},
    }));
    assert_eq!(
        result["properties"]["x"],
        json!({ "$ref": "#/$defs/__proto__", "description": "d" })
    );
}

#[test]
fn keeps_an_inlined_proto_key_as_an_own_property_and_still_closes_the_node() {
    let schema: Value = serde_json::from_str(
        r##"{"type":"object","properties":{"x":{"$ref":"#/$defs/Item","description":"d"}},"$defs":{"Item":{"type":"object","__proto__":{"additionalProperties":true}}}}"##,
    )
    .unwrap();
    let x = &ensure_strict_json_schema(&schema)["properties"]["x"];
    assert_eq!(x["additionalProperties"], false);
    assert_eq!(x["__proto__"], json!({ "additionalProperties": true }));
}

#[test]
fn returns_nothing_for_a_closed_schema_without_bounds() {
    let strict = ensure_strict_json_schema(&open(json!({
        "a": { "type": "string" },
        "b": { "type": "array", "items": { "type": "integer" } },
    })));
    assert!(find_unsupported_strict_keywords(&strict).is_empty());
}

#[test]
fn reports_open_additional_properties_and_bounds_anywhere_in_the_schema_sorted_and_deduplicated() {
    let keywords = find_unsupported_strict_keywords(&json!({
        "type": "object",
        "properties": {
            "headers": { "type": "object", "additionalProperties": { "type": "string", "maxLength": 10 } },
            "count": { "type": "integer", "minimum": 0, "maximum": 5 },
            "timeout": { "type": "number", "exclusiveMinimum": 0, "exclusiveMaximum": 60 },
            "tags": { "type": "array", "items": { "type": "string", "minLength": 1 } },
        },
        "$defs": { "Step": { "type": "number", "multipleOf": 2, "minimum": 1 } },
        "additionalProperties": false,
    }));
    assert_eq!(
        keywords,
        [
            "additionalProperties",
            "exclusiveMaximum",
            "exclusiveMinimum",
            "maxLength",
            "maximum",
            "minLength",
            "minimum",
            "multipleOf",
        ]
    );
}

#[test]
fn does_not_treat_property_names_as_keywords() {
    let keywords = find_unsupported_strict_keywords(&closed(json!({ "minimum": { "type": "number" } })));
    assert!(keywords.is_empty());
}
