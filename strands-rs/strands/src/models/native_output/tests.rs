//! Ported from strands-ts/src/models/__tests__/native-output.test.ts
//!
//! TS builds its schemas with Zod; these are the JSON schemas `z.toJSONSchema` writes for them.

use serde_json::{Value, json};

use super::StructuredOutputMechanism::{Auto, Forced, Native};
use super::*;
use crate::models::model_info::ModelCapabilities;
use crate::test_support::capture_logs;

fn person() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "age": { "type": "integer", "minimum": -9_007_199_254_740_991_i64, "maximum": 9_007_199_254_740_991_i64 },
        },
        "required": ["name", "age"],
        "additionalProperties": false,
    })
}

fn roster() -> Value {
    json!({
        "description": "A team of people",
        "type": "object",
        "properties": {
            "members": { "minItems": 2, "type": "array", "items": person() },
            "lead": { "anyOf": [person(), { "type": "null" }] },
            "code": { "description": "Team code", "type": "string", "pattern": "^[A-Z]+$" },
            "founded": { "type": "string", "format": "date", "pattern": "^\\d{4}-\\d{2}-\\d{2}$" },
            "score": { "type": "number", "minimum": 0 },
        },
        "required": ["members", "code", "founded", "score"],
        "additionalProperties": false,
    })
}

fn team() -> Value {
    let mut team = roster();
    team["properties"]["color"] = json!({ "type": "string", "enum": ["red", "blue"] });
    team["properties"]["kind"] = json!({ "type": "string", "enum": ["a", "b"] });
    team
}

fn with_record() -> Value {
    json!({
        "type": "object",
        "properties": {
            "scores": { "type": "object", "propertyNames": { "type": "string" }, "additionalProperties": { "type": "number" } },
        },
        "required": ["scores"],
        "additionalProperties": false,
    })
}

fn object(properties: Value) -> Value {
    let required: Vec<String> = properties.as_object().unwrap().keys().cloned().collect();
    let mut schema = json!({ "type": "object", "required": required, "additionalProperties": false });
    schema
        .as_object_mut()
        .unwrap()
        .shift_insert(1, "properties".into(), properties);
    schema
}

fn with_tuple() -> Value {
    object(
        json!({ "point": { "type": "array", "prefixItems": [{ "type": "number" }, { "type": "number" }] } }),
    )
}

fn with_any() -> Value {
    object(json!({ "value": {} }))
}

fn forced() -> ToolChoice {
    ToolChoice::tool("strands_structured_output")
}

fn mechanisms(
    support: &StructuredOutputSupport,
    model_id: &str,
    schema: &Value,
) -> Vec<StructuredOutputMechanism> {
    support.structured_output_mechanisms("p", model_id, Some(&forced()), schema, &ModelInfo::default())
}

#[test]
fn lists_the_mechanisms_strongest_first_for_name() {
    let support = StructuredOutputSupport::default();
    let cases: [(Option<ToolChoice>, &[StructuredOutputMechanism]); 4] = [
        (Some(forced()), &[Forced, Native, Auto]),
        (
            Some(serde_json::from_value(json!({ "any": {} })).unwrap()),
            &[Forced, Native, Auto],
        ),
        (
            Some(serde_json::from_value(json!({ "auto": {} })).unwrap()),
            &[Native, Auto],
        ),
        (None, &[Native, Auto]),
    ];
    for (tool_choice, expected) in cases {
        let found = support.structured_output_mechanisms(
            "p",
            "m",
            tool_choice.as_ref(),
            &person(),
            &ModelInfo::default(),
        );
        assert_eq!(found, expected, "{tool_choice:?}");
    }
}

