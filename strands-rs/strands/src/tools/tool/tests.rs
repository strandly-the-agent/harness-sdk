//! Ported from strands-ts/src/tools/__tests__/tool.test.ts (`isValidToolName`, `ToolStreamEvent`,
//! and the async generator cases, which map to a [`Tool`] that streams), plus the `ToolStream`,
//! `ToolError` and `ToolContext` contracts of the design.

use std::sync::Arc;

use futures::{StreamExt, stream};
use serde_json::json;

use super::*;
use crate::agent::invocation_context::InvocationContext;
use crate::agent::tool_agent::ToolAgent;
use crate::interrupt::{InterruptParams, InterruptSource};
use crate::tools::context::{ToolContext, ToolInterrupter};
use crate::tools::fixtures::mock_context;
use crate::tools::function_tool::FunctionTool;
use crate::tools::tool_source::ToolSource;

fn tool_use(name: &str, id: &str) -> ToolUse {
    ToolUse::new(name, id, json!({}))
}

fn text_of(result: &ToolResultBlock) -> JsonValue {
    serde_json::to_value(&result.content).unwrap()
}

fn assert_send_sync<T: Send + Sync>() {}

/// A tool streaming progress and then an outcome, the Rust form of a TS async generator callback.
struct Progress {
    steps: Vec<JsonValue>,
    fail: bool,
}

impl Tool for Progress {
    fn name(&self) -> &str {
        "progress"
    }

    fn description(&self) -> &str {
        "Streams progress"
    }

    fn tool_spec(&self) -> ToolSpec {
        ToolSpec::new(self.name(), self.description())
    }

    fn stream(&self, context: ToolContext) -> ToolStream {
        let id = context.tool_use().tool_use_id.clone();
        let events: Vec<ToolStreamItem> = self
            .steps
            .iter()
            .map(|step| ToolStreamItem::Event(ToolStreamEvent::new().with_data(step.clone())))
            .collect();
        let outcome = if self.fail {
            ToolOutcome::from_error(id, ToolError::msg("Generator error"))
        } else {
            ToolOutcome::Result(ToolResultBlock::new(
                id,
                ToolResultStatus::Success,
                vec!["Final result".into()],
            ))
        };
        ToolStream::new(
            stream::iter(events).chain(stream::once(async move { ToolStreamItem::Outcome(outcome) })),
        )
    }
}

#[test]
fn accepts_valid_names() {
    for name in [
        "simple",
        "with_underscore",
        "with-hyphen",
        "Mixed-Case_123",
        "a",
        &"a".repeat(64),
    ] {
        assert!(is_valid_tool_name(name), "{name}");
    }
}

#[test]
fn rejects_invalid_names() {
    for name in [
        "",
        &"a".repeat(65),
        "has space",
        "has.dot",
        "has/slash",
        "has:colon",
        "emoji🚀",
    ] {
        assert!(!is_valid_tool_name(name), "{name}");
    }
}

#[test]
fn creates_instance_with_data() {
    let event = ToolStreamEvent::new().with_data(json!("test data"));
    assert_eq!(
        serde_json::to_value(&event).unwrap(),
        json!({ "type": "toolStreamEvent", "data": "test data" })
    );
}

#[test]
fn creates_instance_without_data() {
    assert_eq!(
        serde_json::to_value(ToolStreamEvent::new()).unwrap(),
        json!({ "type": "toolStreamEvent" })
    );
}

#[test]
fn creates_instance_with_structured_data() {
    let data = json!({ "progress": 50, "message": "Halfway complete", "nested": { "value": 123 } });
    let event = ToolStreamEvent::new().with_data(data.clone());
    let parsed: ToolStreamEvent =
        serde_json::from_value(json!({ "type": "toolStreamEvent", "data": data })).unwrap();
    assert_eq!(parsed, event);
    assert_eq!(
        event.data.as_ref().map(|data| &data["progress"]),
        Some(&json!(50))
    );
}

