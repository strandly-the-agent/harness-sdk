//! Multi-agent and node spans emitted by `Graph` and `Swarm`
//! (`multiagent/__tests__/graph.tracer.test.ts`, `swarm.tracer.test.ts`).
//!
//! TS spies on its `Tracer`; Rust asserts on the exported spans instead, so every `it` becomes
//! an assertion over span names, `gen_ai.*` attributes and parent links.

#![cfg(feature = "telemetry")]

use opentelemetry::Value;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SpanData};
use strands_agents::hooks::Cancel;
use strands_agents::multiagent::events::BeforeNodeCallEvent;
use strands_agents::multiagent::{Graph, ResultStatus, Swarm};
use strands_agents::telemetry::AttrValue;
use strands_agents::test_support::{MockMessageModel, MockResponse};
use strands_agents::types::{ContentBlock, InterruptParams, Role, StopReason, ToolUseBlock, Usage};
use strands_agents::{Agent, Message};

/// The tracer provider is process-global, so tests that emit spans must not overlap.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Installs a fresh in-memory exporter as the global provider; spans are read back after flush.
fn exporter() -> (InMemorySpanExporter, SdkTracerProvider) {
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    opentelemetry::global::set_tracer_provider(provider.clone());
    (exporter, provider)
}

fn finished(exporter: &InMemorySpanExporter, provider: &SdkTracerProvider) -> Vec<SpanData> {
    let _ = provider.force_flush();
    exporter.get_finished_spans().expect("spans are exported")
}

fn attr<'a>(span: &'a SpanData, key: &str) -> Option<&'a Value> {
    span.attributes
        .iter()
        .find(|kv| kv.key.as_str() == key)
        .map(|kv| &kv.value)
}

fn string_attr(span: &SpanData, key: &str) -> String {
    attr(span, key).map(ToString::to_string).unwrap_or_default()
}

fn int_attr(span: &SpanData, key: &str) -> i64 {
    match attr(span, key) {
        Some(Value::I64(value)) => *value,
        other => panic!("attribute {key} is not an integer: {other:?}"),
    }
}

fn find<'a>(spans: &'a [SpanData], name: &str) -> &'a SpanData {
    spans.iter().find(|span| span.name == name).unwrap_or_else(|| {
        panic!(
            "no span named {name} in {:?}",
            spans.iter().map(|span| &span.name).collect::<Vec<_>>()
        )
    })
}

fn node_spans(spans: &[SpanData]) -> Vec<&SpanData> {
    spans
        .iter()
        .filter(|span| span.name.starts_with("node "))
        .collect()
}

fn agent(id: &str) -> Agent {
    Agent::builder()
        .id(id)
        .name(id)
        .model(MockMessageModel::shared(vec![MockResponse::message(
            Message::assistant("reply"),
        )]))
        .build()
        .expect("agent builds")
}

fn agent_with_usage(id: &str) -> Agent {
    Agent::builder()
        .id(id)
        .model(MockMessageModel::shared(vec![MockResponse::with_usage(
            Message::assistant("reply"),
            Usage {
                input_tokens: 10,
                output_tokens: 5,
                total_tokens: 15,
                ..Usage::default()
            },
        )]))
        .build()
        .expect("agent builds")
}

/// A swarm agent's reply: one `strands_structured_output` call ending the swarm.
fn swarm_agent(id: &str, usage: Option<Usage>) -> Agent {
    Agent::builder()
        .id(id)
        .name(id)
        .description(format!("Agent {id}"))
        .model(MockMessageModel::shared(vec![MockResponse::Message {
            message: Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse(ToolUseBlock::new(
                    "strands_structured_output",
                    "tool-1",
                    serde_json::json!({ "message": "done" }),
                ))],
            ),
            stop_reason: Some(StopReason::ToolUse),
            usage,
        }]))
        .build()
        .expect("agent builds")
}

fn failing_agent(id: &str) -> Agent {
    Agent::builder()
        .id(id)
        .model(MockMessageModel::shared(vec![MockResponse::Error(
            strands_agents::Error::other("agent exploded"),
        )]))
        .build()
        .expect("agent builds")
}

fn is_error(span: &SpanData) -> bool {
    matches!(span.status, opentelemetry::trace::Status::Error { .. })
}

// ---- Graph ----

