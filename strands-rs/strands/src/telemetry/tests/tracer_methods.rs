//! Ports the per-method cases of `strands-ts/src/telemetry/__tests__/tracer.test.node.ts`: each
//! test drives `LocalTracer` directly and reads the spans the recording exporter received, where
//! TS reads a mocked span.

use super::*;
use crate::models::streaming::Metrics;
use crate::telemetry::tracer::{AgentSpanStart, LocalTracer, ModelSpanEnd};
use crate::types::messages::{Message, Role, SystemPrompt};
use crate::types::tools::ToolUse;

const LATEST: &str = "gen_ai_latest_experimental";

fn text_message(role: Role, text: &str) -> Message {
    Message::new(role, vec![TextBlock::new(text).into()])
}

fn hello() -> Vec<Message> {
    vec![text_message(Role::User, "Hello")]
}

fn agent_start(messages: &[Message]) -> AgentSpanStart<'_> {
    AgentSpanStart {
        messages,
        agent_name: "test-agent",
        agent_id: "",
        model_id: None,
        tool_names: Vec::new(),
        tools_config: None,
        system_prompt: None,
    }
}

/// Runs `f` on a tracer under the opt-in value; returns the spans in end order as
/// `{name, parent, attributes, events, status}`.
fn traced(opt_in: &'static str, f: impl FnOnce(&mut LocalTracer)) -> Vec<JsonValue> {
    let (recorder, _guard) = recording();
    let mut tracer = LocalTracer::new(semconv(opt_in));
    f(&mut tracer);
    drop(tracer);
    recorded_with_status(&recorder.spans())
}

fn events<'a>(span: &'a JsonValue, name: &str) -> Vec<&'a JsonValue> {
    span["events"]
        .as_array()
        .expect("events")
        .iter()
        .filter(|event| event["name"] == name)
        .collect()
}

/// The JSON in an event attribute.
fn event_json(event: &JsonValue, key: &str) -> JsonValue {
    serde_json::from_str(event["attributes"][key].as_str().expect("a string attribute")).expect("JSON")
}

fn tool_use_calc() -> ToolUse {
    ToolUse::new("calculator", "call-1", json!({"expr": "1+1"}))
}

fn tool_result(status: ToolResultStatus) -> ToolResultBlock {
    ToolResultBlock::new(
        "call-1",
        status,
        vec![ToolResultContent::Text(TextBlock::new("2"))],
    )
}

mod start_agent_span {
    use super::*;

    #[test]
    fn creates_span_with_correct_name_and_standard_attributes() {
        let messages = hello();
        let spans = traced("", |tracer| {
            tracer.start_agent(&AgentSpanStart {
                model_id: Some("model-123".to_owned()),
                ..agent_start(&messages)
            });
        });
        assert_eq!(spans[0]["name"], "invoke_agent test-agent");
        let attributes = &spans[0]["attributes"];
        assert_eq!(attributes["gen_ai.operation.name"], "invoke_agent");
        assert!(attributes["gen_ai.system"].is_string());
        assert_eq!(attributes["gen_ai.agent.name"], "test-agent");
        assert_eq!(attributes["gen_ai.request.model"], "model-123");
        assert_eq!(attributes["name"], "invoke_agent test-agent");
    }

    #[test]
    fn includes_agent_id_when_provided() {
        let messages = hello();
        let spans = traced("", |tracer| {
            tracer.start_agent(&AgentSpanStart {
                agent_id: "agent-42",
                ..agent_start(&messages)
            });
        });
        assert_eq!(spans[0]["attributes"]["gen_ai.agent.id"], "agent-42");
    }

    #[test]
    fn serializes_tool_names_into_gen_ai_agent_tools() {
        let messages = hello();
        let spans = traced("", |tracer| {
            tracer.start_agent(&AgentSpanStart {
                tool_names: vec!["calculator".to_owned(), "search".to_owned()],
                ..agent_start(&messages)
            });
        });
        assert_eq!(
            spans[0]["attributes"]["gen_ai.agent.tools"],
            r#"["calculator","search"]"#
        );
    }

    #[test]
    fn includes_tool_definitions_when_gen_ai_tool_definitions_opt_in_is_set() {
        let messages = hello();
        let tools_config = json!({ "calc": { "name": "calc", "description": "Calculator" } });
        let start = |tracer: &mut LocalTracer| {
            tracer.start_agent(&AgentSpanStart {
                tools_config: Some(tools_config.clone()),
                ..agent_start(&messages)
            });
        };
        let spans = traced("gen_ai_tool_definitions", start);
        assert_eq!(
            spans[0]["attributes"]["gen_ai.tool.definitions"],
            tools_config.to_string()
        );
        let spans = traced("", start);
        assert!(spans[0]["attributes"].get("gen_ai.tool.definitions").is_none());
    }

    #[test]
    fn serializes_system_prompt_into_attribute() {
        let messages = hello();
        let system_prompt = SystemPrompt::from("You are a helpful assistant");
        let spans = traced("", |tracer| {
            tracer.start_agent(&AgentSpanStart {
                system_prompt: Some(&system_prompt),
                ..agent_start(&messages)
            });
        });
        assert_eq!(
            spans[0]["attributes"]["system_prompt"],
            r#""You are a helpful assistant""#
        );
    }

