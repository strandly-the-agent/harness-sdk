//! Ported from strands-ts/src/models/__tests__/native-output.test.ts. The Zod schemas of the TS spec
//! appear as the JSON schemas TS `zodSchemaToJsonSchema` writes for them, and the expected
//! conversions as the Anthropic SDK's `transformJSONSchema` output, both in the conformance fixture
//! `schemas/native_output_schemas.json` (`conformance/src/native-output.ts`).

use std::sync::LazyLock;

use serde_json::json;

use super::StructuredOutputMechanism::{Auto, Forced, Native};
use super::{
    FORCED_TOOL_CHOICE_UNSUPPORTED_MESSAGES, NATIVE_OUTPUT_REJECTED_MESSAGES,
    NATIVE_OUTPUT_UNSUPPORTED_MESSAGES, StructuredOutputMechanism, StructuredOutputSupport,
    find_unsupported_native_output_feature, to_native_output_schema,
};
use crate::models::model_info::{ModelCapabilities, ModelInfo};
use crate::types::json::JsonValue;
use crate::types::tools::ToolChoice;

static FIXTURES: LazyLock<JsonValue> = LazyLock::new(|| {
    serde_json::from_str(include_str!(
        "../../../../conformance/fixtures/schemas/native_output_schemas.json"
    ))
    .unwrap()
});

fn schema(name: &str) -> JsonValue {
    FIXTURES["schemas"][name].clone()
}

fn transformed(name: &str) -> JsonValue {
    FIXTURES["transformed"][name].clone()
}

fn forced() -> Option<ToolChoice> {
    Some(ToolChoice::tool("strands_structured_output"))
}

fn info(capabilities: ModelCapabilities) -> ModelInfo {
    ModelInfo {
        capabilities: Some(capabilities),
        limits: None,
    }
}

fn mechanisms(
    support: &StructuredOutputSupport,
    provider: &str,
    model_id: &str,
    schema_name: &str,
) -> Vec<StructuredOutputMechanism> {
    support.structured_output_mechanisms(
        provider,
        model_id,
        forced().as_ref(),
        &schema(schema_name),
        &ModelInfo::default(),
    )
}

#[test]
fn lists_the_mechanisms_strongest_first_for_name() {
    let cases = [
        ("a forced tool", forced(), vec![Forced, Native, Auto]),
        ("any tool", Some(ToolChoice::any()), vec![Forced, Native, Auto]),
        ("auto tool choice", Some(ToolChoice::auto()), vec![Native, Auto]),
        ("no tool choice", None, vec![Native, Auto]),
    ];
    for (name, tool_choice, expected) in cases {
        let support = StructuredOutputSupport::default();
        let found = support.structured_output_mechanisms(
            "p",
            "m",
            tool_choice.as_ref(),
            &schema("Person"),
            &ModelInfo::default(),
        );
        assert_eq!(found, expected, "{name}");
    }
}

#[test]
fn skips_mechanisms_the_model_info_says_are_unsupported_without_a_request() {
    let support = StructuredOutputSupport::default();
    let person = schema("Person");
    let with = |capabilities| {
        support.structured_output_mechanisms("p", "m", forced().as_ref(), &person, &info(capabilities))
    };
    assert_eq!(
        with(ModelCapabilities {
            forced_tool_choice: Some(false),
            ..ModelCapabilities::default()
        }),
        [Native, Auto]
    );
    assert_eq!(
        with(ModelCapabilities {
            native_structured_output: Some(false),
            ..ModelCapabilities::default()
        }),
        [Forced, Auto]
    );
    assert_eq!(
        with(ModelCapabilities {
            forced_tool_choice: Some(true),
            native_structured_output: Some(true),
            ..ModelCapabilities::default()
        }),
        [Forced, Native, Auto]
    );
}

#[test]
fn skips_mechanisms_the_model_was_learned_to_reject() {
    let support = StructuredOutputSupport::default();
    let person = schema("Person");
    support.handle_rejected_mechanism(
        "p",
        "m",
        Forced,
        &person,
        FORCED_TOOL_CHOICE_UNSUPPORTED_MESSAGES[0],
    );
    assert_eq!(mechanisms(&support, "p", "m", "Person"), [Native, Auto]);
    support.handle_rejected_mechanism("p", "m", Native, &person, NATIVE_OUTPUT_UNSUPPORTED_MESSAGES[0]);
    assert_eq!(mechanisms(&support, "p", "m", "Person"), [Auto]);
}

#[test]
fn skips_native_output_and_warns_for_a_schema_it_cannot_represent() {
    let support = StructuredOutputSupport::default();
    assert_eq!(mechanisms(&support, "p", "m", "WithRecord"), [Forced, Auto]);
}