#[tokio::test]
async fn graph_starts_and_ends_the_multi_agent_span_on_a_successful_invocation() {
    let _serial = SERIAL.lock().await;
    let (exporter, provider) = exporter();

    let graph = Graph::builder()
        .id("test-graph")
        .node(agent("a"))
        .build()
        .expect("graph builds");
    graph.invoke("Hello").await.expect("graph runs");

    let spans = finished(&exporter, &provider);
    let span = find(&spans, "invoke_graph test-graph");
    assert_eq!(string_attr(span, "gen_ai.operation.name"), "invoke_graph");
    assert_eq!(string_attr(span, "gen_ai.agent.name"), "graph");
    assert_eq!(string_attr(span, "gen_ai.agent.id"), "test-graph");
    assert_eq!(string_attr(span, "name"), "invoke_graph test-graph");
    assert_eq!(string_attr(span, "gen_ai.agent.input"), "\"Hello\"");
    assert!(int_attr(span, "gen_ai.agent.execution_time") >= 0);
    assert_eq!(int_attr(span, "gen_ai.usage.total_tokens"), 0);
    assert!(!is_error(span));
}

#[tokio::test]
async fn graph_records_the_exact_result_usage_on_the_multi_agent_span() {
    let _serial = SERIAL.lock().await;
    let (exporter, provider) = exporter();

    let graph = Graph::builder()
        .id("usage-graph")
        .node(agent_with_usage("a"))
        .build()
        .expect("graph builds");
    graph.invoke("Hello").await.expect("graph runs");

    let spans = finished(&exporter, &provider);
    let span = find(&spans, "invoke_graph usage-graph");
    assert_eq!(int_attr(span, "gen_ai.usage.input_tokens"), 10);
    assert_eq!(int_attr(span, "gen_ai.usage.output_tokens"), 5);
    assert_eq!(int_attr(span, "gen_ai.usage.total_tokens"), 15);
}

#[tokio::test]
async fn graph_ends_the_multi_agent_span_with_an_error_when_max_steps_is_exceeded() {
    let _serial = SERIAL.lock().await;
    let (exporter, provider) = exporter();

    let graph = Graph::builder()
        .id("capped-graph")
        .node(agent("a"))
        .node(agent("b"))
        .edge("a", "b")
        .max_steps(1)
        .build()
        .expect("graph builds");
    let error = graph.invoke("Hello").await.expect_err("max steps is reached");
    assert!(error.to_string().contains("max steps reached"), "{error}");

    let spans = finished(&exporter, &provider);
    let span = find(&spans, "invoke_graph capped-graph");
    assert!(is_error(span));
    assert!(int_attr(span, "gen_ai.agent.execution_time") >= 0);
}

#[tokio::test]
async fn graph_starts_and_ends_a_node_span_for_each_node_execution() {
    let _serial = SERIAL.lock().await;
    let (exporter, provider) = exporter();

    let graph = Graph::builder()
        .node(agent("a"))
        .node(agent("b"))
        .edge("a", "b")
        .build()
        .expect("graph builds");
    graph.invoke("Hello").await.expect("graph runs");

    let spans = finished(&exporter, &provider);
    let nodes = node_spans(&spans);
    assert_eq!(
        nodes.iter().map(|span| span.name.as_ref()).collect::<Vec<_>>(),
        vec!["node a", "node b"]
    );
    for span in nodes {
        assert_eq!(string_attr(span, "gen_ai.operation.name"), "execute_node");
        assert_eq!(string_attr(span, "gen_ai.agent.node_type"), "agentNode");
    }
}

#[tokio::test]
async fn graph_ends_a_node_span_with_completed_status_duration_and_usage() {
    let _serial = SERIAL.lock().await;
    let (exporter, provider) = exporter();

    let graph = Graph::builder()
        .node(agent_with_usage("a"))
        .build()
        .expect("graph builds");
    graph.invoke("Hello").await.expect("graph runs");

    let spans = finished(&exporter, &provider);
    let span = find(&spans, "node a");
    assert_eq!(string_attr(span, "gen_ai.agent.id"), "a");
    assert_eq!(string_attr(span, "gen_ai.agent.status"), "COMPLETED");
    assert!(int_attr(span, "gen_ai.agent.execution_time") >= 0);
    assert_eq!(int_attr(span, "gen_ai.usage.input_tokens"), 10);
    assert_eq!(int_attr(span, "gen_ai.usage.output_tokens"), 5);
    assert_eq!(int_attr(span, "gen_ai.usage.total_tokens"), 15);
}

#[tokio::test]
async fn graph_ends_a_node_span_with_failed_status_when_the_node_agent_errors() {
    let _serial = SERIAL.lock().await;
    let (exporter, provider) = exporter();

    let graph = Graph::builder()
        .node(failing_agent("a"))
        .build()
        .expect("graph builds");
    let result = graph.invoke("Hello").await.expect("graph runs");
    assert_eq!(result.status, ResultStatus::Failed);

    let spans = finished(&exporter, &provider);
    let span = find(&spans, "node a");
    assert_eq!(string_attr(span, "gen_ai.agent.status"), "FAILED");
    assert!(int_attr(span, "gen_ai.agent.execution_time") >= 0);
}