    #[test]
    fn adds_separate_stable_message_events_per_message() {
        let messages = vec![
            text_message(Role::User, "Hello"),
            text_message(Role::Assistant, "Hi"),
        ];
        let spans = traced("", |tracer| tracer.start_agent(&agent_start(&messages)));
        assert_eq!(events(&spans[0], "gen_ai.user.message").len(), 1);
        assert_eq!(events(&spans[0], "gen_ai.assistant.message").len(), 1);
    }

    #[test]
    fn classifies_tool_result_messages_as_gen_ai_tool_message() {
        let result = ToolResultBlock::new(
            "tool-1",
            ToolResultStatus::Success,
            vec![ToolResultContent::Text(TextBlock::new("done"))],
        );
        let messages = vec![Message::new(Role::User, vec![ContentBlock::ToolResult(result)])];
        let spans = traced("", |tracer| tracer.start_agent(&agent_start(&messages)));
        assert_eq!(events(&spans[0], "gen_ai.tool.message").len(), 1);
    }

    #[test]
    fn adds_single_operation_details_event_with_latest_conventions() {
        let messages = vec![
            text_message(Role::User, "Hello"),
            text_message(Role::Assistant, "Hi"),
        ];
        let spans = traced(LATEST, |tracer| tracer.start_agent(&agent_start(&messages)));
        let details = events(&spans[0], "gen_ai.client.inference.operation.details");
        assert_eq!(details.len(), 1);
        assert_eq!(
            event_json(details[0], "gen_ai.input.messages"),
            json!([
                { "role": "user", "parts": [{ "type": "text", "content": "Hello" }] },
                { "role": "assistant", "parts": [{ "type": "text", "content": "Hi" }] },
            ])
        );
    }

    #[test]
    fn uses_gen_ai_provider_name_with_latest_conventions() {
        let messages = hello();
        let spans = traced(LATEST, |tracer| tracer.start_agent(&agent_start(&messages)));
        assert!(spans[0]["attributes"]["gen_ai.provider.name"].is_string());
        assert!(spans[0]["attributes"].get("gen_ai.system").is_none());
    }

    #[test]
    fn uses_gen_ai_system_with_stable_conventions() {
        let messages = hello();
        let spans = traced("", |tracer| tracer.start_agent(&agent_start(&messages)));
        assert!(spans[0]["attributes"]["gen_ai.system"].is_string());
        assert!(spans[0]["attributes"].get("gen_ai.provider.name").is_none());
    }
}

mod end_agent_span {
    use super::*;

    fn ended(
        opt_in: &'static str,
        response: Option<&Message>,
        usage: &Usage,
        error: Option<&str>,
    ) -> JsonValue {
        let messages = hello();
        let mut spans = traced(opt_in, |tracer| {
            tracer.start_agent(&agent_start(&messages));
            tracer.end_agent(response, Some("end_turn"), usage, error);
        });
        spans.remove(0)
    }

    #[test]
    fn sets_ok_status_and_ends_span_on_success() {
        let span = ended("", None, &Usage::default(), None);
        assert_eq!(span["status"], "OK");
    }

    #[test]
    fn sets_error_status_and_records_exception_on_error() {
        let span = ended("", None, &Usage::default(), Some("agent failed"));
        assert_eq!(span["status"], json!({ "error": "agent failed" }));
        let exceptions = events(&span, "exception");
        assert_eq!(exceptions.len(), 1);
        assert_eq!(exceptions[0]["attributes"]["exception.message"], "agent failed");
    }

    #[test]
    fn sets_accumulated_usage_attributes() {
        let span = ended("", None, &Usage::new(100, 50, 150), None);
        let attributes = &span["attributes"];
        assert_eq!(attributes["gen_ai.usage.prompt_tokens"], 100);
        assert_eq!(attributes["gen_ai.usage.input_tokens"], 100);
        assert_eq!(attributes["gen_ai.usage.completion_tokens"], 50);
        assert_eq!(attributes["gen_ai.usage.output_tokens"], 50);
        assert_eq!(attributes["gen_ai.usage.total_tokens"], 150);
    }

    #[test]
    fn includes_cached_tokens_in_accumulated_input_tokens_for_disjoint_cache_providers() {
        let usage = Usage::new(10, 4, 5862).with_cache_read_input_tokens(5848);
        let span = ended("", None, &usage, None);
        assert_eq!(span["attributes"]["gen_ai.usage.input_tokens"], 5858);
        assert_eq!(span["attributes"]["gen_ai.usage.prompt_tokens"], 5858);
    }

    #[test]
    fn adds_response_event_with_stable_conventions() {
        let response = text_message(Role::Assistant, "Done");
        let span = ended("", Some(&response), &Usage::default(), None);
        let choices = events(&span, "gen_ai.choice");
        assert_eq!(choices.len(), 1);
        assert_eq!(
            choices[0]["attributes"],
            json!({ "message": "Done", "finish_reason": "end_turn" })
        );
    }

