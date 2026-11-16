//! Ports of `strands-ts/src/agent/__tests__/agent.tracer.test.node.ts`: the spans an `Agent` opens
//! and closes, read from an in-memory exporter where TS reads a mocked `Tracer`.

use std::sync::Arc;

use futures::StreamExt;
use opentelemetry::trace::Status;
use opentelemetry_sdk::trace::SpanData;
use tokio::sync::Notify;

use super::{Recorder, agent_of, recording, semconv, tool_use, usage_turn};
use crate::telemetry::AttributeValue;
use crate::test_support::{mock_tool, stream_tool, text_result};
use crate::testing::{MockMessageModel, MockTurnOptions};
use crate::tools::{ToolOutcome, ToolStream};
use crate::types::agent::AgentStreamEvent;
use crate::types::messages::{StopReason, TextBlock, ToolResultBlock, ToolResultContent};
use crate::types::tools::ToolResultStatus;
use crate::{Agent, AgentConfig};

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

fn plain_agent(model: MockMessageModel, tools: Vec<Arc<dyn crate::tools::Tool>>) -> Agent {
    let agent = AgentConfig::builder()
        .model(Arc::new(model) as Arc<dyn crate::models::model::Model>)
        .tools(tools)
        .build()
        .expect("a valid agent");
    agent.local.telemetry().lock().tracer.set_semconv(semconv(""));
    agent
}

fn text_model(text: &str) -> MockMessageModel {
    let model = MockMessageModel::new();
    model.add_turn(vec![TextBlock::new(text).into()]);
    model
}

fn max_tokens_model() -> MockMessageModel {
    let model = MockMessageModel::new();
    model.add_turn_with(
        vec![TextBlock::new("Partial").into()],
        MockTurnOptions::builder()
            .stop_reason(StopReason::MaxTokens)
            .build(),
    );
    model
}

/// One `testTool` call (`tool-1`), then the text `Done`.
fn tool_then_text_model() -> MockMessageModel {
    let model = MockMessageModel::new();
    model.add_turn(vec![tool_use(
        "testTool",
        "tool-1",
        serde_json::json!({"key": "val"}),
    )]);
    model.add_turn(vec![TextBlock::new("Done").into()]);
    model
}

fn test_tool(name: &str) -> Arc<dyn crate::tools::Tool> {
    Arc::new(mock_tool(name, |context| {
        text_result(context.tool_use().tool_use_id(), "Result")
    }))
}

fn attribute(span: &SpanData, key: &str) -> Option<String> {
    span.attributes
        .iter()
        .find(|attribute| attribute.key.as_str() == key)
        .map(|attribute| attribute.value.to_string())
}

fn is_error(span: &SpanData) -> bool {
    matches!(span.status, Status::Error { .. })
}