#[tokio::test]
async fn yields_tool_stream_events_then_final_tool_result() {
    let tool = Progress {
        steps: vec![json!("Starting..."), json!("Processing..."), json!("Complete!")],
        fail: false,
    };
    let tool_use = tool_use("progress", "test-gen-1");
    let (events, outcome) = tool
        .stream(mock_context(tool_use.clone()))
        .collect_outcome(&tool_use)
        .await;
    assert_eq!(
        serde_json::to_value(&events).unwrap(),
        json!([
            { "type": "toolStreamEvent", "data": "Starting..." },
            { "type": "toolStreamEvent", "data": "Processing..." },
            { "type": "toolStreamEvent", "data": "Complete!" },
        ])
    );
    let ToolOutcome::Result(result) = outcome else {
        panic!("expected a result")
    };
    assert_eq!(result.status, ToolResultStatus::Success);
    assert_eq!(text_of(&result), json!([{ "text": "Final result" }]));
}

#[tokio::test]
async fn can_yield_objects_as_tool_stream_events() {
    let tool = Progress {
        steps: vec![
            json!({ "progress": 0.25, "message": "Quarter done" }),
            json!({ "progress": 0.5, "message": "Halfway done" }),
            json!({ "progress": 1.0, "message": "Complete" }),
        ],
        fail: false,
    };
    let tool_use = tool_use("progress", "test-gen-2");
    let (events, outcome) = tool
        .stream(mock_context(tool_use.clone()))
        .collect_outcome(&tool_use)
        .await;
    assert_eq!(events.len(), 3);
    assert!(events.iter().all(|event| event.data.is_some()));
    assert!(matches!(outcome, ToolOutcome::Result(result) if result.status == ToolResultStatus::Success));
}

#[tokio::test]
async fn catches_errors_in_async_generators() {
    let tool = Progress {
        steps: vec![json!("Starting...")],
        fail: true,
    };
    let tool_use = tool_use("progress", "test-gen-error");
    let (events, outcome) = tool
        .stream(mock_context(tool_use.clone()))
        .collect_outcome(&tool_use)
        .await;
    assert_eq!(events.len(), 1);
    let ToolOutcome::Result(result) = outcome else {
        panic!("expected a result")
    };
    assert_eq!(result.status, ToolResultStatus::Error);
    assert_eq!(text_of(&result), json!([{ "text": "Error: Generator error" }]));
    assert_eq!(result.error().unwrap().to_string(), "Generator error");
}

#[tokio::test]
async fn a_stream_without_an_outcome_becomes_an_error_result() {
    let tool_use = tool_use("silent", "t-1");
    let stream = ToolStream::new(stream::iter([ToolStreamItem::Event(ToolStreamEvent::new())]));
    let items: Vec<ToolStreamItem> = stream.until_outcome(&tool_use).collect().await;
    assert_eq!(items.len(), 2);
    let ToolStreamItem::Outcome(ToolOutcome::Result(result)) = items[1].clone() else {
        panic!("expected an outcome")
    };
    assert_eq!(result.tool_use_id, "t-1");
    assert_eq!(result.status, ToolResultStatus::Error);
    assert_eq!(
        text_of(&result),
        json!([{ "text": "Tool 'silent' did not return a result" }])
    );
}

#[tokio::test]
async fn a_stream_ends_at_its_first_outcome() {
    let tool_use = tool_use("chatty", "t-1");
    let first = ToolResultBlock::new("t-1", ToolResultStatus::Success, vec!["first".into()]);
    let second = ToolResultBlock::new("t-1", ToolResultStatus::Success, vec!["second".into()]);
    let stream = ToolStream::new(stream::iter([
        ToolStreamItem::Outcome(first.clone().into()),
        ToolStreamItem::Event(ToolStreamEvent::new()),
        ToolStreamItem::Outcome(second.into()),
    ]));
    let items: Vec<ToolStreamItem> = stream.until_outcome(&tool_use).collect().await;
    assert_eq!(items, vec![ToolStreamItem::Outcome(ToolOutcome::Result(first))]);
}

#[tokio::test]
async fn from_future_yields_only_the_outcome() {
    let tool_use = tool_use("one", "t-1");
    let result = ToolResultBlock::new("t-1", ToolResultStatus::Success, vec!["done".into()]);
    let expected = result.clone();
    let (events, outcome) = ToolStream::from_future(async move { result.into() })
        .collect_outcome(&tool_use)
        .await;
    assert!(events.is_empty());
    assert_eq!(outcome, ToolOutcome::Result(expected));
}

