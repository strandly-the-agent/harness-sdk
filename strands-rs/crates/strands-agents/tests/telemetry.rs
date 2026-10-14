//! Span hierarchy, `gen_ai.*` attributes and metrics through the public API
//! (`telemetry/tracer.test.node.ts`, `telemetry/meter.test.ts`).

use std::sync::Arc;

use opentelemetry::Value;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SpanData};
use serde_json::json;
use strands_agents::test_support::{MockMessageModel, MockResponse};
use strands_agents::tools::{function_tool, ToolRef};
use strands_agents::types::{ContentBlock, ToolUseBlock, Usage};
use strands_agents::{Agent, Message};

/// The tracer provider is process-global, so tests that emit spans must not overlap.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn attr<'a>(span: &'a SpanData, key: &str) -> Option<&'a Value> {
    span.attributes
        .iter()
        .find(|kv| kv.key.as_str() == key)
        .map(|kv| &kv.value)
}

fn string_attr(span: &SpanData, key: &str) -> String {
    attr(span, key).map(|v| v.to_string()).unwrap_or_default()
}

fn find<'a>(spans: &'a [SpanData], name: &str) -> &'a SpanData {
    spans.iter().find(|s| s.name == name).unwrap_or_else(|| {
        panic!(
            "no span named {name} in {:?}",
            spans.iter().map(|s| &s.name).collect::<Vec<_>>()
        )
    })
}

fn echo_tool() -> ToolRef {
    Arc::new(
        function_tool("echo")
            .description("Echoes its input")
            .input_schema(json!({ "type": "object", "properties": { "value": { "type": "string" } } }))
            .handler(|input, _ctx| async move { Ok(json!({ "echoed": input })) }),
    )
}