mod constructor {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn initializes_tracer_with_trace_attributes_from_config() {
        let (recorder, _guard) = recording();
        let mut agent = AgentConfig::builder()
            .model(Arc::new(text_model("Hello")) as Arc<dyn crate::models::model::Model>)
            .trace_attributes([("custom.attr".to_owned(), AttributeValue::from("value"))])
            .build()
            .expect("a valid agent");
        agent.invoke("Hi").await.expect("the run succeeds");
        for span in recorder.spans() {
            assert_eq!(
                attribute(&span, "custom.attr").as_deref(),
                Some("value"),
                "{}",
                span.name
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn initializes_tracer_without_trace_attributes_when_not_provided() {
        let (recorder, _guard) = recording();
        let mut agent = plain_agent(text_model("Hello"), Vec::new());
        agent.invoke("Hi").await.expect("the run succeeds");
        for span in recorder.spans() {
            assert_eq!(attribute(&span, "custom.attr"), None, "{}", span.name);
        }
    }
}

mod name_and_id {
    use super::*;

    #[test]
    fn defaults_name_to_strands_agent() {
        assert_eq!(
            plain_agent(text_model("Hello"), Vec::new()).name(),
            "Strands Agent"
        );
    }

    #[test]
    fn defaults_id_to_agent() {
        assert_eq!(plain_agent(text_model("Hello"), Vec::new()).id(), "agent");
    }

    #[test]
    fn uses_provided_name() {
        assert_eq!(
            agent_of(text_model("Hello"), Vec::new(), semconv("")).name(),
            "telemetry_agent"
        );
    }

    #[test]
    fn uses_provided_id() {
        assert_eq!(
            agent_of(text_model("Hello"), Vec::new(), semconv("")).id(),
            "telemetry_agent"
        );
    }
}

mod agent_span_lifecycle {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn starts_and_ends_agent_span_on_successful_invocation() {
        let (recorder, _guard) = recording();
        let mut agent = agent_of(text_model("Hello"), Vec::new(), semconv(""));
        agent.invoke("Hi").await.expect("the run succeeds");
        let spans = recorder.spans();
        let agents = named(&spans, "invoke_agent telemetry_agent");
        assert_eq!(agents.len(), 1);
        assert_eq!(
            attribute(agents[0], "gen_ai.agent.name").as_deref(),
            Some("telemetry_agent")
        );
        assert_eq!(
            attribute(agents[0], "gen_ai.agent.id").as_deref(),
            Some("telemetry_agent")
        );
        assert_eq!(
            attribute(agents[0], "gen_ai.request.model").as_deref(),
            Some("test-model")
        );
        assert_ended_ok(agents[0]);
        let choice = agents[0]
            .events
            .iter()
            .find(|event| event.name == "gen_ai.choice")
            .expect("the response event");
        assert!(
            choice
                .attributes
                .iter()
                .any(|attribute| attribute.key.as_str() == "finish_reason"
                    && attribute.value.as_str() == "endTurn")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ends_agent_span_with_error_when_invocation_fails() {
        let (recorder, _guard) = recording();
        let mut agent = plain_agent(max_tokens_model(), Vec::new());
        agent.invoke("Hi").await.expect_err("the run fails on max tokens");
        let spans = recorder.spans();
        let agents = named(&spans, "invoke_agent Strands Agent");
        assert_eq!(agents.len(), 1);
        assert!(is_error(agents[0]));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn includes_system_prompt_in_agent_span_when_configured() {
        let (recorder, _guard) = recording();
        let mut agent = agent_of(text_model("Hello"), Vec::new(), semconv(""));
        agent.invoke("Hi").await.expect("the run succeeds");
        let spans = recorder.spans();
        let agent_span = named(&spans, "invoke_agent telemetry_agent")[0];
        assert_eq!(
            attribute(agent_span, "system_prompt").as_deref(),
            Some("\"You report the weather.\"")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn includes_empty_string_system_prompt_in_agent_span() {
        let (recorder, _guard) = recording();
        let mut agent = AgentConfig::builder()
            .model(Arc::new(text_model("Hello")) as Arc<dyn crate::models::model::Model>)
            .system_prompt("")
            .build()
            .expect("a valid agent");
        agent.invoke("Hi").await.expect("the run succeeds");
        let spans = recorder.spans();
        let agent_span = named(&spans, "invoke_agent Strands Agent")[0];
        assert_eq!(attribute(agent_span, "system_prompt").as_deref(), Some("\"\""));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn omits_system_prompt_from_agent_span_when_not_configured() {
        let (recorder, _guard) = recording();
        let mut agent = plain_agent(text_model("Hello"), Vec::new());
        agent.invoke("Hi").await.expect("the run succeeds");
        let spans = recorder.spans();
        assert_eq!(
            attribute(named(&spans, "invoke_agent Strands Agent")[0], "system_prompt"),
            None
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn includes_tools_in_agent_span() {
        let (recorder, _guard) = recording();
        let mut agent = plain_agent(text_model("Hello"), vec![test_tool("myTool")]);
        agent.invoke("Hi").await.expect("the run succeeds");
        let spans = recorder.spans();
        assert_eq!(
            attribute(
                named(&spans, "invoke_agent Strands Agent")[0],
                "gen_ai.agent.tools"
            )
            .as_deref(),
            Some("[\"myTool\"]")
        );
    }
}

mod agent_loop_span_lifecycle {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn starts_and_ends_loop_span_for_each_cycle() {
        let (recorder, _guard) = recording();
        let mut agent = plain_agent(text_model("Done"), Vec::new());
        agent.invoke("Hi").await.expect("the run succeeds");
        let spans = recorder.spans();
        let cycles = named(&spans, "execute_agent_loop_cycle");
        assert_eq!(cycles.len(), 1);
        assert_eq!(
            attribute(cycles[0], "agent_loop.cycle_id").as_deref(),
            Some("cycle-1")
        );
        assert_ended_ok(cycles[0]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn creates_multiple_loop_spans_for_multi_cycle_invocations() {
        let (recorder, _guard) = recording();
        let mut agent = plain_agent(tool_then_text_model(), vec![test_tool("testTool")]);
        agent.invoke("Use tool").await.expect("the run succeeds");
        let spans = recorder.spans();
        let mut ids: Vec<_> = named(&spans, "execute_agent_loop_cycle")
            .into_iter()
            .map(|cycle| {
                assert_ended_ok(cycle);
                attribute(cycle, "agent_loop.cycle_id").expect("a cycle id")
            })
            .collect();
        ids.sort();
        assert_eq!(ids, ["cycle-1", "cycle-2"]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ends_loop_span_with_error_when_cycle_fails() {
        let (recorder, _guard) = recording();
        let mut agent = plain_agent(max_tokens_model(), Vec::new());
        agent.invoke("Hi").await.expect_err("the run fails on max tokens");
        let spans = recorder.spans();
        let cycles = named(&spans, "execute_agent_loop_cycle");
        assert_eq!(cycles.len(), 1);
        assert!(is_error(cycles[0]));
    }
}

mod model_invoke_span_lifecycle {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn starts_and_ends_model_span_on_successful_model_call() {
        let (recorder, _guard) = recording();
        let mut agent = plain_agent(text_model("Hello"), Vec::new());
        agent.invoke("Hi").await.expect("the run succeeds");
        let spans = recorder.spans();
        let chats = named(&spans, "chat");
        assert_eq!(chats.len(), 1);
        assert_eq!(
            attribute(chats[0], "gen_ai.request.model").as_deref(),
            Some("test-model")
        );
        assert_ended_ok(chats[0]);
        assert!(chats[0].events.iter().any(|event| event.name == "gen_ai.choice"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ends_model_span_with_error_when_model_call_fails() {
        let (recorder, _guard) = recording();
        let model = MockMessageModel::new();
        model.add_error(|| crate::Error::other("model exploded"));
        let mut agent = plain_agent(model, Vec::new());
        agent.invoke("Hi").await.expect_err("the model call fails");
        let spans = recorder.spans();
        let chats = named(&spans, "chat");
        assert_eq!(chats.len(), 1);
        assert!(is_error(chats[0]));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn creates_model_span_for_each_model_call_in_multi_cycle_invocation() {
        let (recorder, _guard) = recording();
        let mut agent = plain_agent(tool_then_text_model(), vec![test_tool("testTool")]);
        agent.invoke("Use tool").await.expect("the run succeeds");
        let spans = recorder.spans();
        let chats = named(&spans, "chat");
        assert_eq!(chats.len(), 2);
        chats.into_iter().for_each(assert_ended_ok);
    }
}

mod tool_call_span_lifecycle {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn starts_and_ends_tool_span_for_each_tool_execution() {
        let (recorder, _guard) = recording();
        let mut agent = plain_agent(tool_then_text_model(), vec![test_tool("testTool")]);
        agent.invoke("Use tool").await.expect("the run succeeds");
        let spans = recorder.spans();
        let tools = named(&spans, "execute_tool testTool");
        assert_eq!(tools.len(), 1);
        assert_eq!(
            attribute(tools[0], "gen_ai.tool.name").as_deref(),
            Some("testTool")
        );
        assert_eq!(
            attribute(tools[0], "gen_ai.tool.call.id").as_deref(),
            Some("tool-1")
        );
        assert_eq!(
            attribute(tools[0], "gen_ai.tool.status").as_deref(),
            Some("success")
        );
        assert_ended_ok(tools[0]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ends_tool_span_with_error_when_tool_is_not_found() {
        let (recorder, _guard) = recording();
        let mut agent = plain_agent(tool_then_text_model(), Vec::new());
        agent.invoke("Use tool").await.expect("the run succeeds");
        let spans = recorder.spans();
        let tool = named(&spans, "execute_tool testTool")[0];
        assert_eq!(attribute(tool, "gen_ai.tool.status").as_deref(), Some("error"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ends_tool_span_with_error_when_tool_throws() {
        let (recorder, _guard) = recording();
        let failing = mock_tool("testTool", |context| {
            ToolResultBlock::new(
                context.tool_use().tool_use_id(),
                ToolResultStatus::Error,
                vec![ToolResultContent::Text(TextBlock::new("Tool failed"))],
            )
            .with_error("Tool failed")
        });
        let mut agent = plain_agent(tool_then_text_model(), vec![Arc::new(failing)]);
        agent.invoke("Use tool").await.expect("the run succeeds");
        let spans = recorder.spans();
        let tool = named(&spans, "execute_tool testTool")[0];
        assert_eq!(attribute(tool, "gen_ai.tool.status").as_deref(), Some("error"));
        assert_eq!(tool.status, Status::error("Tool failed"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn creates_spans_for_multiple_tool_calls_in_a_single_turn() {
        let (recorder, _guard) = recording();
        let model = MockMessageModel::new();
        model.add_turn(vec![
            tool_use("toolA", "tool-1", serde_json::json!({})),
            tool_use("toolB", "tool-2", serde_json::json!({})),
        ]);
        model.add_turn(vec![TextBlock::new("Done").into()]);
        let mut agent = plain_agent(model, vec![test_tool("toolA"), test_tool("toolB")]);
        agent.invoke("Use tools").await.expect("the run succeeds");
        let spans = recorder.spans();
        assert_ended_ok(named(&spans, "execute_tool toolA")[0]);
        assert_ended_ok(named(&spans, "execute_tool toolB")[0]);
    }
}

mod token_usage_accumulation {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn passes_accumulated_usage_to_end_agent_span() {
        let (recorder, _guard) = recording();
        let model = MockMessageModel::new();
        model.add_turn_with(
            vec![tool_use("testTool", "tool-1", serde_json::json!({}))],
            usage_turn(10, 5),
        );
        model.add_turn_with(vec![TextBlock::new("Done").into()], usage_turn(20, 10));
        let mut agent = plain_agent(model, vec![test_tool("testTool")]);
        agent.invoke("Use tool").await.expect("the run succeeds");
        let spans = recorder.spans();
        let agent_span = named(&spans, "invoke_agent Strands Agent")[0];
        assert_eq!(
            attribute(agent_span, "gen_ai.usage.input_tokens").as_deref(),
            Some("30")
        );
        assert_eq!(
            attribute(agent_span, "gen_ai.usage.output_tokens").as_deref(),
            Some("15")
        );
        assert_eq!(
            attribute(agent_span, "gen_ai.usage.total_tokens").as_deref(),
            Some("45")
        );
    }
}

mod span_context_hierarchy {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn resets_accumulated_usage_on_each_invocation() {
        let (recorder, _guard) = recording();
        let model = MockMessageModel::new();
        model.add_turn_with(vec![TextBlock::new("First").into()], usage_turn(10, 5));
        model.add_turn_with(vec![TextBlock::new("Second").into()], usage_turn(20, 10));
        let mut agent = plain_agent(model, Vec::new());
        agent.invoke("First").await.expect("the first run succeeds");
        agent.invoke("Second").await.expect("the second run succeeds");
        let spans = recorder.spans();
        let agents = named(&spans, "invoke_agent Strands Agent");
        assert_eq!(agents.len(), 2);
        agents.into_iter().for_each(assert_ended_ok);
    }
}

mod structured_output_and_telemetry_interaction {
    use super::*;
    use crate::tools::STRUCTURED_OUTPUT_TOOL_NAME;
    use crate::types::agent::InvokeOptions;

    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    struct Value {
        #[expect(dead_code, reason = "only the schema is used")]
        value: f64,
    }

    fn options() -> InvokeOptions {
        InvokeOptions::default().structured_output_schema::<Value>()
    }

    fn output_call() -> crate::types::messages::ContentBlock {
        tool_use(
            STRUCTURED_OUTPUT_TOOL_NAME,
            "tool-1",
            serde_json::json!({"value": 42}),
        )
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ends_loop_span_for_cycle_where_structured_output_forces_tool_choice() {
        let (recorder, _guard) = recording();
        let model = MockMessageModel::new();
        model.add_turn(vec![TextBlock::new("First response").into()]);
        model.add_turn(vec![output_call()]);
        let mut agent = plain_agent(model, Vec::new());
        agent
            .invoke_with("Test", options())
            .await
            .expect("the run succeeds");
        let spans = recorder.spans();
        let mut ids: Vec<_> = named(&spans, "execute_agent_loop_cycle")
            .into_iter()
            .map(|cycle| {
                assert_ended_ok(cycle);
                attribute(cycle, "agent_loop.cycle_id").expect("a cycle id")
            })
            .collect();
        ids.sort();
        assert_eq!(ids, ["cycle-1", "cycle-2"]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn creates_tool_span_for_structured_output_tool_execution() {
        let (recorder, _guard) = recording();
        let model = MockMessageModel::new();
        model.add_turn(vec![output_call()]);
        model.add_turn(vec![TextBlock::new("Done").into()]);
        let mut agent = plain_agent(model, Vec::new());
        agent
            .invoke_with("Test", options())
            .await
            .expect("the run succeeds");
        let spans = recorder.spans();
        let tools = named(&spans, "execute_tool strands_structured_output");
        assert_eq!(tools.len(), 1);
        assert_eq!(
            attribute(tools[0], "gen_ai.tool.status").as_deref(),
            Some("success")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ends_agent_span_with_result_on_successful_structured_output() {
        let (recorder, _guard) = recording();
        let model = MockMessageModel::new();
        model.add_turn(vec![output_call()]);
        let mut agent = plain_agent(model, Vec::new());
        agent
            .invoke_with("Test", options())
            .await
            .expect("the run succeeds");
        let spans = recorder.spans();
        let agent_span = named(&spans, "invoke_agent Strands Agent")[0];
        assert_ended_ok(agent_span);
        let choice = agent_span
            .events
            .iter()
            .find(|event| event.name == "gen_ai.choice")
            .expect("the response event");
        assert!(
            choice
                .attributes
                .iter()
                .any(|attribute| attribute.key.as_str() == "finish_reason"
                    && attribute.value.as_str() == "toolUse")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ends_agent_span_with_error_on_max_tokens_with_structured_output_schema() {
        let (recorder, _guard) = recording();
        let mut agent = plain_agent(max_tokens_model(), Vec::new());
        agent
            .invoke_with("Test", options())
            .await
            .expect_err("the run fails on max tokens");
        let spans = recorder.spans();
        assert!(is_error(named(&spans, "invoke_agent Strands Agent")[0]));
        assert!(is_error(named(&spans, "execute_agent_loop_cycle")[0]));
    }
}