#[tokio::test]
async fn graph_ends_a_node_span_with_cancelled_status_and_zero_duration_when_a_hook_cancels() {
    let _serial = SERIAL.lock().await;
    let (exporter, provider) = exporter();

    let graph = Graph::builder()
        .node(agent("a"))
        .hook(|event: &mut BeforeNodeCallEvent| {
            event.cancel = Cancel::Reason("cancelled by test".to_owned());
            async move { Ok(()) }
        })
        .build()
        .expect("graph builds");
    graph.invoke("Hello").await.expect("graph runs");

    let spans = finished(&exporter, &provider);
    let span = find(&spans, "node a");
    assert_eq!(string_attr(span, "gen_ai.agent.status"), "CANCELLED");
    assert_eq!(int_attr(span, "gen_ai.agent.execution_time"), 0);
}

#[tokio::test]
async fn graph_ends_a_node_span_with_interrupted_status_when_a_hook_interrupts() {
    let _serial = SERIAL.lock().await;
    let (exporter, provider) = exporter();

    let graph = Graph::builder()
        .node(agent("a"))
        .hook(|event: &mut BeforeNodeCallEvent| {
            let event = event.clone();
            async move {
                let _: String = event.interrupt(InterruptParams::new("gate"))?;
                Ok(())
            }
        })
        .build()
        .expect("graph builds");
    let result = graph.invoke("Hello").await.expect("graph runs");
    assert_eq!(result.status, ResultStatus::Interrupted);

    let spans = finished(&exporter, &provider);
    let span = find(&spans, "node a");
    assert_eq!(string_attr(span, "gen_ai.agent.status"), "INTERRUPTED");
}

#[tokio::test]
async fn graph_parents_every_node_span_to_the_multi_agent_span() {
    let _serial = SERIAL.lock().await;
    let (exporter, provider) = exporter();

    let graph = Graph::builder()
        .id("parent-graph")
        .node(agent("a"))
        .build()
        .expect("graph builds");
    graph.invoke("Hello").await.expect("graph runs");

    let spans = finished(&exporter, &provider);
    let orchestration = find(&spans, "invoke_graph parent-graph");
    let node = find(&spans, "node a");
    assert_eq!(node.parent_span_id, orchestration.span_context.span_id());
    assert_eq!(
        node.span_context.trace_id(),
        orchestration.span_context.trace_id()
    );
}

#[tokio::test]
async fn graph_creates_a_separate_node_span_for_each_parallel_source_node() {
    let _serial = SERIAL.lock().await;
    let (exporter, provider) = exporter();

    let graph = Graph::builder()
        .node(agent("a"))
        .node(agent("b"))
        .node(agent("c"))
        .edge("a", "c")
        .edge("b", "c")
        .build()
        .expect("graph builds");
    graph.invoke("Hello").await.expect("graph runs");

    let spans = finished(&exporter, &provider);
    let mut names = node_spans(&spans)
        .iter()
        .map(|span| span.name.to_string())
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(names, vec!["node a", "node b", "node c"]);
}

#[tokio::test]
async fn graph_merges_builder_trace_attributes_onto_its_spans() {
    let _serial = SERIAL.lock().await;
    let (exporter, provider) = exporter();

    let graph = Graph::builder()
        .id("tagged-graph")
        .node(agent("a"))
        .trace_attributes([("deployment".to_owned(), AttrValue::Str("test".to_owned()))])
        .build()
        .expect("graph builds");
    graph.invoke("Hello").await.expect("graph runs");

    let spans = finished(&exporter, &provider);
    assert_eq!(
        string_attr(find(&spans, "invoke_graph tagged-graph"), "deployment"),
        "test"
    );
    assert_eq!(string_attr(find(&spans, "node a"), "deployment"), "test");
}

// ---- Swarm ----

#[tokio::test]
async fn swarm_starts_and_ends_the_multi_agent_span_on_a_successful_invocation() {
    let _serial = SERIAL.lock().await;
    let (exporter, provider) = exporter();

    let swarm = Swarm::builder()
        .id("test-swarm")
        .node(swarm_agent("a", None))
        .max_steps(3)
        .build()
        .expect("swarm builds");
    swarm.invoke("Hello").await.expect("swarm runs");

    let spans = finished(&exporter, &provider);
    let span = find(&spans, "invoke_swarm test-swarm");
    assert_eq!(string_attr(span, "gen_ai.operation.name"), "invoke_swarm");
    assert_eq!(string_attr(span, "gen_ai.agent.name"), "swarm");
    assert_eq!(string_attr(span, "gen_ai.agent.id"), "test-swarm");
    assert_eq!(string_attr(span, "gen_ai.agent.input"), "\"Hello\"");
    assert!(int_attr(span, "gen_ai.agent.execution_time") >= 0);
    assert!(!is_error(span));
}

