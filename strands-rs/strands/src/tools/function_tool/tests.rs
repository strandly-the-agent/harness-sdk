//! Ported from strands-ts/src/tools/__tests__/tool.test.ts (the `FunctionTool` cases) and
//! strands-ts/src/tools/__tests__/zod-tool.test.ts (mapped to `FunctionTool::typed`).
//!
//! Gaps: async generator callbacks (TS-only mechanism; streaming tools implement `Tool`
//! directly), deep-copy and circular-reference cases (Rust values are owned and acyclic),
//! non-`Error` thrown values and stack traces (no Rust equivalent).

use std::sync::Arc;

use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;

use super::*;
use crate::interrupt::{Interrupt, InterruptParams};
use crate::testing::MockMessageModel;
use crate::tools::tool::ToolStreamItem;
use crate::types::agent::{InvocationState, LocalAgent};
use crate::types::tools::ToolUse;

fn context(input: JsonValue) -> ToolContext {
    let agent = LocalAgent::new(Arc::new(MockMessageModel::new()));
    ToolContext::new(
        ToolUse::new("test", "tool-1", input),
        agent.tool_agent(),
        InvocationState::new(),
    )
}

async fn run(tool: &FunctionTool, input: JsonValue) -> ToolOutcome {
    let items: Vec<ToolStreamItem> = tool.stream(context(input)).collect().await;
    assert_eq!(items.len(), 1);
    match items.into_iter().next() {
        Some(ToolStreamItem::Outcome(outcome)) => outcome,
        other => panic!("expected an outcome, got {other:?}"),
    }
}

async fn result_json(tool: &FunctionTool, input: JsonValue) -> JsonValue {
    match run(tool, input).await {
        ToolOutcome::Result(result) => serde_json::to_value(result).unwrap(),
        ToolOutcome::Interrupt(interrupt) => panic!("unexpected interrupt {interrupt:?}"),
    }
}

fn returning(value: JsonValue) -> FunctionTool {
    FunctionTool::new("test", "Test tool.", move |_, _| {
        let value = value.clone();
        async move { Ok::<_, ToolError>(value) }
    })
}

#[expect(clippy::needless_pass_by_value, reason = "call sites pass `json!` literals")]
fn success(content: JsonValue) -> JsonValue {
    json!({"toolUseId": "tool-1", "status": "success", "content": content})
}

mod properties {
    use super::*;

    #[test]
    fn has_a_valid_tool_spec() {
        let tool = returning(json!(null));
        assert_eq!(tool.name(), "test");
        assert_eq!(tool.description(), "Test tool.");
        assert_eq!(tool.tool_spec().name(), tool.name());
        assert_eq!(tool.tool_spec().description(), tool.description());
    }

    #[test]
    fn creates_tool_with_default_empty_object_schema() {
        let tool = FunctionTool::new("flaky_tool", "Always fails.", |_, _| async {
            Err::<(), _>(ToolError::msg("no"))
        });
        let expected: JsonValue = serde_json::from_str(include_str!(
            "../../../../conformance/fixtures/tools/function_tool_default_schema.json"
        ))
        .unwrap();
        assert_eq!(serde_json::to_value(tool.tool_spec()).unwrap(), expected);
    }

    #[test]
    fn a_raw_schema_is_sent_unchanged() {
        let schema = json!({"type": "object", "properties": {"city": {"type": "string", "description": "City name"}}, "required": ["city"], "additionalProperties": false});
        let tool = FunctionTool::new("get_weather", "Get the weather for a city.", |_, _| async {
            Ok::<_, ToolError>("x")
        })
        .with_input_schema(schema);
        let expected: JsonValue = serde_json::from_str(include_str!(
            "../../../../conformance/fixtures/tools/function_tool_spec.json"
        ))
        .unwrap();
        assert_eq!(serde_json::to_value(tool.tool_spec()).unwrap(), expected);
    }
}

mod stream_method {
    use super::*;

    #[tokio::test]
    async fn handles_string_return_values() {
        assert_eq!(
            result_json(&returning(json!("hi")), json!({})).await,
            success(json!([{"text": "hi"}]))
        );
    }

    #[tokio::test]
    async fn handles_object_return_values() {
        assert_eq!(
            result_json(&returning(json!({"a": 1, "b": 2})), json!({})).await,
            success(json!([{"json": {"a": 1, "b": 2}}]))
        );
    }

    #[tokio::test]
    async fn treats_objects_with_extra_keys_beyond_a_content_block_key_as_json() {
        assert_eq!(
            result_json(&returning(json!({"text": "x", "extra": 1})), json!({})).await,
            success(json!([{"json": {"text": "x", "extra": 1}}]))
        );
    }

