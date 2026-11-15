//! Ported from strands-ts/src/models/__tests__/native-output.test.ts
//!
//! `schemas.json` holds the JSON schemas strands-ts derives from the spec's zod schemas
//! (`zodSchemaToJsonSchema`) and the Anthropic SDK's `transformJSONSchema` output for three of them.

use std::sync::LazyLock;

use serde_json::json;

use super::StructuredOutputMechanism::{Auto, Forced, Native};
use super::*;
use crate::models::model_info::ModelCapabilities;
use crate::test_support::WarnRecorder;

static DATA: LazyLock<JsonValue> =
    LazyLock::new(|| serde_json::from_str(include_str!("schemas.json")).unwrap());

fn schema(name: &str) -> &'static JsonValue {
    &DATA["schemas"][name]
}

fn forced() -> ToolChoice {
    ToolChoice::Tool {
        name: "strands_structured_output".into(),
    }
}

fn mechanisms(
    support: &StructuredOutputSupport,
    model_id: &str,
    name: &str,
) -> Vec<StructuredOutputMechanism> {
    support.mechanisms(
        "p",
        model_id,
        Some(&forced()),
        schema(name),
        &ModelInfo::default(),
    )
}

fn with_capabilities(capabilities: ModelCapabilities) -> ModelInfo {
    ModelInfo {
        capabilities: Some(capabilities),
        limits: None,
    }
}

mod structured_output_mechanisms {
    use super::*;

    #[test]
    fn lists_the_mechanisms_strongest_first_for_name() {
        let support = StructuredOutputSupport::default();
        let cases = [
            (Some(forced()), vec![Forced, Native, Auto]),
            (Some(ToolChoice::Any), vec![Forced, Native, Auto]),
            (Some(ToolChoice::Auto), vec![Native, Auto]),
            (None, vec![Native, Auto]),
        ];
        for (choice, expected) in cases {
            let found =
                support.mechanisms("p", "m", choice.as_ref(), schema("Person"), &ModelInfo::default());
            assert_eq!(found, expected, "{choice:?}");
        }
    }

    #[test]
    fn skips_mechanisms_the_model_info_says_are_unsupported_without_a_request() {
        let support = StructuredOutputSupport::default();
        let check = |capabilities: ModelCapabilities| {
            support.mechanisms(
                "p",
                "m",
                Some(&forced()),
                schema("Person"),
                &with_capabilities(capabilities),
            )
        };
        assert_eq!(
            check(ModelCapabilities {
                forced_tool_choice: Some(false),
                ..Default::default()
            }),
            [Native, Auto]
        );
        assert_eq!(
            check(ModelCapabilities {
                native_structured_output: Some(false),
                ..Default::default()
            }),
            [Forced, Auto]
        );
        assert_eq!(
            check(ModelCapabilities {
                forced_tool_choice: Some(true),
                native_structured_output: Some(true),
                ..Default::default()
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
            person,
            FORCED_TOOL_CHOICE_UNSUPPORTED_MESSAGES[0],
        );
        assert_eq!(mechanisms(&support, "m", "Person"), [Native, Auto]);
        support.handle_rejected_mechanism("p", "m", Native, person, NATIVE_OUTPUT_UNSUPPORTED_MESSAGES[0]);
        assert_eq!(mechanisms(&support, "m", "Person"), [Auto]);
    }

    #[test]
    fn skips_native_output_and_warns_for_a_schema_it_cannot_represent() {
        let support = StructuredOutputSupport::default();
        let (found, warnings) = WarnRecorder::capture(|| mechanisms(&support, "m", "WithRecord"));
        assert_eq!(found, [Forced, Auto]);
        assert!(warnings.iter().any(|w| w.contains("feature=<record fields>")));
    }

    #[test]
    fn does_not_inspect_the_schema_when_native_output_is_known_unsupported() {
        let support = StructuredOutputSupport::default();
        let (found, warnings) = WarnRecorder::capture(|| {
            support.handle_rejected_mechanism(
                "p",
                "m",
                Native,
                schema("WithTuple"),
                NATIVE_OUTPUT_UNSUPPORTED_MESSAGES[0],
            );
            mechanisms(&support, "m", "WithTuple")
        });
        assert_eq!(found, [Forced, Auto]);
        assert!(warnings.is_empty());
    }
}

mod handle_rejected_mechanism {
    use super::*;

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
            assert!(support.handle_rejected_mechanism("p", "m", mechanism, schema("Person"), &message));
            assert!(
                !mechanisms(&support, "m", "Team").contains(&mechanism),
                "{signal}"
            );
            assert!(mechanisms(&support, "other-model", "Person").contains(&mechanism));
            let other_provider = support.mechanisms(
                "other-provider",
                "m",
                Some(&forced()),
                schema("Person"),
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
            let (handled, warnings) = WarnRecorder::capture(|| {
                support.handle_rejected_mechanism("p", "m", Native, schema("Person"), &message)
            });
            assert!(handled);
            assert!(
                warnings
                    .iter()
                    .any(|w| w.contains("native structured output rejected the schema"))
            );
            assert_eq!(mechanisms(&support, "m", "Person"), [Forced, Auto], "{signal}");
            assert_eq!(mechanisms(&support, "m", "Team"), [Forced, Native, Auto]);
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
            assert!(!support.handle_rejected_mechanism("p", "m", mechanism, schema("Person"), message));
            assert_eq!(mechanisms(&support, "m", "Person"), [Forced, Native, Auto]);
        }
    }
}

mod find_unsupported_native_output_feature {
    use super::*;

