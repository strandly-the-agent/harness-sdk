//! Live telemetry test against Bedrock (TS checklist:
//! strands-ts/test/integ/telemetry.test.node.ts): the spans of a real invocation, read through
//! a recording span exporter. Covered: the span hierarchy, span attributes, custom trace
//! attributes, status and usage. Not covered here: the `getTracer`/`register()` cases and the
//! `Metrics Integration` cases, whose Rust API (`setup_tracer`, the meter) is not in this tree
//! yet, and the concurrent-agents cases.
//!
//! The subscriber is thread-local (`tracing::subscriber::set_default`), so the tests run on the
//! current-thread runtime and share no global state. They use the default AWS credential chain
//! and skip with a message when no credentials resolve. Run with
//! `cargo test -p strands --all-features -- --ignored live_`.
#![cfg(all(feature = "otel", feature = "bedrock"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test code"
)]

mod live_support;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use live_support::{MODEL_ID, bedrock, bedrock_with, credentials_available, tool_succeeded, trace};
use opentelemetry::Value;
use opentelemetry::trace::{SpanId, Status, TracerProvider as _};
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::trace::{SdkTracerProvider, SpanData, SpanExporter};
use strands::telemetry::AttributeValue;
use strands::{Agent, StopReason, ToolError, tool};
use tracing_subscriber::layer::SubscriberExt as _;

/// The name of the loop-cycle span (TS `startAgentLoopSpan`).
const CYCLE: &str = "execute_agent_loop_cycle";

#[derive(Debug, Clone, Default)]
struct Recorder(Arc<Mutex<Vec<SpanData>>>);

impl SpanExporter for Recorder {
    async fn export(&self, batch: Vec<SpanData>) -> OTelSdkResult {
        self.0.lock().unwrap().extend(batch);
        Ok(())
    }
}

/// A thread-local subscriber exporting every span to the returned recorder.
fn recording() -> (Recorder, tracing::subscriber::DefaultGuard) {
    let recorder = Recorder::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(recorder.clone())
        .build();
    let layer = tracing_opentelemetry::layer().with_tracer(provider.tracer("strands-agents"));
    let guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(layer));
    (recorder, guard)
}

/// The SDK's spans (`invoke_agent ..`, the loop cycle, `chat`, `execute_tool ..`), without the
/// spans of the HTTP and AWS clients.
fn sdk_spans(recorder: &Recorder) -> Vec<SpanData> {
    recorder
        .0
        .lock()
        .unwrap()
        .iter()
        .filter(|span| {
            span.name.starts_with("invoke_agent ")
                || span.name.starts_with("execute_tool ")
                || span.name == "chat"
                || span.name == CYCLE
        })
        .cloned()
        .collect()
}

fn attributes(span: &SpanData) -> BTreeMap<String, Value> {
    span.attributes
        .iter()
        .map(|pair| (pair.key.as_str().to_owned(), pair.value.clone()))
        .collect()
}

fn int(span: &SpanData, key: &str) -> i64 {
    match attributes(span).get(key) {
        Some(Value::I64(value)) => *value,
        other => panic!("{}: `{key}` is {other:?}", span.name),
    }
}

fn text(span: &SpanData, key: &str) -> String {
    attributes(span)
        .get(key)
        .map(|value| value.as_str().into_owned())
        .unwrap_or_else(|| panic!("{}: no `{key}`", span.name))
}

fn named<'a>(spans: &'a [SpanData], prefix: &str) -> Vec<&'a SpanData> {
    spans
        .iter()
        .filter(|span| span.name.starts_with(prefix))
        .collect()
}

fn parent_of<'a>(spans: &'a [SpanData], span: &SpanData) -> Option<&'a SpanData> {
    spans
        .iter()
        .find(|candidate| candidate.span_context.span_id() == span.parent_span_id)
}

/// Returns the current time.
#[tool]
async fn time_tool() -> Result<String, ToolError> {
    Ok("12:00".into())
}