    #[tokio::test]
    async fn single_key_content_blocks_pass_through() {
        assert_eq!(
            result_json(&returning(json!({"text": "x"})), json!({})).await,
            success(json!([{"text": "x"}]))
        );
    }

    #[tokio::test]
    async fn passes_input_to_callback_exactly_as_provided_to_stream() {
        let tool = FunctionTool::new("test", "Test tool.", |input, _| async move {
            Ok::<_, ToolError>(json!({ "echo": input }))
        });
        assert_eq!(
            result_json(&tool, json!({"n": [1, 2]})).await,
            success(json!([{"json": {"echo": {"n": [1, 2]}}}]))
        );
    }

    #[tokio::test]
    async fn handles_null_return_values_correctly() {
        assert_eq!(
            result_json(&returning(json!(null)), json!({})).await,
            success(json!([{"text": "null"}]))
        );
    }

    #[tokio::test]
    async fn handles_undefined_return_values_correctly() {
        let tool = FunctionTool::new("test", "Test tool.", |_, _| async { Ok::<_, ToolError>(()) });
        assert_eq!(
            result_json(&tool, json!({})).await,
            success(json!([{"text": "null"}]))
        );
    }

    #[tokio::test]
    async fn handles_boolean_return_values_as_text_content() {
        assert_eq!(
            result_json(&returning(json!(false)), json!({})).await,
            success(json!([{"text": "false"}]))
        );
    }

    #[tokio::test]
    async fn handles_number_return_values_as_text_content() {
        assert_eq!(
            result_json(&returning(json!(42)), json!({})).await,
            success(json!([{"text": "42"}]))
        );
        assert_eq!(
            result_json(&returning(json!(1.5)), json!({})).await,
            success(json!([{"text": "1.5"}]))
        );
        assert_eq!(
            result_json(&returning(json!(8.0)), json!({})).await,
            success(json!([{"text": "8"}]))
        );
        assert_eq!(
            result_json(&returning(json!(1e21)), json!({})).await,
            success(json!([{"text": "1e+21"}]))
        );
    }

    #[tokio::test]
    async fn handles_array_return_values_as_wrapped_json_content() {
        assert_eq!(
            result_json(&returning(json!([1, "a"])), json!({})).await,
            success(json!([{"json": {"$value": [1, "a"]}}]))
        );
        assert_eq!(
            result_json(&returning(json!([])), json!({})).await,
            success(json!([{"json": {"$value": []}}]))
        );
    }

    #[tokio::test]
    async fn arrays_of_content_blocks_become_the_content_array() {
        assert_eq!(
            result_json(&returning(json!([{"text": "a"}, {"json": {"b": 1}}])), json!({})).await,
            success(json!([{"text": "a"}, {"json": {"b": 1}}]))
        );
    }

    #[tokio::test]
    async fn can_access_tool_context_in_promise() {
        let tool = FunctionTool::new("test", "Test tool.", |_, ctx: ToolContext| async move {
            Ok::<_, ToolError>(ctx.tool_use().tool_use_id().to_owned())
        });
        assert_eq!(
            result_json(&tool, json!({})).await,
            success(json!([{"text": "tool-1"}]))
        );
    }
}

mod error_handling {
    use super::*;

    #[tokio::test]
    async fn catches_promise_rejections() {
        let tool = FunctionTool::new("test", "Test tool.", |_, _| async {
            Err::<(), _>(ToolError::msg("boom"))
        });
        let ToolOutcome::Result(result) = run(&tool, json!({})).await else {
            panic!("expected a result");
        };
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            json!({"toolUseId": "tool-1", "status": "error", "content": [{"text": "Error: boom"}]})
        );
        assert_eq!(result.error().map(ToString::to_string).as_deref(), Some("boom"));
    }

    #[tokio::test]
    async fn preserves_custom_error_subclass_instances() {
        let tool = FunctionTool::new("test", "Test tool.", |_, _| async {
            Err::<(), _>(std::io::Error::other("disk full").into())
        });
        let ToolOutcome::Result(result) = run(&tool, json!({})).await else {
            panic!("expected a result");
        };
        let error = result.error().unwrap();
        assert!(error.downcast_ref::<std::io::Error>().is_some());
    }

    #[tokio::test]
    async fn an_interrupt_raised_through_the_context_ends_with_an_interrupt_outcome() {
        let tool = FunctionTool::new("test", "Test tool.", |_, ctx: ToolContext| async move {
            let answer = ctx.interrupt(InterruptParams::new("confirm"))?;
            Ok::<_, ToolError>(answer)
        });
        let ToolOutcome::Interrupt(interrupt) = run(&tool, json!({})).await else {
            panic!("expected an interrupt");
        };
        assert_eq!(interrupt.id(), "tool:tool-1:confirm");
        assert_eq!(interrupt.name(), "confirm");
    }

    #[test]
    fn tool_error_converts_errors_and_interrupts() {
        let failed = ToolError::from(std::io::Error::other("x"));
        assert!(failed.interrupt().is_none());
        let interrupted = ToolError::from(Interrupt::new("id", "name"));
        assert_eq!(interrupted.interrupt().map(Interrupt::id), Some("id"));
    }
}

