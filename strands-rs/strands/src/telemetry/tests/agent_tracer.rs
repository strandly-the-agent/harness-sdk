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

    #[tokio::test(flavor = "current_thread")]
    async fn model_span_records_post_middleware_context_when_invoke_model_stage_middleware_transforms_input() {
        use crate::middleware::{InvokeModelStage, MiddlewareStage as _};
        use crate::models::model::{BaseModelConfig, Model};

        let (recorder, _guard) = recording();
        let selected = text_model("Selected response");
        selected.update_config(BaseModelConfig {
            model_id: Some("selected-model".to_owned()),
            ..BaseModelConfig::default()
        });
        let selected: Arc<dyn Model> = Arc::new(selected);
        let mut agent = AgentConfig::builder()
            .model(Arc::new(text_model("Default response")) as Arc<dyn Model>)
            .system_prompt("Original prompt")
            .build()
            .expect("a valid agent");
        agent.local.telemetry().lock().tracer.set_semconv(semconv(""));
        agent.add_middleware(InvokeModelStage::input(move |mut context| {
            let selected = Arc::clone(&selected);
            async move {
                context.model = selected;
                context.system_prompt = Some("Transformed prompt".into());
                Ok(context)
            }
        }));
        agent.invoke("Hi").await.expect("the run succeeds");
        let spans = recorder.spans();
        let chat = named(&spans, "chat")[0];
        assert_eq!(
            attribute(chat, "gen_ai.request.model").as_deref(),
            Some("selected-model")
        );
        let system = chat
            .events
            .iter()
            .find(|event| event.name == "gen_ai.system.message")
            .expect("the system prompt event");
        assert_eq!(
            system
                .attributes
                .iter()
                .find(|attribute| attribute.key.as_str() == "content")
                .map(|attribute| attribute.value.to_string())
                .as_deref(),
            Some(r#"[{"text":"Transformed prompt"}]"#)
        );
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

    #[tokio::test(flavor = "current_thread")]
    async fn creates_overlapping_tool_spans_when_tool_executor_is_concurrent() {
        let (recorder, _guard) = recording();
        let model = MockMessageModel::new();
        model.add_turn(vec![
            tool_use("tool1", "id-1", serde_json::json!({})),
            tool_use("tool2", "id-2", serde_json::json!({})),
        ]);
        model.add_turn(vec![TextBlock::new("Done").into()]);
        // Each tool waits for the other to start, so neither can end before both started.
        let both_started = Arc::new(tokio::sync::Barrier::new(2));
        let waiting_tool = |name: &str, text: &'static str| -> Arc<dyn crate::tools::Tool> {
            let both_started = Arc::clone(&both_started);
            Arc::new(stream_tool(name, move |context| {
                let both_started = Arc::clone(&both_started);
                ToolStream::from_future(async move {
                    both_started.wait().await;
                    text_result(context.tool_use().tool_use_id(), text).into()
                })
            }))
        };
        let mut agent = AgentConfig::builder()
            .model(Arc::new(model) as Arc<dyn crate::models::model::Model>)
            .tools(vec![waiting_tool("tool1", "R1"), waiting_tool("tool2", "R2")])
            .tool_executor(crate::tools::ConcurrentToolExecutor::new())
            .build()
            .expect("a valid agent");
        agent.invoke("Use tools").await.expect("the run succeeds");
        let spans = recorder.spans();
        let tools: Vec<_> = ["execute_tool tool1", "execute_tool tool2"]
            .into_iter()
            .flat_map(|name| named(&spans, name))
            .collect();
        assert_eq!(tools.len(), 2);
        // Both starts precede both ends: the spans overlap.
        let last_start = tools.iter().map(|span| span.start_time).max();
        let first_end = tools.iter().map(|span| span.end_time).min();
        assert!(last_start <= first_end, "{last_start:?} > {first_end:?}");
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

    #[tokio::test(flavor = "current_thread")]
    async fn ends_agent_span_with_error_when_model_refuses_structured_output_tool_after_forcing() {
        let (recorder, _guard) = recording();
        let mut agent = plain_agent(text_model("I refuse"), Vec::new());
        let error = agent
            .invoke_with("Test", options())
            .await
            .expect_err("the model never calls the tool");
        assert!(matches!(error, crate::Error::StructuredOutput(_)), "{error:?}");
        let spans = recorder.spans();
        let agent_span = named(&spans, "invoke_agent Strands Agent")[0];
        assert_eq!(agent_span.status, Status::error(error.to_string()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ends_cycle_span_with_error_on_structured_output_error() {
        let (recorder, _guard) = recording();
        let mut agent = plain_agent(text_model("I refuse"), Vec::new());
        let error = agent
            .invoke_with("Test", options())
            .await
            .expect_err("the model never calls the tool");
        assert!(matches!(error, crate::Error::StructuredOutput(_)), "{error:?}");
        // Cycle 1 ends cleanly and forces the tool; cycle 2 fails.
        let spans = recorder.spans();
        let cycle = |id: &str| {
            *named(&spans, "execute_agent_loop_cycle")
                .iter()
                .find(|cycle| attribute(cycle, "agent_loop.cycle_id").as_deref() == Some(id))
                .expect("the cycle span")
        };
        assert_eq!(named(&spans, "execute_agent_loop_cycle").len(), 2);
        assert_ended_ok(cycle("cycle-1"));
        assert_eq!(cycle("cycle-2").status, Status::error(error.to_string()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn creates_correct_spans_for_validation_retry_cycle() {
        #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
        struct Person {
            #[expect(dead_code, reason = "only the schema is used")]
            name: String,
            #[expect(dead_code, reason = "only the schema is used")]
            age: f64,
        }

        let (recorder, _guard) = recording();
        let model = MockMessageModel::new();
        model.add_turn(vec![tool_use(
            STRUCTURED_OUTPUT_TOOL_NAME,
            "tool-1",
            serde_json::json!({"name": "John", "age": "not-a-number"}),
        )]);
        model.add_turn(vec![tool_use(
            STRUCTURED_OUTPUT_TOOL_NAME,
            "tool-2",
            serde_json::json!({"name": "John", "age": 30}),
        )]);
        let mut agent = plain_agent(model, Vec::new());
        agent
            .invoke_with(
                "Test",
                InvokeOptions::default().structured_output_schema::<Person>(),
            )
            .await
            .expect("the second call validates");
        let spans = recorder.spans();

        // Two cycles: the invalid tool use, then the valid one with the early exit.
        let cycles = named(&spans, "execute_agent_loop_cycle");
        let ids: Vec<_> = cycles
            .iter()
            .map(|cycle| {
                assert_ended_ok(cycle);
                attribute(cycle, "agent_loop.cycle_id").expect("a cycle id")
            })
            .collect();
        assert_eq!(ids, ["cycle-1", "cycle-2"]);

        // One model call per cycle, each ended with its assistant message.
        let chats = named(&spans, "chat");
        assert_eq!(chats.len(), 2);
        for chat in chats {
            assert_ended_ok(chat);
            assert!(chat.events.iter().any(|event| event.name == "gen_ai.choice"));
        }

        // Two tool calls: the first fails validation, the second succeeds.
        let tools: Vec<_> = named(&spans, "execute_tool strands_structured_output")
            .into_iter()
            .map(|tool| {
                (
                    attribute(tool, "gen_ai.tool.call.id").expect("a tool use id"),
                    attribute(tool, "gen_ai.tool.status").expect("a status"),
                )
            })
            .collect();
        assert_eq!(
            tools,
            [
                ("tool-1".to_owned(), "error".to_owned()),
                ("tool-2".to_owned(), "success".to_owned())
            ]
        );
    }
}

mod baggage_propagation {
    use opentelemetry::baggage::BaggageExt as _;
    use opentelemetry::trace::{FutureExt as _, TracerProvider as _};
    use opentelemetry::{Context, KeyValue};
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use tracing_subscriber::layer::SubscriberExt as _;

    use super::*;
    use crate::hooks::AfterInvocationEvent;
    use crate::telemetry::{BaggageKeys, BaggageSpanProcessor};

    type Seen = Arc<std::sync::Mutex<Vec<Option<String>>>>;

    fn tenant() -> Option<String> {
        entry("tenant.id")
    }

    fn entry(key: &str) -> Option<String> {
        Context::current()
            .baggage()
            .get(key)
            .map(|value| value.as_str().to_owned())
    }

    fn seen_by(seen: &Seen) -> Vec<Option<String>> {
        seen.lock().expect("the samples").clone()
    }

    /// An agent that calls `tool` once and then ends its turn.
    fn tool_agent(tool: Arc<dyn crate::tools::Tool>, baggage: &[(&str, &str)]) -> Agent {
        let model = MockMessageModel::new();
        model.add_turn(vec![tool_use(tool.name(), "t1", serde_json::json!({}))]);
        model.add_turn(vec![TextBlock::new("done").into()]);
        AgentConfig::builder()
            .model(Arc::new(model) as Arc<dyn crate::models::model::Model>)
            .tools(vec![tool])
            .printer(false)
            .baggage_attributes(
                baggage
                    .iter()
                    .map(|(key, value)| ((*key).to_owned(), (*value).to_owned())),
            )
            .build()
            .expect("a valid agent")
    }

    /// A tool that records the `tenant.id` baggage entry it runs under.
    fn probe(name: &str, seen: &Seen) -> Arc<dyn crate::tools::Tool> {
        let seen = Arc::clone(seen);
        Arc::new(mock_tool(name, move |context| {
            seen.lock().expect("the samples").push(tenant());
            ToolResultBlock::new(
                context.tool_use().tool_use_id(),
                ToolResultStatus::Success,
                Vec::new(),
            )
        }))
    }

    #[tokio::test(flavor = "current_thread")]
    async fn defaults_to_an_empty_map_and_leaves_the_active_baggage_untouched() {
        let seen = Seen::default();
        let mut agent = tool_agent(probe("probe", &seen), &[]);
        agent.invoke("Hi").await.expect("the run succeeds");
        // The agent keeps its entries private; the config default is the empty map.
        assert!(AgentConfig::default().baggage_attributes.is_empty());
        assert_eq!(seen_by(&seen), [None]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn is_current_inside_tools_and_when_the_agent_span_starts_merged_with_ambient_baggage() {
        // The baggage span processor copies the baggage active at span start onto the span.
        let recorder = Recorder::default();
        let provider = SdkTracerProvider::builder()
            .with_span_processor(BaggageSpanProcessor::new(BaggageKeys::All))
            .with_simple_exporter(recorder.clone())
            .build();
        let layer = tracing_opentelemetry::layer().with_tracer(provider.tracer("strands-agents"));
        let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(layer));

        let seen: Arc<std::sync::Mutex<Vec<(Option<String>, Option<String>)>>> = Arc::default();
        let samples = Arc::clone(&seen);
        let tool = mock_tool("probe", move |context| {
            samples
                .lock()
                .expect("the samples")
                .push((tenant(), entry("ambient")));
            ToolResultBlock::new(
                context.tool_use().tool_use_id(),
                ToolResultStatus::Success,
                Vec::new(),
            )
        });
        let mut agent = tool_agent(Arc::new(tool), &[("tenant.id", "acme")]);
        let ambient = Context::current().with_baggage([KeyValue::new("ambient", "up")]);
        agent
            .invoke("Hi")
            .with_context(ambient)
            .await
            .expect("the run succeeds");

        assert_eq!(
            *seen.lock().expect("the samples"),
            [(Some("acme".to_owned()), Some("up".to_owned()))]
        );
        let spans = recorder.spans();
        let agent_span = named(&spans, "invoke_agent Strands Agent")[0];
        assert_eq!(attribute(agent_span, "tenant.id").as_deref(), Some("acme"));
        assert_eq!(tenant(), None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn isolates_baggage_between_interleaved_concurrent_agents() {
        // Each tool samples the baggage before and after yielding to the other agent.
        let both_sampled = Arc::new(tokio::sync::Barrier::new(2));
        let sampler = |name: &str, seen: &Seen| -> Arc<dyn crate::tools::Tool> {
            let seen = Arc::clone(seen);
            let both_sampled = Arc::clone(&both_sampled);
            Arc::new(stream_tool(name, move |context| {
                let seen = Arc::clone(&seen);
                let both_sampled = Arc::clone(&both_sampled);
                ToolStream::from_future(async move {
                    seen.lock().expect("the samples").push(tenant());
                    both_sampled.wait().await;
                    seen.lock().expect("the samples").push(tenant());
                    ToolResultBlock::new(
                        context.tool_use().tool_use_id(),
                        ToolResultStatus::Success,
                        Vec::new(),
                    )
                    .into()
                })
            }))
        };
        let (seen_a, seen_b) = (Seen::default(), Seen::default());
        let mut a = tool_agent(sampler("tool_a", &seen_a), &[("tenant.id", "acme")]);
        let mut b = tool_agent(sampler("tool_b", &seen_b), &[("tenant.id", "globex")]);
        let (result_a, result_b) = tokio::join!(a.invoke("Hi"), b.invoke("Hi"));
        result_a.expect("agent a succeeds");
        result_b.expect("agent b succeeds");
        let twice = |tenant: &str| vec![Some(tenant.to_owned()); 2];
        assert_eq!(seen_by(&seen_a), twice("acme"));
        assert_eq!(seen_by(&seen_b), twice("globex"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn lets_a_nested_agent_override_baggage_for_its_scope_and_restores_the_outer_value_after() {
        let seen = Seen::default();
        let inner = Arc::new(tokio::sync::Mutex::new(tool_agent(
            probe("inner_probe", &seen),
            &[("tenant.id", "inner-corp")],
        )));
        let samples = Arc::clone(&seen);
        let outer_probe = stream_tool("outer_probe", move |context| {
            let samples = Arc::clone(&samples);
            let inner = Arc::clone(&inner);
            ToolStream::from_future(async move {
                samples.lock().expect("the samples").push(tenant());
                inner
                    .lock()
                    .await
                    .invoke("Hi")
                    .await
                    .expect("the inner run succeeds");
                samples.lock().expect("the samples").push(tenant());
                ToolResultBlock::new(
                    context.tool_use().tool_use_id(),
                    ToolResultStatus::Success,
                    Vec::new(),
                )
                .into()
            })
        });
        let mut outer = tool_agent(Arc::new(outer_probe), &[("tenant.id", "outer-corp")]);
        outer.invoke("Hi").await.expect("the outer run succeeds");
        assert_eq!(
            seen_by(&seen),
            ["outer-corp", "inner-corp", "outer-corp"].map(|tenant| Some(tenant.to_owned()))
        );
    }

    // The TS rows stop the generator with `throw` and with `return`; a Rust consumer drops the
    // stream, and the hooks the drop skipped run in `shutdown`.
    #[tokio::test(flavor = "current_thread")]
    async fn keeps_baggage_current_for_invocation_cleanup_when_the() {
        let seen = Seen::default();
        let mut agent = tool_agent(probe("probe", &Seen::default()), &[("tenant.id", "acme")]);
        let samples = Arc::clone(&seen);
        agent.add_hook(move |_: &mut AfterInvocationEvent<'_>| {
            samples.lock().expect("the samples").push(tenant());
        });
        // Stop inside the model call so the invocation's cleanup runs `AfterInvocationEvent`.
        stream_until(&mut agent, |event| {
            matches!(event, AgentStreamEvent::BeforeModelCall(_))
        })
        .await;
        agent.shutdown().await.expect("the cleanup succeeds");
        assert_eq!(seen_by(&seen), [Some("acme".to_owned())]);
    }
}