    #[test]
    fn returns_expected_for_name() {
        let pattern = json!({"type": "object", "patternProperties": {"^a": {"type": "string"}}});
        let cases = [
            (schema("Person"), None),
            (schema("Team"), None),
            (schema("WithRecord"), Some("record fields")),
            (schema("WithNestedRecord"), Some("record fields")),
            (schema("WithCatchall"), Some("record fields")),
            (&pattern, Some("record fields")),
            (schema("WithTuple"), Some("tuple fields")),
            (schema("WithAny"), Some("untyped fields")),
            (schema("WithMixedLiteral"), Some("untyped fields")),
        ];
        for (schema, expected) in cases {
            assert_eq!(
                find_unsupported_native_output_feature(schema),
                expected,
                "{schema}"
            );
        }
    }
}

mod to_native_output_schema {
    use super::*;

    #[test]
    fn matches_the_anthropic_sdk_transform_for_name() {
        for name in ["Person", "Roster", "WithNestedRecord"] {
            let converted = to_native_output_schema(schema(name)).unwrap();
            assert_eq!(
                to_js_string(&converted).unwrap(),
                to_js_string(&DATA["anthropicTransform"][name]).unwrap(),
                "{name}"
            );
        }
    }

    #[test]
    fn keeps_enums_which_native_output_supports() {
        assert_eq!(
            to_native_output_schema(schema("Color")).unwrap(),
            json!({
                "type": "object",
                "properties": {"color": {"type": "string", "enum": ["red", "blue"]}},
                "additionalProperties": false,
                "required": ["color"]
            })
        );
    }

    #[test]
    fn keeps_defs_alongside_a_ref_and_moves_unsupported_array_keywords_into_the_description() {
        let schema = json!({"$defs": {"Item": {"type": "array", "items": false, "minItems": 3}}, "$ref": "#/$defs/Item"});
        assert_eq!(
            to_native_output_schema(&schema).unwrap(),
            json!({
                "$defs": {"Item": {"type": "array", "description": "{items: false, minItems: 3}"}},
                "$ref": "#/$defs/Item"
            })
        );
    }

    #[test]
    fn converts_oneof_to_anyof_keeps_allof_and_moves_unsupported_formats_and_item_bounds_into_the_description()
     {
        let schema = json!({
            "type": "object",
            "properties": {
                "choice": {"oneOf": [{"type": "string", "format": "phone"}, {"type": "integer"}]},
                "merged": {"allOf": [{"type": "object", "properties": {"a": {"type": "string"}}}]},
                "tags": {"type": "array", "items": {"type": "string"}, "minItems": 1}
            }
        });
        assert_eq!(
            to_native_output_schema(&schema).unwrap(),
            json!({
                "type": "object",
                "properties": {
                    "choice": {"anyOf": [{"type": "string", "description": "{format: \"phone\"}"}, {"type": "integer"}]},
                    "merged": {"allOf": [{"type": "object", "properties": {"a": {"type": "string"}}, "additionalProperties": false}]},
                    "tags": {"type": "array", "items": {"type": "string"}, "minItems": 1}
                },
                "additionalProperties": false
            })
        );
    }

    #[test]
    fn does_not_mutate_the_input_schema() {
        let copy = schema("Team").clone();
        to_native_output_schema(schema("Team")).unwrap();
        assert_eq!(schema("Team"), &copy);
    }

    #[test]
    fn throws_for_an_untyped_node() {
        let error = to_native_output_schema(schema("WithAny")).unwrap_err();
        assert!(error.to_string().contains("Schema must have a 'type'"));
    }
}

#[test]
fn mechanisms_log_as_their_ts_strings() {
    let names = STRUCTURED_OUTPUT_MECHANISMS.map(StructuredOutputMechanism::as_str);
    assert_eq!(names, ["forced", "native", "auto"]);
}