mod optional_input_schema {
    use super::*;

    #[tokio::test]
    async fn executes_successfully_with_empty_input() {
        assert_eq!(
            result_json(&returning(json!({"status": "operational"})), json!({})).await,
            success(json!([{"json": {"status": "operational"}}]))
        );
    }

    #[tokio::test]
    async fn callback_receives_empty_object_when_no_schema_provided() {
        let tool = FunctionTool::new("test", "Test tool.", |input, _| async move {
            Ok::<_, ToolError>(json!({ "received": input }))
        });
        assert_eq!(
            result_json(&tool, json!({})).await,
            success(json!([{"json": {"received": {}}}]))
        );
    }
}

mod typed {
    use super::*;

    #[derive(Debug, Deserialize, schemars::JsonSchema)]
    struct WeatherInput {
        /// City name
        city: String,
        /// Days ahead
        days: Option<u8>,
    }

    fn weather_tool() -> FunctionTool {
        FunctionTool::typed(
            "get_weather",
            "Get the weather for a city.",
            |input: WeatherInput, _ctx| async move {
                Ok::<_, ToolError>(format!("{} in {} days", input.city, input.days.unwrap_or(0)))
            },
        )
    }

    #[test]
    fn generates_the_input_schema_from_the_type() {
        assert_eq!(
            weather_tool().tool_spec().input_schema(),
            Some(&json!({
                "type": "object",
                "properties": {
                    "city": {"description": "City name", "type": "string"},
                    "days": {"description": "Days ahead", "type": "integer", "minimum": 0, "maximum": 255}
                },
                "required": ["city"],
                "additionalProperties": false
            }))
        );
    }

    #[tokio::test]
    async fn validates_and_parses_input_before_the_callback() {
        assert_eq!(
            result_json(&weather_tool(), json!({"city": "Paris", "days": 2})).await,
            success(json!([{"text": "Paris in 2 days"}]))
        );
    }

    #[tokio::test]
    async fn invalid_input_becomes_an_error_result() {
        let ToolOutcome::Result(result) = run(&weather_tool(), json!({"days": 2})).await else {
            panic!("expected a result");
        };
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            json!({"toolUseId": "tool-1", "status": "error", "content": [{"text": "Error: Invalid input: \"city\" is a required property"}]})
        );
    }

    #[tokio::test]
    async fn a_type_mismatch_names_the_path() {
        let ToolOutcome::Result(result) = run(&weather_tool(), json!({"city": 3})).await else {
            panic!("expected a result");
        };
        let text = serde_json::to_value(&result).unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(text.starts_with("Error: Invalid input at /city: "), "{text}");
    }

    #[tokio::test]
    async fn an_undeclared_argument_is_dropped_and_the_tool_runs() {
        assert_eq!(
            result_json(
                &weather_tool(),
                json!({"city": "Paris", "days": 2, "units": "metric"})
            )
            .await,
            success(json!([{"text": "Paris in 2 days"}]))
        );
    }

    #[tokio::test]
    async fn an_undeclared_argument_does_not_hide_an_invalid_declared_one() {
        let ToolOutcome::Result(result) = run(&weather_tool(), json!({"city": 3, "units": "metric"})).await
        else {
            panic!("expected a result");
        };
        let text = serde_json::to_value(&result).unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(text.starts_with("Error: Invalid input at /city: "), "{text}");
    }

    #[tokio::test]
    async fn an_undeclared_key_inside_a_tuple_item_is_dropped_and_the_tool_runs() {
        #[derive(Debug, Deserialize, schemars::JsonSchema)]
        struct Point {
            label: String,
        }
        #[derive(Debug, Deserialize, schemars::JsonSchema)]
        struct Input {
            pair: (Point, u32),
        }
        let tool = FunctionTool::typed("pair", "A tuple field.", |input: Input, _ctx| async move {
            Ok::<_, ToolError>(format!("{} {}", input.pair.0.label, input.pair.1))
        });
        assert_eq!(
            result_json(&tool, json!({"pair": [{"label": "x", "extra": 1}, 2]})).await,
            success(json!([{"text": "x 2"}]))
        );
        let ToolOutcome::Result(result) = run(&tool, json!({"pair": [{"label": 3, "extra": 1}, 2]})).await
        else {
            panic!("expected a result");
        };
        let text = serde_json::to_value(&result).unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(
            text.starts_with("Error: Invalid input at /pair/0/label: "),
            "{text}"
        );
    }

    #[tokio::test]
    async fn a_type_that_denies_unknown_fields_still_rejects_an_undeclared_argument() {
        #[derive(Debug, Deserialize, schemars::JsonSchema)]
        #[serde(deny_unknown_fields)]
        struct Strict {
            city: String,
        }
        let tool = FunctionTool::typed("strict", "Strict input.", |input: Strict, _ctx| async move {
            Ok::<_, ToolError>(input.city)
        });
        let ToolOutcome::Result(result) = run(&tool, json!({"city": "Paris", "units": "metric"})).await
        else {
            panic!("expected a result");
        };
        assert_eq!(
            serde_json::to_value(&result).unwrap()["content"],
            json!([{"text": "Error: Invalid input: Additional properties are not allowed ('units' was unexpected)"}])
        );
        assert_eq!(
            result_json(&tool, json!({"city": "Paris"})).await,
            success(json!([{"text": "Paris"}]))
        );
    }

    #[tokio::test]
    async fn invoke_returns_the_json_value() {
        let value = weather_tool()
            .invoke(json!({"city": "Oslo"}), context(json!({})))
            .await
            .unwrap();
        assert_eq!(value, json!("Oslo in 0 days"));
    }
}