#[test]
fn does_not_inspect_the_schema_when_native_output_is_known_unsupported() {
    let support = StructuredOutputSupport::default();
    support.handle_rejected_mechanism(
        "p",
        "m",
        Native,
        &schema("WithTuple"),
        NATIVE_OUTPUT_UNSUPPORTED_MESSAGES[0],
    );
    assert_eq!(mechanisms(&support, "p", "m", "WithTuple"), [Forced, Auto]);
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
        assert!(
            support.handle_rejected_mechanism("p", "m", mechanism, &schema("Person"), &message),
            "{signal}"
        );
        assert!(
            !mechanisms(&support, "p", "m", "Team").contains(&mechanism),
            "{signal}"
        );
        assert!(
            mechanisms(&support, "p", "other-model", "Person").contains(&mechanism),
            "{signal}"
        );
        assert!(
            mechanisms(&support, "other-provider", "m", "Person").contains(&mechanism),
            "{signal}"
        );
    }
}

#[test]
fn remembers_a_rejected_native_schema_for_that_schema_only_signal() {
    for signal in NATIVE_OUTPUT_REJECTED_MESSAGES {
        let support = StructuredOutputSupport::default();
        let message = format!("invalid_request_error: {signal}");
        assert!(
            support.handle_rejected_mechanism("p", "m", Native, &schema("Person"), &message),
            "{signal}"
        );
        assert_eq!(
            mechanisms(&support, "p", "m", "Person"),
            [Forced, Auto],
            "{signal}"
        );
        assert_eq!(
            mechanisms(&support, "p", "m", "Team"),
            [Forced, Native, Auto],
            "{signal}"
        );
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
        assert!(
            !support.handle_rejected_mechanism("p", "m", mechanism, &schema("Person"), message),
            "{message}"
        );
        assert_eq!(
            mechanisms(&support, "p", "m", "Person"),
            [Forced, Native, Auto],
            "{message}"
        );
    }
}

#[test]
fn clear_forgets_what_was_learned() {
    let support = StructuredOutputSupport::default();
    support.handle_rejected_mechanism(
        "p",
        "m",
        Forced,
        &schema("Person"),
        FORCED_TOOL_CHOICE_UNSUPPORTED_MESSAGES[0],
    );
    support.handle_rejected_mechanism(
        "p",
        "m",
        Native,
        &schema("Person"),
        NATIVE_OUTPUT_REJECTED_MESSAGES[0],
    );
    support.clear();
    assert_eq!(mechanisms(&support, "p", "m", "Person"), [Forced, Native, Auto]);
}

#[test]
fn returns_expected_for_name() {
    let cases = [
        ("a flat object", schema("Person"), None),
        ("nested objects, unions, enums and formats", schema("Team"), None),
        ("a record", schema("WithRecord"), Some("record fields")),
        (
            "a nested record",
            schema("WithNestedRecord"),
            Some("record fields"),
        ),
        ("a catchall", schema("WithCatchall"), Some("record fields")),
        (
            "pattern properties",
            json!({ "type": "object", "patternProperties": { "^a": { "type": "string" } } }),
            Some("record fields"),
        ),
        ("a tuple", schema("WithTuple"), Some("tuple fields")),
        ("an untyped field", schema("WithAny"), Some("untyped fields")),
        (
            "a mixed literal",
            schema("WithMixedLiteral"),
            Some("untyped fields"),
        ),
    ];
    for (name, schema, expected) in cases {
        assert_eq!(
            find_unsupported_native_output_feature(&schema),
            expected,
            "{name}"
        );
    }
}

#[test]
fn matches_the_anthropic_sdk_transform_for_name() {
    for name in ["Person", "Roster", "WithNestedRecord"] {
        assert_eq!(
            to_native_output_schema(&schema(name)).unwrap(),
            transformed(name),
            "{name}"
        );
    }
}

#[test]
fn keeps_enums_which_native_output_supports() {
    assert_eq!(
        to_native_output_schema(&schema("Enum")).unwrap(),
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
        json!({ "$defs": { "Item": { "type": "array", "description": "{items: false, minItems: 3}" } }, "$ref": "#/$defs/Item" })
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
    let schema = schema("Team");
    let copy = schema.clone();
    to_native_output_schema(&schema).unwrap();
    assert_eq!(schema, copy);
}

#[test]
fn throws_for_an_untyped_node() {
    let error = to_native_output_schema(&schema("WithAny")).unwrap_err();
    assert!(
        error.to_string().starts_with("Schema must have a 'type'"),
        "{error}"
    );
}