#[tokio::test]
async fn swarm_records_the_exact_result_usage_on_the_multi_agent_span() {
    let _serial = SERIAL.lock().await;
    let (exporter, provider) = exporter();

    let swarm = Swarm::builder()
        .id("usage-swarm")
        .node(swarm_agent(
            "a",
            Some(Usage {
                input_tokens: 10,
                output_tokens: 5,
                total_tokens: 15,
                ..Usage::default()
            }),
        ))
        .max_steps(3)
        .build()
        .expect("swarm builds");
    swarm.invoke("Hello").await.expect("swarm runs");

    let spans = finished(&exporter, &provider);
    let span = find(&spans, "invoke_swarm usage-swarm");
    assert_eq!(int_attr(span, "gen_ai.usage.input_tokens"), 10);
    assert_eq!(int_attr(span, "gen_ai.usage.output_tokens"), 5);
    assert_eq!(int_attr(span, "gen_ai.usage.total_tokens"), 15);
}

#[tokio::test]
async fn swarm_ends_a_node_span_with_completed_status_and_parents_it_to_the_multi_agent_span() {
    let _serial = SERIAL.lock().await;
    let (exporter, provider) = exporter();

    let swarm = Swarm::builder()
        .id("node-swarm")
        .node(swarm_agent(
            "a",
            Some(Usage {
                input_tokens: 10,
                output_tokens: 5,
                total_tokens: 15,
                ..Usage::default()
            }),
        ))
        .max_steps(3)
        .build()
        .expect("swarm builds");
    swarm.invoke("Hello").await.expect("swarm runs");

    let spans = finished(&exporter, &provider);
    let orchestration = find(&spans, "invoke_swarm node-swarm");
    let node = find(&spans, "node a");
    assert_eq!(string_attr(node, "gen_ai.operation.name"), "execute_node");
    assert_eq!(string_attr(node, "gen_ai.agent.node_type"), "agentNode");
    assert_eq!(string_attr(node, "gen_ai.agent.status"), "COMPLETED");
    assert_eq!(int_attr(node, "gen_ai.usage.total_tokens"), 15);
    assert_eq!(node.parent_span_id, orchestration.span_context.span_id());
}

#[tokio::test]
async fn swarm_ends_a_node_span_with_cancelled_status_and_zero_duration_when_a_hook_cancels() {
    let _serial = SERIAL.lock().await;
    let (exporter, provider) = exporter();

    let swarm = Swarm::builder()
        .id("cancel-swarm")
        .node(swarm_agent("a", None))
        .max_steps(3)
        .hook(|event: &mut BeforeNodeCallEvent| {
            event.cancel = Cancel::Reason("cancelled by test".to_owned());
            Ok(())
        })
        .build()
        .expect("swarm builds");
    swarm.invoke("Hello").await.expect("swarm runs");

    let spans = finished(&exporter, &provider);
    let span = find(&spans, "node a");
    assert_eq!(string_attr(span, "gen_ai.agent.status"), "CANCELLED");
    assert_eq!(int_attr(span, "gen_ai.agent.execution_time"), 0);
}

#[tokio::test]
async fn swarm_merges_builder_trace_attributes_onto_its_spans() {
    let _serial = SERIAL.lock().await;
    let (exporter, provider) = exporter();

    let swarm = Swarm::builder()
        .id("tagged-swarm")
        .node(swarm_agent("a", None))
        .max_steps(3)
        .trace_attributes([("deployment".to_owned(), AttrValue::Str("test".to_owned()))])
        .build()
        .expect("swarm builds");
    swarm.invoke("Hello").await.expect("swarm runs");

    let spans = finished(&exporter, &provider);
    assert_eq!(
        string_attr(find(&spans, "invoke_swarm tagged-swarm"), "deployment"),
        "test"
    );
    assert_eq!(string_attr(find(&spans, "node a"), "deployment"), "test");
}

/// Mirrors TS's `withSpanContext` propagation test: Rust has no ambient span context, so the
/// node's agent span is a root span instead of a child of the node span (see DESIGN.md).
#[tokio::test]
async fn agent_spans_inside_a_node_are_not_parented_to_the_node_span() {
    let _serial = SERIAL.lock().await;
    let (exporter, provider) = exporter();

    let graph = Graph::builder()
        .id("context-graph")
        .node(agent("a"))
        .build()
        .expect("graph builds");
    graph.invoke("Hello").await.expect("graph runs");

    let spans = finished(&exporter, &provider);
    let node = find(&spans, "node a");
    let agent_span = find(&spans, "invoke_agent a");
    assert_ne!(agent_span.parent_span_id, node.span_context.span_id());
}