mod input_validator {
    use super::*;

    fn nested_schema(additional: &JsonValue) -> JsonValue {
        json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "address": {
                    "type": "object",
                    "properties": {"city": {"type": "string"}},
                    "required": ["city"],
                    "additionalProperties": false
                },
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {"id": {"type": "integer"}},
                        "required": ["id"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["name"],
            "additionalProperties": additional
        })
    }

    #[test]
    fn undeclared_keys_are_dropped_at_every_level_before_validation() {
        let parsed: JsonValue = InputValidator::new(&nested_schema(&json!(false)))
            .parse(json!({
                "name": "a",
                "extra": 1,
                "address": {"city": "Oslo", "zip": "0150"},
                "items": [{"id": 1, "note": "x"}, {"id": 2}]
            }))
            .unwrap();
        assert_eq!(
            parsed,
            json!({"name": "a", "address": {"city": "Oslo"}, "items": [{"id": 1}, {"id": 2}]})
        );
    }

    #[test]
    fn undeclared_keys_are_dropped_from_tuple_items_by_position() {
        let closed = |key: &str| {
            json!({
                "type": "object",
                "properties": {key: {"type": "integer"}},
                "additionalProperties": false
            })
        };
        let schema = json!({
            "type": "object",
            "properties": {
                "pair": {"type": "array", "prefixItems": [closed("a"), closed("b")], "items": closed("c")}
            },
            "additionalProperties": false
        });
        let parsed: JsonValue = InputValidator::new(&schema)
            .parse(json!({"pair": [{"a": 1, "b": 0}, {"b": 2, "c": 0}, {"c": 3, "a": 0}, {"c": 4, "z": 0}]}))
            .unwrap();
        assert_eq!(parsed, json!({"pair": [{"a": 1}, {"b": 2}, {"c": 3}, {"c": 4}]}));
    }

    #[test]
    fn a_schema_that_allows_additional_properties_keeps_them() {
        for additional in [json!(true), json!({})] {
            let input = json!({"name": "a", "extra": 1});
            let parsed: JsonValue = InputValidator::new(&nested_schema(&additional))
                .parse(input.clone())
                .unwrap();
            assert_eq!(parsed, input);
        }
        let open = json!({"type": "object", "properties": {"name": {"type": "string"}}});
        let input = json!({"name": "a", "extra": 1});
        let parsed: JsonValue = InputValidator::new(&open).parse(input.clone()).unwrap();
        assert_eq!(parsed, input);
    }

    #[test]
    fn dropping_keys_does_not_relax_the_declared_ones() {
        let error = InputValidator::new(&nested_schema(&json!(false)))
            .parse::<JsonValue>(json!({"name": "a", "extra": 1, "address": {"zip": "0150"}}))
            .unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<ToolValidationError>()
                .map(ToString::to_string)
                .as_deref(),
            Some("Invalid input at /address: \"city\" is a required property")
        );
    }
}