#[test]
fn tool_error_converts_errors_and_interrupts_with_the_question_mark() {
    fn parse(text: &str) -> Result<u32, ToolError> {
        Ok(text.parse::<u32>()?)
    }
    fn pause(interrupt: Interrupt) -> Result<(), ToolError> {
        Err(interrupt)?
    }
    let error = parse("x").unwrap_err();
    assert_eq!(error.to_string(), "invalid digit found in string");
    assert!(error.interrupt().is_none());
    assert!(error.error().is_some());

    let error = pause(Interrupt::new("tool:t-1:confirm", "confirm")).unwrap_err();
    assert_eq!(
        error.interrupt().map(|interrupt| interrupt.name.as_str()),
        Some("confirm")
    );
    assert_eq!(error.to_string(), "Interrupt raised: confirm");
}

#[test]
fn from_error_keeps_the_interrupt_and_wraps_other_errors() {
    let interrupt = Interrupt::new("tool:t-1:confirm", "confirm");
    assert_eq!(
        ToolOutcome::from_error("t-1", interrupt.clone().into()),
        ToolOutcome::Interrupt(interrupt)
    );
    let ToolOutcome::Result(result) = ToolOutcome::from_error("t-1", ToolError::msg("boom")) else {
        panic!("expected a result")
    };
    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        json!({ "toolResult": { "toolUseId": "t-1", "status": "error", "content": [{ "text": "Error: boom" }] } })
    );
    assert_eq!(result.error().unwrap().to_string(), "boom");
}

#[test]
fn interrupt_raises_a_tool_interrupt_with_the_ts_id() {
    let context = mock_context(tool_use("ask", "t-9"));
    let params = InterruptParams::builder()
        .name("confirm")
        .reason(json!("Proceed?"))
        .build();
    let interrupt = context.interrupt(params).unwrap_err();
    assert_eq!(
        serde_json::to_value(&interrupt).unwrap(),
        json!({ "id": "tool:t-9:confirm", "name": "confirm", "reason": "Proceed?", "source": "tool" })
    );
    assert_eq!(interrupt.source, InterruptSource::Tool);
}

#[test]
fn interrupt_returns_a_preset_response() {
    let context = mock_context(tool_use("ask", "t-9"));
    let params = InterruptParams::builder()
        .name("confirm")
        .response(json!("yes"))
        .build();
    assert_eq!(context.interrupt(params).unwrap(), json!("yes"));
}

struct StoredResponses;

impl ToolInterrupter for StoredResponses {
    fn interrupt(&self, id: String, params: InterruptParams) -> Result<JsonValue, Interrupt> {
        match id.as_str() {
            "tool:t-9:confirm" => Ok(json!("stored")),
            _ => Err(Interrupt::new(id, params.name)),
        }
    }
}

#[test]
fn interrupt_returns_the_stored_response_on_resume() {
    let context = mock_context(tool_use("ask", "t-9")).with_interrupter(Arc::new(StoredResponses));
    assert_eq!(context.interrupt("confirm").unwrap(), json!("stored"));
    assert_eq!(context.interrupt("other").unwrap_err().id, "tool:t-9:other");
}

#[test]
fn tool_handles_are_send_and_sync() {
    assert_send_sync::<ToolContext>();
    assert_send_sync::<ToolAgent>();
    assert_send_sync::<ToolOutcome>();
    assert_send_sync::<ToolError>();
    assert_send_sync::<Arc<dyn Tool>>();
    fn assert_send<T: Send>() {}
    assert_send::<ToolStream>();
}

#[test]
fn the_context_exposes_the_call_and_its_request() {
    let request = InvocationContext::new();
    let context = mock_context(tool_use("ask", "t-1")).with_invocation_context(request.clone());
    assert_eq!(context.tool_use().tool_use_id, "t-1");
    assert_eq!(context.agent().id(), "mock-agent");
    assert!(!context.cancel_signal().is_cancelled());
    assert!(context.invocation_state().get::<u8>().is_none());
    assert!(context.invocation_context().unwrap().same_request(&request));
}

#[test]
fn tool_sources_flatten_in_order() {
    let make = |name: &'static str| FunctionTool::new(name, "A tool.", |_input, _ctx| async { Ok("ok") });
    let shared: Arc<dyn Tool> = Arc::new(make("b"));
    let sources = crate::tool_list![make("a"), Arc::clone(&shared), vec![make("c"), make("d")]];
    let names: Vec<String> = ToolSource::List(sources)
        .into_tools()
        .iter()
        .map(|tool| tool.name().to_owned())
        .collect();
    assert_eq!(names, ["a", "b", "c", "d"]);
}