    #[test]
    fn adds_response_event_with_latest_conventions() {
        let response = text_message(Role::Assistant, "Done");
        let span = ended(LATEST, Some(&response), &Usage::default(), None);
        let details = events(&span, "gen_ai.client.inference.operation.details");
        let output = details
            .iter()
            .find(|event| event["attributes"].get("gen_ai.output.messages").is_some())
            .expect("an output event");
        assert_eq!(
            event_json(output, "gen_ai.output.messages"),
            json!([{
                "role": "assistant",
                "parts": [{ "type": "text", "content": "Done" }],
                "finish_reason": "end_turn",
            }])
        );
    }

    #[test]
    fn handles_null_span_gracefully() {
        let spans = traced("", |tracer| tracer.end_agent(None, None, &Usage::default(), None));
        assert!(spans.is_empty());
    }
}

mod start_model_invoke_span {
    use super::*;

    #[test]
    fn creates_span_with_chat_operation_name_and_model_id() {
        let messages = hello();
        let spans = traced("", |tracer| {
            tracer.start_model(&messages, Some("model-123"), None);
        });
        assert_eq!(spans[0]["name"], "chat");
        assert_eq!(spans[0]["attributes"]["gen_ai.operation.name"], "chat");
        assert_eq!(spans[0]["attributes"]["gen_ai.request.model"], "model-123");
    }

    #[test]
    fn adds_message_events_to_span() {
        let messages = hello();
        let spans = traced("", |tracer| tracer.start_model(&messages, None, None));
        assert_eq!(events(&spans[0], "gen_ai.user.message").len(), 1);
    }

    #[test]
    fn includes_constructor_level_trace_attributes_overriding_standard_ones() {
        let messages = hello();
        let spans = traced("", |tracer| {
            tracer.set_trace_attributes(vec![
                ("global.attr".to_owned(), "global-val".into()),
                ("gen_ai.operation.name".to_owned(), "overridden".into()),
            ]);
            tracer.start_model(&messages, None, None);
        });
        assert_eq!(spans[0]["attributes"]["global.attr"], "global-val");
        assert_eq!(spans[0]["attributes"]["gen_ai.operation.name"], "overridden");
    }
}

mod end_model_invoke_span {
    use super::*;

    fn ended(opt_in: &'static str, end: &ModelSpanEnd<'_>) -> JsonValue {
        let messages = hello();
        let mut spans = traced(opt_in, |tracer| {
            tracer.start_model(&messages, Some("model-123"), None);
            tracer.end_model(end);
        });
        spans.remove(0)
    }

    fn with_usage(opt_in: &'static str, usage: &Usage, metrics: &Metrics) -> JsonValue {
        ended(
            opt_in,
            &ModelSpanEnd {
                usage: Some(usage),
                metrics: Some(metrics),
                ..ModelSpanEnd::default()
            },
        )
    }

    #[test]
    fn sets_usage_and_metrics_attributes() {
        let span = with_usage("", &Usage::new(100, 50, 150), &Metrics::new(500));
        let attributes = &span["attributes"];
        assert_eq!(attributes["gen_ai.usage.prompt_tokens"], 100);
        assert_eq!(attributes["gen_ai.usage.input_tokens"], 100);
        assert_eq!(attributes["gen_ai.usage.completion_tokens"], 50);
        assert_eq!(attributes["gen_ai.usage.output_tokens"], 50);
        assert_eq!(attributes["gen_ai.usage.total_tokens"], 150);
        assert_eq!(attributes["gen_ai.server.request.duration"], 500);
        assert_eq!(span["status"], "OK");
    }

    #[test]
    fn includes_cached_tokens_in_input_tokens_for_disjoint_cache_providers() {
        let usage = Usage::new(10, 4, 5862).with_cache_read_input_tokens(5848);
        let span = with_usage("", &usage, &Metrics::new(0));
        assert_eq!(span["attributes"]["gen_ai.usage.input_tokens"], 5858);
        assert_eq!(span["attributes"]["gen_ai.usage.prompt_tokens"], 5858);
    }

    #[test]
    fn does_not_double_count_when_the_provider_already_includes_cache_in_input_tokens() {
        let usage = Usage::new(12936, 10, 12946).with_cache_read_input_tokens(6457);
        let span = with_usage("", &usage, &Metrics::new(0));
        assert_eq!(span["attributes"]["gen_ai.usage.input_tokens"], 12936);
    }

    #[test]
    fn sets_cache_token_attributes_when_provided() {
        let usage = Usage::new(100, 50, 150)
            .with_cache_read_input_tokens(20)
            .with_cache_write_input_tokens(10);
        let span = with_usage("", &usage, &Metrics::new(0));
        assert_eq!(span["attributes"]["gen_ai.usage.cache_read.input_tokens"], 20);
        assert_eq!(span["attributes"]["gen_ai.usage.cache_creation.input_tokens"], 10);
    }

    #[test]
    fn skips_cache_token_attributes_when_zero() {
        let usage = Usage::new(100, 50, 150)
            .with_cache_read_input_tokens(0)
            .with_cache_write_input_tokens(0);
        let span = with_usage("", &usage, &Metrics::new(0));
        let attributes = span["attributes"].as_object().expect("attributes");
        assert!(
            !attributes.keys().any(|key| key.contains("cache")),
            "{attributes:?}"
        );
    }