fn info(forced_tool_choice: Option<bool>, native_structured_output: Option<bool>) -> ModelInfo {
    ModelInfo {
        capabilities: Some(ModelCapabilities {
            forced_tool_choice,
            native_structured_output,
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[test]
fn skips_mechanisms_the_model_info_says_are_unsupported_without_a_request() {
    let support = StructuredOutputSupport::default();
    let run =
        |info: ModelInfo| support.structured_output_mechanisms("p", "m", Some(&forced()), &person(), &info);
    assert_eq!(run(info(Some(false), None)), [Native, Auto]);
    assert_eq!(run(info(None, Some(false))), [Forced, Auto]);
    assert_eq!(run(info(Some(true), Some(true))), [Forced, Native, Auto]);
}

#[test]
fn skips_mechanisms_the_model_was_learned_to_reject() {
    let support = StructuredOutputSupport::default();
    support.handle_rejected_mechanism(
        "p",
        "m",
        Forced,
        &person(),
        FORCED_TOOL_CHOICE_UNSUPPORTED_MESSAGES[0],
    );
    assert_eq!(mechanisms(&support, "m", &person()), [Native, Auto]);
    support.handle_rejected_mechanism("p", "m", Native, &person(), NATIVE_OUTPUT_UNSUPPORTED_MESSAGES[0]);
    assert_eq!(mechanisms(&support, "m", &person()), [Auto]);
}

#[test]
fn skips_native_output_and_warns_for_a_schema_it_cannot_represent() {
    let support = StructuredOutputSupport::default();
    let (found, logs) = capture_logs(|| mechanisms(&support, "m", &with_record()));
    assert_eq!(found, [Forced, Auto]);
    // Warn-once: logged here unless an earlier test in this process logged it.
    assert!(
        logs.iter().all(|log| log.contains("feature=<record fields>")),
        "{logs:?}"
    );
}

#[test]
fn does_not_inspect_the_schema_when_native_output_is_known_unsupported() {
    let support = StructuredOutputSupport::default();
    support.handle_rejected_mechanism(
        "p",
        "m",
        Native,
        &with_tuple(),
        NATIVE_OUTPUT_UNSUPPORTED_MESSAGES[0],
    );
    let (found, logs) = capture_logs(|| mechanisms(&support, "m", &with_tuple()));
    assert_eq!(found, [Forced, Auto]);
    assert!(logs.is_empty(), "{logs:?}");
}

#[test]
fn remembers_that_the_model_rejects_mechanism_for_every_schema_signal() {
    let cases = FORCED_TOOL_CHOICE_UNSUPPORTED_MESSAGES
        .iter()
        .map(|signal| (Forced, *signal))
        .chain(
            NATIVE_OUTPUT_UNSUPPORTED_MESSAGES
                .iter()
                .map(|signal| (Native, *signal)),
        );
    for (mechanism, signal) in cases {
        let support = StructuredOutputSupport::default();
        let message = format!("ValidationException: {signal}.");
        assert!(support.handle_rejected_mechanism("p", "m", mechanism, &person(), &message));
        assert!(
            !mechanisms(&support, "m", &team()).contains(&mechanism),
            "{signal}"
        );
        assert!(mechanisms(&support, "other-model", &person()).contains(&mechanism));
        let other_provider = support.structured_output_mechanisms(
            "other-provider",
            "m",
            Some(&forced()),
            &person(),
            &ModelInfo::default(),
        );
        assert!(other_provider.contains(&mechanism));
    }
}

#[test]
fn remembers_a_rejected_native_schema_for_that_schema_only_s() {
    for signal in NATIVE_OUTPUT_REJECTED_MESSAGES {
        let support = StructuredOutputSupport::default();
        let message = format!("invalid_request_error: {signal}");
        let (handled, logs) =
            capture_logs(|| support.handle_rejected_mechanism("p", "m", Native, &person(), &message));
        assert!(handled);
        assert!(
            logs.iter()
                .any(|log| log.contains("native structured output rejected the schema"))
        );
        assert_eq!(mechanisms(&support, "m", &person()), [Forced, Auto]);
        assert_eq!(mechanisms(&support, "m", &team()), [Forced, Native, Auto]);
    }
}

#[test]
fn ignores_an_unrelated_mechanism_error_message() {
    let cases = [
        (Forced, "Input is too long for requested model"),
        (Forced, NATIVE_OUTPUT_REJECTED_MESSAGES[0]),
        (Native, FORCED_TOOL_CHOICE_UNSUPPORTED_MESSAGES[0]),
        (Auto, FORCED_TOOL_CHOICE_UNSUPPORTED_MESSAGES[0]),
    ];
    for (mechanism, message) in cases {
        let support = StructuredOutputSupport::default();
        assert!(!support.handle_rejected_mechanism("p", "m", mechanism, &person(), message));
        assert_eq!(mechanisms(&support, "m", &person()), [Forced, Native, Auto]);
    }
}

#[test]
fn returns_expected_for_name() {
    let mut nested_record =
        object(json!({ "team": roster(), "extra": { "type": "array", "items": with_record() } }));
    nested_record["properties"]["team"]["additionalProperties"] = json!(false);
    let with_catchall = json!({
        "type": "object",
        "properties": { "name": { "type": "string" } },
        "required": ["name"],
        "additionalProperties": { "type": "string" },
    });
    let cases = [
        (person(), None),
        (team(), None),
        (with_record(), Some("record fields")),
        (nested_record, Some("record fields")),
        (with_catchall, Some("record fields")),
        (
            json!({ "type": "object", "patternProperties": { "^a": { "type": "string" } } }),
            Some("record fields"),
        ),
        (with_tuple(), Some("tuple fields")),
        (with_any(), Some("untyped fields")),
        (
            object(json!({ "value": { "enum": ["a", 1] } })),
            Some("untyped fields"),
        ),
    ];
    for (schema, expected) in cases {
        assert_eq!(
            find_unsupported_native_output_feature(&schema),
            expected,
            "{schema}"
        );
    }
}

#[test]
fn keeps_enums_which_native_output_supports() {
    let schema = object(json!({ "color": { "type": "string", "enum": ["red", "blue"] } }));
    assert_eq!(
        to_native_output_schema(&schema).unwrap(),
        json!({
            "type": "object",
            "properties": { "color": { "type": "string", "enum": ["red", "blue"] } },
            "additionalProperties": false,
            "required": ["color"],
        })
    );
}

#[test]
fn keeps_defs_alongside_a_ref_and_moves_unsupported_array_keywords_into_the_description() {
    let schema = json!({ "$defs": { "Item": { "type": "array", "items": false, "minItems": 3 } }, "$ref": "#/$defs/Item" });
    assert_eq!(
        to_native_output_schema(&schema).unwrap(),
        json!({
            "$defs": { "Item": { "type": "array", "description": "{items: false, minItems: 3}" } },
            "$ref": "#/$defs/Item",
        })
    );
}

#[test]
fn converts_one_of_to_any_of_keeps_all_of_and_moves_unsupported_formats_and_item_bounds_into_the_description()
{
    let schema = json!({
        "type": "object",
        "properties": {
            "choice": { "oneOf": [{ "type": "string", "format": "phone" }, { "type": "integer" }] },
            "merged": { "allOf": [{ "type": "object", "properties": { "a": { "type": "string" } } }] },
            "tags": { "type": "array", "items": { "type": "string" }, "minItems": 1 },
        },
    });
    assert_eq!(
        to_native_output_schema(&schema).unwrap(),
        json!({
            "type": "object",
            "properties": {
                "choice": { "anyOf": [{ "type": "string", "description": "{format: \"phone\"}" }, { "type": "integer" }] },
                "merged": {
                    "allOf": [{ "type": "object", "properties": { "a": { "type": "string" } }, "additionalProperties": false }],
                },
                "tags": { "type": "array", "items": { "type": "string" }, "minItems": 1 },
            },
            "additionalProperties": false,
        })
    );
}

#[test]
fn does_not_mutate_the_input_schema() {
    let schema = team();
    let copy = schema.clone();
    to_native_output_schema(&schema).unwrap();
    assert_eq!(schema, copy);
}

#[test]
fn throws_for_an_untyped_node() {
    let error = to_native_output_schema(&with_any()).unwrap_err();
    assert!(error.to_string().starts_with("Schema must have a 'type'"));
}

#[test]
fn appends_unsupported_keywords_to_an_existing_description() {
    let converted = to_native_output_schema(&roster()).unwrap();
    assert_eq!(
        converted["properties"]["code"]["description"],
        "Team code\n\n{pattern: \"^[A-Z]+$\"}"
    );
    assert_eq!(converted["properties"]["members"]["description"], "{minItems: 2}");
    assert_eq!(converted["properties"]["founded"]["format"], "date");
}