/// One test owns the global provider: it is process-wide, so all span assertions share a run.
#[tokio::test]
async fn agent_loop_emits_the_gen_ai_span_tree_with_attributes_and_metrics() {
    let _serial = SERIAL.lock().await;
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    opentelemetry::global::set_tracer_provider(provider.clone());

    let tool_use = ToolUseBlock {
        name: "echo".into(),
        tool_use_id: "call-1".into(),
        input: json!({ "value": "hi" }),
        reasoning_signature: None,
    };
    let model = MockMessageModel::shared(vec![
        MockResponse::with_usage(
            Message::new(
                strands_agents::types::Role::Assistant,
                vec![ContentBlock::ToolUse(tool_use)],
            ),
            Usage {
                input_tokens: 12,
                output_tokens: 3,
                total_tokens: 15,
                cache_read_input_tokens: Some(4),
                cache_write_input_tokens: None,
            },
        ),
        MockResponse::with_usage(
            Message::assistant("done"),
            Usage {
                input_tokens: 20,
                output_tokens: 5,
                total_tokens: 25,
                ..Usage::default()
            },
        ),
    ]);

    let agent = Agent::builder()
        .name("my-agent")
        .model_arc(model)
        .system_prompt("be brief")
        .tool_ref(echo_tool())
        .trace_attributes([("deployment".to_string(), "test".into())])
        .build()
        .await
        .expect("agent builds");

    let result = agent.invoke("hello").await.expect("invocation succeeds");
    provider.force_flush().ok();
    let spans = exporter.get_finished_spans().expect("spans exported");

    // Span inventory and names.
    let agent_span = find(&spans, "invoke_agent my-agent");
    let cycle = find(&spans, "execute_agent_loop_cycle");
    let chat = find(&spans, "chat");
    let tool = find(&spans, "execute_tool echo");

    // Hierarchy: agent → cycle → { chat, execute_tool }.
    assert_eq!(agent_span.parent_span_id, opentelemetry::trace::SpanId::INVALID);
    assert_eq!(cycle.parent_span_id, agent_span.span_context.span_id());
    assert_eq!(chat.parent_span_id, cycle.span_context.span_id());
    assert_eq!(tool.parent_span_id, cycle.span_context.span_id());

    // Agent span attributes.
    assert_eq!(string_attr(agent_span, "gen_ai.operation.name"), "invoke_agent");
    assert_eq!(string_attr(agent_span, "gen_ai.agent.name"), "my-agent");
    assert_eq!(string_attr(agent_span, "name"), "invoke_agent my-agent");
    assert!(string_attr(agent_span, "gen_ai.agent.tools").contains("echo"));
    assert_eq!(string_attr(agent_span, "system_prompt"), "\"be brief\"");
    assert!(attr(agent_span, "gen_ai.event.start_time").is_some());
    assert!(attr(agent_span, "gen_ai.event.end_time").is_some());
    // Custom trace attributes are merged onto every span.
    assert_eq!(string_attr(agent_span, "deployment"), "test");
    assert_eq!(string_attr(chat, "deployment"), "test");

    // Cycle span carries its id.
    assert!(!string_attr(cycle, "agent_loop.cycle_id").is_empty());

    // Model span usage attributes (stable conventions keep the deprecated cache alias).
    let chat_spans: Vec<&SpanData> = spans.iter().filter(|s| s.name == "chat").collect();
    assert_eq!(chat_spans.len(), 2, "one chat span per cycle");
    let first_chat = chat_spans[0];
    assert_eq!(string_attr(first_chat, "gen_ai.operation.name"), "chat");
    assert_eq!(
        attr(first_chat, "gen_ai.usage.prompt_tokens"),
        Some(&Value::I64(12))
    );
    assert_eq!(
        attr(first_chat, "gen_ai.usage.input_tokens"),
        Some(&Value::I64(12))
    );
    assert_eq!(
        attr(first_chat, "gen_ai.usage.completion_tokens"),
        Some(&Value::I64(3))
    );
    assert_eq!(
        attr(first_chat, "gen_ai.usage.output_tokens"),
        Some(&Value::I64(3))
    );
    assert_eq!(
        attr(first_chat, "gen_ai.usage.total_tokens"),
        Some(&Value::I64(15))
    );
    assert_eq!(
        attr(first_chat, "gen_ai.usage.cache_read.input_tokens"),
        Some(&Value::I64(4))
    );
    assert_eq!(
        attr(first_chat, "gen_ai.usage.cache_read_input_tokens"),
        Some(&Value::I64(4))
    );
    assert!(attr(first_chat, "gen_ai.usage.cache_creation.input_tokens").is_none());

    // Tool span attributes.
    assert_eq!(string_attr(tool, "gen_ai.operation.name"), "execute_tool");
    assert_eq!(string_attr(tool, "gen_ai.tool.name"), "echo");
    assert_eq!(string_attr(tool, "gen_ai.tool.call.id"), "call-1");
    assert_eq!(string_attr(tool, "gen_ai.tool.status"), "success");
    // Opt-in only (`gen_ai_tool_definitions`).
    assert!(attr(tool, "gen_ai.tool.call.arguments").is_none());
    assert!(attr(tool, "gen_ai.tool.call.result").is_none());

    // Events: input messages on the agent/model spans, choice on completion.
    let event_names: Vec<&str> = agent_span.events.iter().map(|e| e.name.as_ref()).collect();
    assert!(event_names.contains(&"gen_ai.system.message"), "{event_names:?}");
    assert!(event_names.contains(&"gen_ai.user.message"), "{event_names:?}");
    assert!(event_names.contains(&"gen_ai.choice"), "{event_names:?}");
    let tool_events: Vec<&str> = tool.events.iter().map(|e| e.name.as_ref()).collect();
    assert!(tool_events.contains(&"gen_ai.choice"), "{tool_events:?}");
    let second_chat_events: Vec<&str> = chat_spans[1].events.iter().map(|e| e.name.as_ref()).collect();
    assert!(
        second_chat_events.contains(&"gen_ai.tool.message"),
        "tool results are recorded as gen_ai.tool.message: {second_chat_events:?}"
    );

    // Statuses.
    assert_eq!(agent_span.status, opentelemetry::trace::Status::Ok);
    assert_eq!(tool.status, opentelemetry::trace::Status::Ok);

    // AgentResult telemetry.
    let metrics = &result.metrics;
    assert_eq!(metrics.cycle_count, 2);
    assert_eq!(metrics.accumulated_usage.input_tokens, 32);
    assert_eq!(metrics.accumulated_usage.output_tokens, 8);
    assert_eq!(metrics.accumulated_usage.total_tokens, 40);
    assert_eq!(result.context_size(), Some(20));
    assert_eq!(result.projected_context_size(), Some(25));
    let tool_metrics = &metrics.tool_metrics["echo"];
    assert_eq!((tool_metrics.call_count, tool_metrics.success_count), (1, 1));
    assert_eq!(tool_metrics.error_count, 0);
    assert_eq!(metrics.agent_invocations.len(), 1);
    assert_eq!(metrics.agent_invocations[0].cycles.len(), 2);

    // Local traces mirror the span tree.
    assert_eq!(result.traces.len(), 1);
    let root = &result.traces[0];
    assert_eq!(root.name, "invoke_agent my-agent");
    assert_eq!(root.children.len(), 2, "one cycle per model call");
    let child_names: Vec<&str> = root.children[0]
        .children
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert!(child_names.contains(&"Model invoke"), "{child_names:?}");
    assert!(child_names.contains(&"Tool: echo"), "{child_names:?}");
    assert!(root.end_time.is_some());
}