    #[test]
    fn suppresses_the_deprecated_cache_aliases_when_opted_into_the_latest_conventions() {
        let usage = Usage::new(100, 50, 150)
            .with_cache_read_input_tokens(20)
            .with_cache_write_input_tokens(10);
        let span = with_usage(LATEST, &usage, &Metrics::new(0));
        let attributes = &span["attributes"];
        assert_eq!(attributes["gen_ai.usage.cache_read.input_tokens"], 20);
        assert_eq!(attributes["gen_ai.usage.cache_creation.input_tokens"], 10);
        assert!(attributes.get("gen_ai.usage.cache_read_input_tokens").is_none());
        assert!(attributes.get("gen_ai.usage.cache_write_input_tokens").is_none());
    }

    #[test]
    fn skips_latency_attribute_when_zero() {
        let span = with_usage("", &Usage::new(100, 50, 150), &Metrics::new(0));
        assert!(span["attributes"].get("gen_ai.server.request.duration").is_none());
    }

    fn mixed_output() -> Message {
        Message::new(
            Role::Assistant,
            vec![
                TextBlock::new("Let me check").into(),
                tool_use("calculator", "call-1", json!({"expr": "1+1"})),
            ],
        )
    }

    #[test]
    fn adds_output_event_with_stable_conventions_for_mixed_content() {
        let output = mixed_output();
        let span = ended(
            "",
            &ModelSpanEnd {
                output: Some(&output),
                stop_reason: Some("tool_use"),
                ..ModelSpanEnd::default()
            },
        );
        let choices = events(&span, "gen_ai.choice");
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0]["attributes"]["finish_reason"], "tool_use");
        assert_eq!(
            event_json(choices[0], "message"),
            json!([
                { "text": "Let me check" },
                { "type": "toolUse", "name": "calculator", "toolUseId": "call-1", "input": { "expr": "1+1" } },
            ])
        );
    }

    #[test]
    fn adds_output_event_with_latest_conventions_for_mixed_content() {
        let output = mixed_output();
        let span = ended(
            LATEST,
            &ModelSpanEnd {
                output: Some(&output),
                stop_reason: Some("tool_use"),
                ..ModelSpanEnd::default()
            },
        );
        let details = events(&span, "gen_ai.client.inference.operation.details");
        let output = details
            .iter()
            .find(|event| event["attributes"].get("gen_ai.output.messages").is_some())
            .expect("an output event");
        assert_eq!(
            event_json(output, "gen_ai.output.messages"),
            json!([{
                "role": "assistant",
                "parts": [
                    { "type": "text", "content": "Let me check" },
                    { "type": "tool_call", "name": "calculator", "id": "call-1", "arguments": { "expr": "1+1" } },
                ],
                "finish_reason": "tool_use",
            }])
        );
    }

    #[test]
    fn records_error_on_model_invocation_failure() {
        let span = ended("", &ModelSpanEnd::failed(&"model failed"));
        assert_eq!(span["status"], json!({ "error": "model failed" }));
        assert_eq!(
            events(&span, "exception")[0]["attributes"]["exception.message"],
            "model failed"
        );
    }

    #[test]
    fn handles_null_span_gracefully() {
        let spans = traced("", |tracer| tracer.end_model(&ModelSpanEnd::default()));
        assert!(spans.is_empty());
    }

    /// TS `timeToFirstByteMs` section: TTFB is a histogram metric, not a span attribute.
    #[test]
    fn does_not_set_ttfb_as_span_attribute_ttfb_is_a_histogram_metric_not_a_span_attribute() {
        let metrics = Metrics::new(500).with_time_to_first_byte_ms(150);
        let span = with_usage("", &Usage::new(100, 50, 150), &metrics);
        let attributes = span["attributes"].as_object().expect("attributes");
        assert!(
            !attributes.keys().any(|key| key.contains("first")),
            "{attributes:?}"
        );
    }
}

mod start_tool_call_span {
    use super::*;

    fn started(opt_in: &'static str) -> JsonValue {
        let mut spans = traced(opt_in, |tracer| {
            tracer.set_trace_attributes(vec![("global.attr".to_owned(), "global-val".into())]);
            drop(tracer.start_tool(&tool_use_calc()));
        });
        spans.remove(0)
    }

    #[test]
    fn creates_span_with_tool_name_and_call_id() {
        let span = started("");
        assert_eq!(span["name"], "execute_tool calculator");
        assert_eq!(span["attributes"]["gen_ai.operation.name"], "execute_tool");
        assert_eq!(span["attributes"]["gen_ai.tool.name"], "calculator");
        assert_eq!(span["attributes"]["gen_ai.tool.call.id"], "call-1");
    }

    #[test]
    fn includes_constructor_level_trace_attributes() {
        assert_eq!(started("")["attributes"]["global.attr"], "global-val");
    }

    #[test]
    fn adds_stable_tool_message_event_with_serialized_input() {
        let span = started("");
        let messages = events(&span, "gen_ai.tool.message");
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0]["attributes"],
            json!({ "role": "tool", "content": r#"{"expr":"1+1"}"#, "id": "call-1" })
        );
    }

    #[test]
    fn adds_latest_convention_tool_input_event() {
        let span = started(LATEST);
        let details = events(&span, "gen_ai.client.inference.operation.details");
        assert_eq!(details.len(), 1);
        assert_eq!(
            event_json(details[0], "gen_ai.input.messages"),
            json!([{
                "role": "tool",
                "parts": [{ "type": "tool_call", "name": "calculator", "id": "call-1", "arguments": { "expr": "1+1" } }],
            }])
        );
    }

