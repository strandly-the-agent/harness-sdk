//! Ports of `strands-ts/src/agent/__tests__/agent.tracer.test.node.ts`: the spans an `Agent` opens
//! and closes, read from an in-memory exporter where TS reads a mocked `Tracer`.

use std::sync::Arc;

use futures::StreamExt;
use opentelemetry::trace::Status;
use opentelemetry_sdk::trace::SpanData;
use tokio::sync::Notify;

use super::{Recorder, agent_of, recording, semconv, tool_use};
use crate::Agent;
use crate::test_support::stream_tool;
use crate::testing::MockMessageModel;
use crate::tools::{ToolOutcome, ToolStream};
use crate::types::agent::AgentStreamEvent;
use crate::types::messages::TextBlock;

fn text_agent() -> Agent {
    let model = MockMessageModel::new();
    model.add_turn(vec![TextBlock::new("Done").into()]);
    agent_of(model, Vec::new(), semconv(""))
}

/// Streams `prompt` until `stop` matches an event, then drops the stream.
async fn stream_until(agent: &mut Agent, stop: impl Fn(&AgentStreamEvent) -> bool) {
    let mut stream = agent.stream("Hi");
    while let Some(event) = stream.next().await {
        if stop(&event.expect("an event")) {
            break;
        }
    }
}

fn named<'a>(spans: &'a [SpanData], name: &str) -> Vec<&'a SpanData> {
    spans.iter().filter(|span| span.name == name).collect()
}

/// The span ended through the tracer: OK status and the end-time attribute.
fn assert_ended_ok(span: &SpanData) {
    assert_eq!(span.status, Status::Ok, "{}", span.name);
    assert!(
        span.attributes
            .iter()
            .any(|attribute| attribute.key.as_str() == "gen_ai.event.end_time"),
        "{} has no end time",
        span.name
    );
}

fn assert_closed(agent: &Agent, recorder: &Recorder, chats: usize) {
    let spans = recorder.spans();
    assert_eq!(named(&spans, "chat").len(), chats);
    let cycles = named(&spans, "execute_agent_loop_cycle");
    assert_eq!(cycles.len(), 1);
    assert_ended_ok(cycles[0]);
    let agents = named(&spans, "invoke_agent telemetry_agent");
    assert_eq!(agents.len(), 1);
    assert_ended_ok(agents[0]);
    assert!(!agent.local.telemetry().lock().meter.has_open_cycle());
    assert_eq!(
        agent
            .metrics()
            .latest_agent_invocation()
            .map(|i| i.cycles().len()),
        Some(1)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn ends_the_loop_span_once_when_the_public_stream_iterator_is_closed_mid_cycle() {
    let (recorder, _guard) = recording();
    let mut agent = text_agent();
    stream_until(&mut agent, |event| {
        matches!(event, AgentStreamEvent::MessageAdded(_))
    })
    .await;
    assert_closed(&agent, &recorder, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn ends_the_loop_span_once_when_the_consumer_cancels_and_breaks_during_model_streaming() {
    let (recorder, _guard) = recording();
    let mut agent = text_agent();
    let cancel = agent.cancel_handle();
    stream_until(&mut agent, |event| {
        let update = matches!(event, AgentStreamEvent::ModelStreamUpdate(_));
        if update {
            cancel.cancel(crate::cancel::CancelOptions::default());
        }
        update
    })
    .await;
    assert_closed(&agent, &recorder, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn ends_the_model_invoke_span_once_when_the_consumer_breaks_mid_model_response() {
    let (recorder, _guard) = recording();
    let mut agent = text_agent();
    stream_until(&mut agent, |event| {
        matches!(event, AgentStreamEvent::ModelStreamUpdate(_))
    })
    .await;
    assert_closed(&agent, &recorder, 1);
    assert_ended_ok(named(&recorder.spans(), "chat")[0]);
}

// No TS case: a dropped stream aborts the running tool, whose span and metric still end.
#[tokio::test(flavor = "current_thread")]
async fn a_dropped_stream_ends_the_span_of_the_running_tool() {
    let (recorder, _guard) = recording();
    let model = MockMessageModel::new();
    model.add_turn(vec![tool_use("hang", "tool-1", serde_json::json!({}))]);
    let started = Arc::new(Notify::new());
    let tool_started = Arc::clone(&started);
    let hang = stream_tool("hang", move |_context| {
        let started = Arc::clone(&tool_started);
        ToolStream::from_future(async move {
            started.notify_one();
            std::future::pending::<ToolOutcome>().await
        })
    });
    let mut agent = agent_of(model, vec![Arc::new(hang)], semconv(""));
    {
        let mut stream = agent.stream("Hi");
        loop {
            tokio::select! {
                biased;
                () = started.notified() => break,
                event = stream.next() => { event.expect("the stream is open").expect("an event"); }
            }
        }
    }
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert_closed(&agent, &recorder, 1);
    let spans = recorder.spans();
    assert_ended_ok(named(&spans, "execute_tool hang")[0]);
    let metrics = agent.metrics();
    let (_, tool) = metrics
        .tool_metrics()
        .find(|(name, _)| *name == "hang")
        .expect("the tool metric");
    assert_eq!((tool.call_count(), tool.success_count()), (1, 0));
}

// No TS case (TS reads `this.tools` before it registers the structured-output tool).
#[tokio::test(flavor = "current_thread")]
async fn agent_span_tools_exclude_the_structured_output_tool() {
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    struct Value {
        #[expect(dead_code, reason = "only the schema is used")]
        value: f64,
    }
    let (recorder, _guard) = recording();
    let model = MockMessageModel::new();
    model.add_turn(vec![tool_use(
        crate::tools::STRUCTURED_OUTPUT_TOOL_NAME,
        "so-1",
        serde_json::json!({"value": 1}),
    )]);
    let mut agent = agent_of(model, Vec::new(), semconv(""));
    let options = crate::types::agent::InvokeOptions::default().structured_output_schema::<Value>();
    agent.invoke_with("Hi", options).await.expect("the run succeeds");

    let spans = recorder.spans();
    let agent_span = named(&spans, "invoke_agent telemetry_agent")[0];
    assert!(
        agent_span
            .attributes
            .iter()
            .all(|attribute| attribute.key.as_str() != "gen_ai.agent.tools")
    );
    assert_eq!(named(&spans, "execute_tool strands_structured_output").len(), 1);
}

// No TS case: a `CancelledError` leaves the TS cycle through `closeCycle(error)`.
#[tokio::test(flavor = "current_thread")]
async fn ends_the_loop_span_with_an_error_when_the_invocation_is_cancelled_mid_model_call() {
    let (recorder, _guard) = recording();
    let mut agent = text_agent();
    let cancel = agent.cancel_handle();
    {
        let mut stream = agent.stream("Hi");
        while let Some(event) = stream.next().await {
            if matches!(event.expect("an event"), AgentStreamEvent::ModelStreamUpdate(_)) {
                cancel.cancel(crate::cancel::CancelOptions::default());
            }
        }
    }
    let spans = recorder.spans();
    let cycle = named(&spans, "execute_agent_loop_cycle")[0];
    assert_eq!(cycle.status, Status::error("Agent invocation cancelled"));
    assert_eq!(
        cycle
            .events
            .iter()
            .filter(|event| event.name == "exception")
            .count(),
        1
    );
    assert_ended_ok(named(&spans, "invoke_agent telemetry_agent")[0]);
}