/// TS cases: `creates agent → cycle → model spans for a simple invocation`, `creates tool spans
/// nested under cycle spans`, `creates separate cycle spans for each loop iteration`, `sets
/// agent span attributes correctly`, `sets model span attributes correctly`, `sets tool span
/// attributes correctly`, `merges constructor-level trace attributes onto agent span`, `sets OK
/// status on all spans for successful invocations`, `records accumulated usage on agent span
/// across multiple cycles`, `records per-call usage on individual model spans`.
#[tokio::test]
#[ignore = "live bedrock"]
async fn live_telemetry_records_the_span_hierarchy_attributes_and_usage_of_a_tool_invocation() {
    if !credentials_available().await {
        return;
    }
    let (recorder, _guard) = recording();
    let mut agent = Agent::builder()
        .model(bedrock(300))
        .name("telemetry_agent")
        .system_prompt("Use the time_tool to answer time questions.")
        .tools([TimeTool])
        .trace_attributes(
            [(
                "deployment.tier".to_owned(),
                AttributeValue::String("live-test".into()),
            )]
            .into_iter()
            .collect(),
        )
        .printer(false)
        .build()
        .unwrap();
    let result = agent.invoke("What time is it?").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert!(tool_succeeded(&trace(&agent), "time_tool"));
    let usage = *result.metrics().accumulated_usage();
    agent.shutdown().await.unwrap();

    let spans = sdk_spans(&recorder);
    let names: Vec<String> = spans.iter().map(|span| span.name.to_string()).collect();

    // One agent span, at the root of the SDK's spans, with the custom attribute.
    let [agent_span] = named(&spans, "invoke_agent ")[..] else {
        panic!("one agent span: {names:?}")
    };
    assert_eq!(agent_span.name, "invoke_agent telemetry_agent");
    assert!(parent_of(&spans, agent_span).is_none());
    assert_eq!(text(agent_span, "gen_ai.agent.name"), "telemetry_agent");
    assert_eq!(text(agent_span, "gen_ai.request.model"), MODEL_ID);
    assert_eq!(text(agent_span, "deployment.tier"), "live-test");

    // One cycle per loop iteration (the tool call, then the answer), each under the agent span.
    let cycles = named(&spans, CYCLE);
    assert_eq!(cycles.len(), 2, "{names:?}");
    for cycle in &cycles {
        assert_eq!(cycle.parent_span_id, agent_span.span_context.span_id());
    }

    // One model span per cycle, under its cycle, with that call's usage.
    let chats = named(&spans, "chat");
    assert_eq!(chats.len(), 2, "{names:?}");
    let mut input_tokens = 0;
    let mut output_tokens = 0;
    for chat in &chats {
        let parent = parent_of(&spans, chat).expect("a model span has a parent");
        assert_eq!(parent.name, CYCLE);
        assert_eq!(text(chat, "gen_ai.request.model"), MODEL_ID);
        assert!(int(chat, "gen_ai.usage.input_tokens") > 0);
        assert!(int(chat, "gen_ai.usage.output_tokens") > 0);
        input_tokens += int(chat, "gen_ai.usage.input_tokens");
        output_tokens += int(chat, "gen_ai.usage.output_tokens");
    }
    let cycle_ids: Vec<SpanId> = chats.iter().map(|chat| chat.parent_span_id).collect();
    assert_ne!(cycle_ids[0], cycle_ids[1]);

    // The agent span carries the usage accumulated over both calls.
    assert_eq!(int(agent_span, "gen_ai.usage.input_tokens"), input_tokens);
    assert_eq!(int(agent_span, "gen_ai.usage.output_tokens"), output_tokens);
    assert_eq!(u64::try_from(input_tokens).unwrap(), usage.input_tokens());
    assert_eq!(u64::try_from(output_tokens).unwrap(), usage.output_tokens());

    // The tool span sits under a cycle span.
    let [tool_span] = named(&spans, "execute_tool ")[..] else {
        panic!("one tool span: {names:?}")
    };
    assert_eq!(tool_span.name, "execute_tool time_tool");
    assert_eq!(parent_of(&spans, tool_span).unwrap().name, CYCLE);
    assert_eq!(text(tool_span, "gen_ai.tool.name"), "time_tool");
    assert!(!text(tool_span, "gen_ai.tool.call.id").is_empty());

    for span in &spans {
        assert!(
            matches!(span.status, Status::Ok),
            "{}: {:?}",
            span.name,
            span.status
        );
        assert!(span.end_time >= span.start_time);
    }
}

/// TS cases: `records error status on agent span when model throws`, `records error status and
/// exception event on model span when model throws`.
#[tokio::test]
#[ignore = "live bedrock"]
async fn live_telemetry_records_error_status_when_the_model_call_fails() {
    if !credentials_available().await {
        return;
    }
    let (recorder, _guard) = recording();
    let mut agent = Agent::builder()
        .model(bedrock_with("us.invalid.model-that-does-not-exist-v1:0", 100))
        .name("telemetry_agent")
        .printer(false)
        .build()
        .unwrap();
    agent.invoke("Hello").await.unwrap_err();
    agent.shutdown().await.unwrap();

    let spans = sdk_spans(&recorder);
    let names: Vec<String> = spans.iter().map(|span| span.name.to_string()).collect();
    let [agent_span] = named(&spans, "invoke_agent ")[..] else {
        panic!("one agent span: {names:?}")
    };
    assert!(
        matches!(agent_span.status, Status::Error { .. }),
        "{:?}",
        agent_span.status
    );
    let chat = named(&spans, "chat")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("a model span: {names:?}"));
    assert!(matches!(chat.status, Status::Error { .. }), "{:?}", chat.status);
    assert!(
        chat.events.iter().any(|event| event.name == "exception"),
        "{:?}",
        chat.events
    );
}