    #[test]
    fn sets_latest_convention_tool_call_arguments_attribute() {
        assert_eq!(
            started(LATEST)["attributes"]["gen_ai.tool.call.arguments"],
            r#"{"expr":"1+1"}"#
        );
        assert!(
            started("")["attributes"]
                .get("gen_ai.tool.call.arguments")
                .is_none()
        );
    }
}

mod end_tool_call_span {
    use super::*;

    fn ended(opt_in: &'static str, result: Option<&ToolResultBlock>) -> JsonValue {
        let mut spans = traced(opt_in, |tracer| {
            let span = tracer.start_tool(&tool_use_calc());
            tracer.end_tool("call-1", &span, result);
        });
        spans.remove(0)
    }

    #[test]
    fn sets_tool_status_attribute_and_adds_stable_result_event() {
        let span = ended("", Some(&tool_result(ToolResultStatus::Success)));
        assert_eq!(span["attributes"]["gen_ai.tool.status"], "success");
        let choices = events(&span, "gen_ai.choice");
        assert_eq!(choices.len(), 1);
        assert_eq!(
            choices[0]["attributes"],
            json!({ "message": r#"[{"text":"2"}]"#, "id": "call-1" })
        );
        assert_eq!(span["status"], "OK");
    }

    #[test]
    fn adds_latest_convention_tool_result_event() {
        let span = ended(LATEST, Some(&tool_result(ToolResultStatus::Success)));
        let details = events(&span, "gen_ai.client.inference.operation.details");
        let output = details
            .iter()
            .find(|event| event["attributes"].get("gen_ai.output.messages").is_some())
            .expect("an output event");
        assert_eq!(
            event_json(output, "gen_ai.output.messages"),
            json!([{
                "role": "tool",
                "parts": [{ "type": "tool_call_response", "id": "call-1", "response": [{ "text": "2" }] }],
            }])
        );
    }

    #[test]
    fn sets_latest_convention_tool_call_result_attribute_on_success() {
        let span = ended(LATEST, Some(&tool_result(ToolResultStatus::Success)));
        assert_eq!(span["attributes"]["gen_ai.tool.call.result"], r#"[{"text":"2"}]"#);
    }

    #[test]
    fn omits_latest_convention_tool_call_result_attribute_on_error_status() {
        let span = ended(LATEST, Some(&tool_result(ToolResultStatus::Error)));
        assert_eq!(span["attributes"]["gen_ai.tool.status"], "error");
        assert!(span["attributes"].get("gen_ai.tool.call.result").is_none());
    }

    #[test]
    fn records_error_on_tool_failure() {
        let result = tool_result(ToolResultStatus::Error).with_error("tool failed");
        let span = ended("", Some(&result));
        assert_eq!(span["status"], json!({ "error": "tool failed" }));
        assert_eq!(
            events(&span, "exception")[0]["attributes"]["exception.message"],
            "tool failed"
        );
    }

    #[test]
    fn handles_null_span_gracefully() {
        let spans = traced("", |tracer| {
            tracer.end_tool(
                "call-1",
                &tracing::Span::none(),
                Some(&tool_result(ToolResultStatus::Success)),
            );
        });
        assert!(spans.is_empty());
    }
}

mod start_agent_loop_span {
    use super::*;

    #[test]
    fn creates_span_with_cycle_id_attribute() {
        let spans = traced("", |tracer| tracer.start_cycle("cycle-1", &[]));
        assert_eq!(spans[0]["name"], "execute_agent_loop_cycle");
        assert_eq!(spans[0]["attributes"]["agent_loop.cycle_id"], "cycle-1");
    }

    #[test]
    fn includes_constructor_level_trace_attributes() {
        let spans = traced("", |tracer| {
            tracer.set_trace_attributes(vec![("global.attr".to_owned(), "global-val".into())]);
            tracer.start_cycle("cycle-1", &[]);
        });
        assert_eq!(spans[0]["attributes"]["global.attr"], "global-val");
    }

    #[test]
    fn adds_message_events_to_loop_span() {
        let messages = hello();
        let spans = traced("", |tracer| tracer.start_cycle("cycle-1", &messages));
        assert_eq!(events(&spans[0], "gen_ai.user.message").len(), 1);
    }

    #[test]
    fn creates_local_trace_with_cycle_id_in_metadata() {
        let mut tracer = LocalTracer::new(semconv(""));
        tracer.start_cycle("cycle-1", &[]);
        assert_eq!(tracer.traces().len(), 1);
        assert_eq!(tracer.traces()[0].name(), "Cycle 1");
        assert_eq!(tracer.traces()[0].metadata_value("cycleId"), Some("cycle-1"));
    }

    #[test]
    fn stores_unique_cycle_ids_for_multiple_cycles() {
        let mut tracer = LocalTracer::new(semconv(""));
        tracer.start_cycle("cycle-1", &[]);
        tracer.end_cycle(None);
        tracer.start_cycle("cycle-2", &[]);
        tracer.end_cycle(None);
        let ids: Vec<_> = tracer
            .traces()
            .iter()
            .map(|trace| trace.metadata_value("cycleId"))
            .collect();
        assert_eq!(ids, [Some("cycle-1"), Some("cycle-2")]);
    }
}

mod end_agent_loop_span {
    use super::*;

    fn ended(error: Option<&str>) -> JsonValue {
        let mut spans = traced("", |tracer| {
            tracer.start_cycle("cycle-1", &[]);
            tracer.end_cycle(error);
        });
        spans.remove(0)
    }

    #[test]
    fn ends_span_with_ok_status() {
        assert_eq!(ended(None)["status"], "OK");
    }

    #[test]
    fn records_error_on_loop_failure() {
        let span = ended(Some("loop failed"));
        assert_eq!(span["status"], json!({ "error": "loop failed" }));
        assert_eq!(
            events(&span, "exception")[0]["attributes"]["exception.message"],
            "loop failed"
        );
    }

    #[test]
    fn handles_null_span_gracefully() {
        let spans = traced("", |tracer| tracer.end_cycle(None));
        assert!(spans.is_empty());
    }
}

mod system_prompt_on_chat_spans {
    use super::*;

    fn chat(opt_in: &'static str, system_prompt: Option<&SystemPrompt>) -> JsonValue {
        let messages = hello();
        let mut spans = traced(opt_in, |tracer| {
            tracer.start_model(&messages, None, system_prompt);
        });
        spans.remove(0)
    }

    #[test]
    fn emits_gen_ai_system_message_event_with_stable_conventions() {
        let system_prompt = SystemPrompt::from("You are helpful");
        let span = chat("", Some(&system_prompt));
        let system = events(&span, "gen_ai.system.message");
        assert_eq!(system.len(), 1);
        assert_eq!(
            event_json(system[0], "content"),
            json!([{ "text": "You are helpful" }])
        );
    }

    #[test]
    fn emits_gen_ai_system_instructions_with_latest_conventions() {
        let system_prompt = SystemPrompt::from("You are helpful");
        let span = chat(LATEST, Some(&system_prompt));
        let details = events(&span, "gen_ai.client.inference.operation.details");
        let instructions = details
            .iter()
            .find(|event| event["attributes"].get("gen_ai.system_instructions").is_some())
            .expect("a system instructions event");
        assert_eq!(
            event_json(instructions, "gen_ai.system_instructions"),
            json!([{ "type": "text", "content": "You are helpful" }])
        );
    }

    #[test]
    fn does_not_emit_system_prompt_event_when_system_prompt_is_undefined() {
        let span = chat("", None);
        assert!(events(&span, "gen_ai.system.message").is_empty());
    }
}

mod span_attributes_only {
    use super::*;

    fn agent_span(opt_in: &'static str) -> JsonValue {
        let messages = hello();
        let mut spans = traced(opt_in, |tracer| tracer.start_agent(&agent_start(&messages)));
        spans.remove(0)
    }

    #[test]
    fn records_latest_convention_content_as_span_attributes_and_skips_the_event_when_opted_in() {
        let span = agent_span("gen_ai_latest_experimental,gen_ai_span_attributes_only");
        assert!(span["attributes"]["gen_ai.input.messages"].is_string());
        assert!(events(&span, "gen_ai.client.inference.operation.details").is_empty());
    }

    #[test]
    fn emits_span_events_by_default_without_the_opt_in_or_langfuse() {
        let span = agent_span(LATEST);
        assert!(span["attributes"].get("gen_ai.input.messages").is_none());
        assert_eq!(
            events(&span, "gen_ai.client.inference.operation.details").len(),
            1
        );
    }

    #[test]
    fn leaves_legacy_per_message_events_as_events_even_when_opted_in() {
        let span = agent_span("gen_ai_span_attributes_only");
        assert!(span["attributes"].get("gen_ai.input.messages").is_none());
        assert_eq!(events(&span, "gen_ai.user.message").len(), 1);
    }
}

mod message_event_formatting {
    use super::*;

    fn first_input_part(content: Vec<ContentBlock>, role: Role) -> JsonValue {
        let messages = vec![Message::new(role, content)];
        let spans = traced(LATEST, |tracer| tracer.start_agent(&agent_start(&messages)));
        let details = events(&spans[0], "gen_ai.client.inference.operation.details");
        event_json(details[0], "gen_ai.input.messages")[0]["parts"][0].clone()
    }

    #[test]
    fn maps_tool_use_blocks_to_tool_call_parts_in_latest_conventions() {
        let part = first_input_part(
            vec![tool_use("search", "tu-1", json!({"q": "test"}))],
            Role::Assistant,
        );
        assert_eq!(
            part,
            json!({ "type": "tool_call", "name": "search", "id": "tu-1", "arguments": { "q": "test" } })
        );
    }

    #[test]
    fn maps_tool_result_blocks_to_tool_call_response_parts_in_latest_conventions() {
        let result = ToolResultBlock::new(
            "tu-1",
            ToolResultStatus::Success,
            vec![ToolResultContent::Text(TextBlock::new("result"))],
        );
        let part = first_input_part(vec![ContentBlock::ToolResult(result)], Role::User);
        assert_eq!(part["type"], "tool_call_response");
        assert_eq!(part["id"], "tu-1");
    }

    #[test]
    fn serializes_text_block_content_in_stable_convention_events() {
        let messages = vec![text_message(Role::User, "Hello world")];
        let spans = traced("", |tracer| tracer.start_model(&messages, None, None));
        let user = events(&spans[0], "gen_ai.user.message");
        assert_eq!(event_json(user[0], "content")[0]["text"], "Hello world");
    }
}

mod error_resilience {
    use super::*;

    #[test]
    fn does_not_throw_when_ending_null_spans_with_errors() {
        let spans = traced("", |tracer| {
            tracer.end_agent(None, None, &Usage::default(), Some("test"));
            tracer.end_model(&ModelSpanEnd::failed(&"test"));
            let result = tool_result(ToolResultStatus::Error).with_error("test");
            tracer.end_tool("call-1", &tracing::Span::none(), Some(&result));
            tracer.end_cycle(Some("test"));
        });
        assert!(spans.is_empty());
    }
}

mod memory_spans {
    use opentelemetry::trace::{SpanContext, TraceContextExt as _};
    use tracing_opentelemetry::OpenTelemetrySpanExt as _;

    use super::*;
    use crate::memory::MemoryEntry;
    use crate::telemetry::tracer::memory::{ExtractSpanStart, MemoryTracer};

    /// Runs `f` on a memory tracer under the opt-in value; returns the spans in end order.
    fn traced(opt_in: &'static str, f: impl FnOnce(&MemoryTracer)) -> Vec<JsonValue> {
        let (recorder, _guard) = recording();
        f(&MemoryTracer::new(semconv(opt_in)));
        recorded_with_status(&recorder.spans())
    }

    fn stores(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    fn extract_start(agent_span_context: Option<SpanContext>) -> ExtractSpanStart<'static> {
        ExtractSpanStart {
            store_name: "personal",
            message_count: 3,
            filtered_count: 1,
            agent_span_context,
        }
    }

    #[test]
    fn start_memory_search_span_sets_attributes_and_records_the_query_as_an_event() {
        let spans = traced("", |tracer| {
            drop(tracer.start_search("dark mode preference", &stores(&["personal", "team"]), Some(5)));
        });
        assert_eq!(spans[0]["name"], "memory.search");
        let attributes = &spans[0]["attributes"];
        assert_eq!(attributes["gen_ai.operation.name"], "memory.search");
        assert_eq!(attributes["memory.store.names"], r#"["personal","team"]"#);
        assert_eq!(attributes["memory.store.count"], 2);
        assert_eq!(attributes["memory.max_search_results"], 5);
        let query = events(&spans[0], "memory.query");
        assert_eq!(query[0]["attributes"]["content"], "dark mode preference");
    }

    #[test]
    fn end_memory_search_span_records_result_failure_counts_and_the_entries_event() {
        let mut metadata = serde_json::Map::new();
        metadata.insert("score".to_owned(), json!(0.9));
        let entry = MemoryEntry::new("likes dark mode")
            .with_store_name("personal")
            .with_metadata(metadata);
        let spans = traced("", |tracer| {
            let span = tracer.start_search("q", &stores(&["personal"]), None);
            tracer.end_search(&span, &[entry], 1, None);
        });
        assert_eq!(spans[0]["attributes"]["memory.result.count"], 1);
        assert_eq!(spans[0]["attributes"]["memory.store.failure_count"], 1);
        let results = events(&spans[0], "memory.results");
        assert_eq!(results.len(), 1);
        assert_eq!(
            event_json(results[0], "content"),
            json!([{ "content": "likes dark mode", "storeName": "personal", "metadata": { "score": 0.9 } }])
        );
        assert_eq!(spans[0]["status"], "OK");
    }

    #[test]
    fn end_memory_search_span_records_error_and_skips_the_results_event() {
        let spans = traced("", |tracer| {
            let span = tracer.start_search("q", &stores(&["personal"]), None);
            tracer.end_search(&span, &[], 0, Some("store not found"));
        });
        assert_eq!(spans[0]["status"], json!({ "error": "store not found" }));
        assert!(events(&spans[0], "memory.results").is_empty());
    }

    #[test]
    fn start_memory_add_span_records_content_as_an_event() {
        let spans = traced("", |tracer| {
            drop(tracer.start_add("remember dark mode", &stores(&["personal"]), false));
        });
        assert_eq!(spans[0]["name"], "memory.add");
        assert_eq!(spans[0]["attributes"]["gen_ai.operation.name"], "memory.add");
        assert_eq!(spans[0]["attributes"]["memory.store.count"], 1);
        let content = events(&spans[0], "memory.content");
        assert_eq!(content[0]["attributes"]["content"], "remember dark mode");
    }

    #[test]
    fn end_memory_add_span_records_the_store_failure_count_and_error() {
        let spans = traced("", |tracer| {
            let span = tracer.start_add("c", &stores(&["personal"]), false);
            tracer.end_add(&span, 2, Some("writes failed"));
        });
        assert_eq!(spans[0]["attributes"]["memory.store.failure_count"], 2);
        assert_eq!(spans[0]["status"], json!({ "error": "writes failed" }));
    }

    #[test]
    fn memory_inject_span_start_sets_max_entries_end_records_injected_and_entry_count() {
        let spans = traced("", |tracer| {
            let span = tracer.start_inject(Some(5));
            tracer.end_inject(&span, true, 3, false);
        });
        assert_eq!(spans[0]["name"], "memory.inject");
        let attributes = &spans[0]["attributes"];
        assert_eq!(attributes["memory.max_entries"], 5);
        assert_eq!(attributes["memory.injected"], true);
        assert_eq!(attributes["memory.entry.count"], 3);
        assert!(attributes.get("memory.inject.format_error").is_none());
    }

    #[test]
    fn end_memory_inject_span_fails_open_on_a_format_error_ok_status_flag_set() {
        let spans = traced("", |tracer| {
            let span = tracer.start_inject(None);
            tracer.end_inject(&span, false, 0, true);
        });
        assert_eq!(spans[0]["attributes"]["memory.injected"], false);
        assert_eq!(spans[0]["attributes"]["memory.inject.format_error"], true);
        assert_eq!(spans[0]["status"], "OK");
    }

    #[test]
    fn start_memory_extract_span_is_a_root_span_force_root_no_parent_context() {
        let spans = traced("", |tracer| {
            let caller = tracing::info_span!("caller");
            caller.in_scope(|| drop(tracer.start_extract(extract_start(None))));
        });
        let extract = &spans[0];
        assert_eq!(extract["name"], "memory.extract");
        assert_eq!(extract["parent"], JsonValue::Null);
        assert_eq!(extract["attributes"]["memory.store.name"], "personal");
        assert_eq!(extract["attributes"]["memory.message.count"], 3);
        assert_eq!(extract["attributes"]["memory.message.filtered_count"], 1);
    }

    #[test]
    fn start_memory_extract_span_attaches_a_valid_agent_span_context_as_a_link() {
        let (recorder, _guard) = recording();
        let tracer = MemoryTracer::new(semconv(""));
        let agent = tracing::info_span!("agent");
        let agent_context = agent.context().span().span_context().clone();
        assert!(agent_context.is_valid());
        drop(tracer.start_extract(extract_start(Some(agent_context.clone()))));
        drop(agent);

        let spans = recorder.spans();
        let extract = spans.iter().find(|span| span.name == "memory.extract").unwrap();
        assert_eq!(extract.links.len(), 1);
        assert_eq!(extract.links[0].span_context, agent_context);
        let attributes = attributes_json(&extract.attributes);
        assert_eq!(
            attributes["memory.parent.trace_id"],
            agent_context.trace_id().to_string()
        );
        assert_eq!(
            attributes["memory.parent.span_id"],
            agent_context.span_id().to_string()
        );
    }

    #[test]
    fn start_memory_extract_span_skips_the_link_and_parent_ids_for_an_invalid_agent_span_context() {
        let (recorder, _guard) = recording();
        let tracer = MemoryTracer::new(semconv(""));
        drop(tracer.start_extract(extract_start(Some(SpanContext::empty_context()))));

        let spans = recorder.spans();
        assert!(spans[0].links.is_empty());
        let attributes = attributes_json(&spans[0].attributes);
        assert!(attributes.get("memory.parent.trace_id").is_none());
        assert!(attributes.get("memory.parent.span_id").is_none());
    }

    #[test]
    fn end_memory_extract_span_records_the_entry_count_on_success() {
        let spans = traced("", |tracer| {
            let span = tracer.start_extract(extract_start(None));
            tracer.end_extract(&span, Some(4), None);
        });
        assert_eq!(spans[0]["attributes"]["memory.entry.count"], 4);
        assert_eq!(spans[0]["status"], "OK");
    }

    #[test]
    fn end_memory_extract_span_records_error_failures_are_swallowed_by_the_coordinator() {
        let spans = traced("", |tracer| {
            let span = tracer.start_extract(extract_start(None));
            tracer.end_extract(&span, None, Some("backend down"));
        });
        assert_eq!(spans[0]["status"], json!({ "error": "backend down" }));
        assert!(spans[0]["attributes"].get("memory.entry.count").is_none());
    }

    #[test]
    fn memory_span_methods_handle_a_null_span_gracefully() {
        let spans = traced("", |tracer| {
            let none = tracing::Span::none();
            tracer.end_search(&none, &[], 0, None);
            tracer.end_add(&none, 0, None);
            tracer.end_inject(&none, false, 0, false);
            tracer.end_extract(&none, None, None);
        });
        assert!(spans.is_empty());
    }

    #[test]
    fn records_the_memory_search_query_as_a_span_attribute_and_skips_the_event_when_opted_in() {
        let spans = traced("gen_ai_span_attributes_only", |tracer| {
            drop(tracer.start_search("dark mode preference", &stores(&["personal"]), None));
        });
        assert_eq!(spans[0]["attributes"]["content"], "dark mode preference");
        assert!(events(&spans[0], "memory.query").is_empty());
    }

    #[test]
    fn records_the_memory_add_content_as_a_span_attribute_and_skips_the_event_when_opted_in() {
        let spans = traced("gen_ai_span_attributes_only", |tracer| {
            drop(tracer.start_add("remember dark mode", &stores(&["personal"]), false));
        });
        assert_eq!(spans[0]["attributes"]["content"], "remember dark mode");
        assert!(events(&spans[0], "memory.content").is_empty());
    }
}