/// Error paths: a failing model records the exception and an ERROR status on the model span.
#[tokio::test]
async fn model_errors_are_recorded_on_the_model_span() {
    let _serial = SERIAL.lock().await;
    let tracer = strands_agents::telemetry::Tracer::new(strands_agents::telemetry::TelemetryConfig::new());
    let agent_span = tracer.start_agent_span(Default::default());
    let cycle = tracer.start_agent_loop_span(&agent_span, "cycle-1");
    let model_span = tracer.start_model_invoke_span(&cycle, Default::default());
    tracer.end_model_invoke_span(&model_span, None, None, None, None, Some("boom"));
    tracer.end_agent_loop_span(&cycle, None);
    tracer.end_agent_span(&agent_span, None, None);

    let traces = tracer.local_traces();
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].children[0].children[0].name, "Model invoke");
    assert!(traces[0].children[0].children[0].end_time.is_some());
}

/// `end_*` tolerates a span that was never started (TS "handles null span gracefully").
#[tokio::test]
async fn handles_unstarted_spans_gracefully() {
    let _serial = SERIAL.lock().await;
    let tracer = strands_agents::telemetry::Tracer::new(strands_agents::telemetry::TelemetryConfig::new());
    let span = strands_agents::telemetry::SpanHandle::default();
    tracer.end_agent_span(&span, None, None);
    tracer.end_agent_loop_span(&span, Some("boom"));
    tracer.end_model_invoke_span(&span, None, None, None, None, None);
    tracer.end_tool_call_span(&span, None, None);
}

/// Spans carry no telemetry when no provider is installed, but metrics still accumulate.
#[tokio::test]
async fn metrics_are_collected_without_a_provider() {
    let _serial = SERIAL.lock().await;
    let model = MockMessageModel::shared(vec![MockResponse::with_usage(
        Message::assistant("hi"),
        Usage {
            input_tokens: 7,
            output_tokens: 2,
            total_tokens: 9,
            ..Usage::default()
        },
    )]);
    let agent = Agent::builder()
        .model_arc(model)
        .build()
        .await
        .expect("agent builds");
    let result = agent.invoke("hello").await.expect("invocation succeeds");
    assert_eq!(result.metrics.cycle_count, 1);
    assert_eq!(result.metrics.accumulated_usage.total_tokens, 9);
    assert_eq!(result.context_size(), Some(7));
    assert!(result.metrics.total_duration.is_some());
}
