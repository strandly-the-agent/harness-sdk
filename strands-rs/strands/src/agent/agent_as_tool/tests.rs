//! Ported from strands-ts/src/agent/__tests__/{agent-as-tool, agent-as-tool.invocation-context,
//! agent-as-tool.invocation-state}.test.ts.

use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use futures::StreamExt;
use serde_json::json;

use crate::agent::{Agent, AgentAsTool, AgentAsToolOptions};
use crate::error::{Error, ModelError};
use crate::hooks::{AfterToolCallEvent, BeforeModelCallEvent};
use crate::models::streaming::Usage;
use crate::test_support::stream_tool;
use crate::testing::{MockMessageModel, MockTurnOptions};
use crate::tools::{Tool, ToolContext, ToolOutcome, ToolStream, ToolStreamEvent, ToolStreamItem};
use crate::types::agent::{InvocationState, InvokeLimits, InvokeOptions, LocalAgent};
use crate::types::json::{JsonObject, JsonValue};
use crate::types::messages::{
    ContentBlock, Message, Role, StopReason, TextBlock, ToolResultBlock, ToolUseBlock, limit_stop_message,
};
use crate::types::tools::{ToolResultStatus, ToolUse};

use super::DELEGATION_DESCRIPTION_SUFFIX;

fn text(text: &str) -> ContentBlock {
    TextBlock::new(text).into()
}

fn model(reply: &str) -> MockMessageModel {
    let model = MockMessageModel::new();
    model.add_turn(vec![text(reply)]);
    model
}

fn agent(name: &str, description: Option<&str>) -> Agent {
    Agent::builder()
        .model(model("Hi"))
        .name(name)
        .maybe_description(description)
        .build()
        .unwrap()
}

/// TS `createMockContext`.
fn context(name: &str, tool_use_id: &str, input: JsonValue) -> ToolContext {
    let parent = LocalAgent::new(Arc::new(MockMessageModel::new()));
    ToolContext::new(
        ToolUse::new(name, tool_use_id, input),
        parent.tool_agent(),
        InvocationState::new(),
    )
}

async fn collect(stream: ToolStream) -> (Vec<ToolStreamEvent>, ToolResultBlock) {
    let mut events = Vec::new();
    let mut result = None;
    let mut stream = stream;
    while let Some(item) = stream.next().await {
        match item {
            ToolStreamItem::Event(event) => events.push(event),
            ToolStreamItem::Outcome(ToolOutcome::Result(last)) => result = Some(last),
            other => panic!("unexpected item {other:?}"),
        }
    }
    (events, result.unwrap())
}

fn content_json(result: &ToolResultBlock) -> JsonValue {
    serde_json::to_value(result.content()).unwrap()
}

fn message_count(tool: &AgentAsTool) -> usize {
    tool.with_agent(|agent| agent.messages().len()).unwrap()
}

#[test]
fn uses_agent_name_as_default_tool_name() {
    let tool = agent("researcher", None)
        .as_tool(AgentAsToolOptions::default())
        .unwrap();
    assert_eq!(tool.name(), "researcher");
}

#[test]
fn allows_overriding_the_tool_name() {
    let options = AgentAsToolOptions::builder().name("research-tool").build();
    let tool = agent("researcher", None).as_tool(options).unwrap();
    assert_eq!(tool.name(), "research-tool");
}

#[test]
fn uses_agent_description_as_default_tool_description() {
    let tool = agent("researcher", Some("Finds information"))
        .as_tool(AgentAsToolOptions::default())
        .unwrap();
    assert_eq!(tool.description(), "Finds information");
}

#[test]
fn falls_back_to_generic_description_when_agent_has_no_description() {
    let tool = agent("researcher", None)
        .as_tool(AgentAsToolOptions::default())
        .unwrap();
    assert_eq!(
        tool.description(),
        "Use the researcher agent by providing a natural language input"
    );
}

#[test]
fn allows_overriding_the_tool_description() {
    let options = AgentAsToolOptions::builder()
        .description("Custom description")
        .build();
    let tool = agent("researcher", None).as_tool(options).unwrap();
    assert_eq!(tool.description(), "Custom description");
}

#[test]
fn exposes_the_wrapped_agent_via_getter() {
    let tool = agent("researcher", None)
        .as_tool(AgentAsToolOptions::default())
        .unwrap();
    assert_eq!(
        tool.with_agent(|agent| agent.name().to_owned()).as_deref(),
        Some("researcher")
    );
}

#[test]
fn has_correct_tool_spec_shape() {
    let tool = agent("researcher", Some("Finds info"))
        .as_tool(AgentAsToolOptions::default())
        .unwrap();
    assert_eq!(
        serde_json::to_value(tool.tool_spec()).unwrap(),
        json!({
            "name": "researcher",
            "description": "Finds info",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "input": {"type": "string", "description": "The natural language input to send to the agent."}
                },
                "required": ["input"]
            }
        })
    );
}

#[test]
fn throws_when_registered_with_agent_name_containing_spaces() {
    let sub = agent("Strands Agent", None);
    let error = Agent::builder()
        .model(model("Hi"))
        .tools([sub])
        .build()
        .unwrap_err();
    assert!(matches!(error, Error::ToolValidation(_)));
}

#[test]
fn throws_when_registered_with_explicit_name_containing_invalid_characters() {
    let options = AgentAsToolOptions::builder().name("has spaces").build();
    let tool = agent("researcher", None).as_tool(options).unwrap();
    let error = Agent::builder()
        .model(model("Hi"))
        .tools([tool])
        .build()
        .unwrap_err();
    assert!(matches!(error, Error::ToolValidation(_)));
}

#[test]
fn accepts_valid_name_with_hyphens_and_underscores() {
    let tool = agent("my_research-agent", None)
        .as_tool(AgentAsToolOptions::default())
        .unwrap();
    assert_eq!(tool.name(), "my_research-agent");
    Agent::builder().model(model("Hi")).tools([tool]).build().unwrap();
}

fn wrapped(reply: &str) -> AgentAsTool {
    Agent::builder()
        .model(model(reply))
        .name("test-agent")
        .build()
        .unwrap()
        .as_tool(AgentAsToolOptions::default())
        .unwrap()
}

#[tokio::test]
async fn invokes_the_wrapped_agent_and_returns_text_result() {
    let tool = wrapped("Agent response");

    let (_, result) =
        collect(tool.stream(context("test-agent", "tool-1", json!({"input": "Hello agent"})))).await;

    assert_eq!(result.tool_use_id(), "tool-1");
    assert_eq!(*result.status(), ToolResultStatus::Success);
    assert_eq!(content_json(&result), json!([{"text": "Agent response"}]));
}

#[tokio::test]
async fn yields_tool_stream_events_wrapping_sub_agent_events() {
    let tool = wrapped("Hello");

    let (events, _) = collect(tool.stream(context("test-agent", "tool-1", json!({"input": "Hi"})))).await;

    assert!(!events.is_empty());
    let types: Vec<&str> = events
        .iter()
        .filter_map(|event| event.data()?["type"].as_str())
        .collect();
    assert_eq!(types.len(), events.len());
    assert!(types.contains(&"modelStreamUpdateEvent"));
    assert_eq!(types.last(), Some(&"agentResultEvent"));
}

#[tokio::test]
async fn unwraps_tool_stream_update_event_by_yielding_inner_tool_stream_event_directly() {
    let streaming = stream_tool("streaming-tool", |context| {
        let id = context.tool_use().tool_use_id().to_owned();
        ToolStream::new(async_stream::stream! {
            yield ToolStreamEvent::new(json!("progress-1")).into();
            yield ToolStreamEvent::new(json!("progress-2")).into();
            yield ToolOutcome::Result(ToolResultBlock::new(id, ToolResultStatus::Success, vec![])).into();
        })
    });
    let sub_model = MockMessageModel::new();
    sub_model.add_turn(vec![
        ToolUseBlock::new("streaming-tool", "sub-tool-1", json!({})).into(),
    ]);
    sub_model.add_turn(vec![text("Final response")]);
    let tool = Agent::builder()
        .model(sub_model)
        .name("test-agent")
        .tools([streaming])
        .build()
        .unwrap()
        .as_tool(AgentAsToolOptions::default())
        .unwrap();

    let (events, _) = collect(tool.stream(context(
        "test-agent",
        "outer-tool-1",
        json!({"input": "Do something"}),
    )))
    .await;

    let progress: Vec<&JsonValue> = events
        .iter()
        .filter_map(ToolStreamEvent::data)
        .filter(|data| data.is_string())
        .collect();
    assert_eq!(progress, [&json!("progress-1"), &json!("progress-2")]);
}

#[tokio::test]
async fn returns_error_result_on_agent_failure() {
    let failing = MockMessageModel::new();
    failing.add_error(|| ModelError::new("Model failed").into());
    let tool = Agent::builder()
        .model(failing)
        .name("test-agent")
        .no_retry_strategy()
        .build()
        .unwrap()
        .as_tool(AgentAsToolOptions::default())
        .unwrap();

    let (_, result) = collect(tool.stream(context("test-agent", "tool-1", json!({"input": "Hello"})))).await;

    assert_eq!(result.tool_use_id(), "tool-1");
    assert_eq!(*result.status(), ToolResultStatus::Error);
    assert_eq!(content_json(&result), json!([{"text": "Error: Model failed"}]));
}

#[tokio::test]
async fn returns_error_result_when_agent_is_already_busy() {
    let tool = wrapped("Slow response");

    let mut first = tool.stream(context("test-agent", "tool-1", json!({"input": "First call"})));
    first.next().await.unwrap();

    let (_, result) =
        collect(tool.stream(context("test-agent", "tool-2", json!({"input": "Second call"})))).await;

    assert_eq!(result.tool_use_id(), "tool-2");
    assert_eq!(*result.status(), ToolResultStatus::Error);
    assert_eq!(
        content_json(&result),
        json!([{"text": "Error: Agent 'test-agent' is already processing a request"}])
    );
    let (_, first) = collect(first).await;
    assert_eq!(*first.status(), ToolResultStatus::Success);
}

#[tokio::test]
async fn resets_agent_state_between_invocations_when_false_default() {
    let tool = wrapped("Response");

    collect(tool.stream(context("test-agent", "tool-1", json!({"input": "Hello"})))).await;
    let after_first = message_count(&tool);
    collect(tool.stream(context("test-agent", "tool-2", json!({"input": "Hello again"})))).await;

    assert_eq!(after_first, 2);
    assert_eq!(message_count(&tool), after_first);
}

#[tokio::test]
async fn preserves_agent_state_across_invocations_when_true() {
    let options = AgentAsToolOptions::builder().preserve_context(true).build();
    let tool = Agent::builder()
        .model(model("Response"))
        .name("test-agent")
        .build()
        .unwrap()
        .as_tool(options)
        .unwrap();

    collect(tool.stream(context("test-agent", "tool-1", json!({"input": "Hello"})))).await;
    let after_first = message_count(&tool);
    collect(tool.stream(context("test-agent", "tool-2", json!({"input": "Hello again"})))).await;

    assert!(message_count(&tool) > after_first);
}

#[tokio::test]
async fn snapshots_at_construction_time_not_first_invocation() {
    let tool = wrapped("Response");
    tool.with_agent(|agent| {
        agent
            .messages_mut()
            .push(Message::new(Role::User, vec![text("Direct invocation")]));
        agent.app_state().set("touched", true).unwrap();
    });

    collect(tool.stream(context("test-agent", "tool-1", json!({"input": "Hello"})))).await;
    let after_first = message_count(&tool);
    collect(tool.stream(context("test-agent", "tool-2", json!({"input": "Hello again"})))).await;

    assert_eq!(after_first, 2);
    assert_eq!(message_count(&tool), after_first);
    assert_eq!(
        tool.with_agent(|agent| agent.app_state().get_value("touched")),
        Some(None)
    );
}

mod session_manager_validation {
    use super::*;
    use crate::session::{SessionManager, SessionManagerConfig};
    use crate::storage::InMemoryStorage;

    const CONFLICT: &str = "Agent 'test-agent' has a SessionManager, which conflicts with preserveContext=false. \
        The SessionManager persists conversation history externally, but preserveContext=false resets state \
        between invocations. Use preserveContext=true or remove the SessionManager.";

    fn session_agent() -> Agent {
        let session_manager = SessionManager::new(SessionManagerConfig {
            storage: Some(Arc::new(InMemoryStorage::new())),
            ..Default::default()
        })
        .unwrap();
        Agent::builder()
            .model(model("Hi"))
            .name("test-agent")
            .session_manager(session_manager)
            .build()
            .unwrap()
    }

    #[test]
    fn throws_when_preserve_context_is_false_and_agent_has_a_session_manager() {
        let error = session_agent()
            .as_tool(AgentAsToolOptions::default())
            .unwrap_err();
        assert_eq!(error.to_string(), CONFLICT);
    }

    #[test]
    fn throws_when_preserve_context_is_explicitly_false_and_agent_has_a_session_manager() {
        let options = AgentAsToolOptions::builder().preserve_context(false).build();
        let error = session_agent().as_tool(options).unwrap_err();
        assert_eq!(error.to_string(), CONFLICT);
    }

    #[test]
    fn allows_preserve_context_true_with_session_manager() {
        let options = AgentAsToolOptions::builder().preserve_context(true).build();
        session_agent().as_tool(options).unwrap();
    }

    /// TS wraps an agent in `tools` with `new AgentAsTool({ agent })`, which throws the same way.
    #[test]
    fn an_agent_with_a_session_manager_in_tools_fails_the_build() {
        let error = Agent::builder()
            .model(model("Hi"))
            .tools([session_agent()])
            .build()
            .unwrap_err();
        assert_eq!(error.to_string(), CONFLICT);
    }
}

#[test]
fn returns_an_agent_as_tool_instance() {
    let tool = agent("researcher", None)
        .as_tool(AgentAsToolOptions::default())
        .unwrap();
    assert_eq!(tool.name(), "researcher");
    assert!(!tool.delegate());
}

#[test]
fn passes_options_through_to_agent_as_tool() {
    let options = AgentAsToolOptions::builder()
        .name("custom-name")
        .description("Custom desc")
        .build();
    let tool = agent("researcher", None).as_tool(options).unwrap();
    assert_eq!(tool.name(), "custom-name");
    assert_eq!(tool.description(), "Custom desc");
}

#[test]
fn sets_delegate_and_appends_description_suffix_when_delegate_is_true() {
    let options = AgentAsToolOptions::builder().delegate(true).build();
    let tool = agent("specialist", Some("Handles billing"))
        .as_tool(options)
        .unwrap();

    let expected = format!("Handles billing{DELEGATION_DESCRIPTION_SUFFIX}");
    assert!(tool.delegate());
    assert_eq!(tool.description(), expected);
    assert_eq!(tool.tool_spec().description(), expected);
}

#[test]
fn does_not_set_delegate_or_append_description_suffix_when_delegate_is_false() {
    let options = AgentAsToolOptions::builder().delegate(false).build();
    let tool = agent("specialist", Some("Handles billing"))
        .as_tool(options)
        .unwrap();

    assert!(!tool.delegate());
    assert_eq!(tool.description(), "Handles billing");
}

#[test]
fn auto_wraps_agent_as_agent_as_tool_when_passed_in_tools_array() {
    let sub = agent("sub-agent", Some("A sub agent"));
    let parent = Agent::builder().model(model("Hi")).tools([sub]).build().unwrap();

    let registered = parent.tool_registry().get("sub-agent").unwrap();
    assert_eq!(registered.name(), "sub-agent");
    assert_eq!(registered.description(), "A sub agent");
    assert_eq!(
        registered.tool_spec().input_schema().unwrap()["required"],
        json!(["input"])
    );
}

#[test]
fn auto_wraps_agent_in_nested_tools_arrays() {
    let sub = agent("nested-agent", None);
    let parent = Agent::builder()
        .model(model("Hi"))
        .tools([vec![sub]])
        .build()
        .unwrap();

    assert!(parent.tool_registry().get("nested-agent").is_some());
}

fn usage(output: u64) -> MockTurnOptions {
    MockTurnOptions {
        usage: Some(Usage::new(0, output, output)),
        ..MockTurnOptions::default()
    }
}

fn inner_agent(output: u64) -> Agent {
    let inner_model = MockMessageModel::new();
    inner_model.add_turn_with(vec![text("inner-done")], usage(output));
    Agent::builder()
        .model(inner_model)
        .name("inner")
        .description("inner agent")
        .build()
        .unwrap()
}

fn outer_model(turns: &[(Option<&str>, u64)]) -> MockMessageModel {
    let outer = MockMessageModel::new();
    for &(reply, output) in turns {
        let content = match reply {
            Some(reply) => text(reply),
            None => ToolUseBlock::new("inner", "tu-1", json!({"input": "hi"})).into(),
        };
        outer.add_turn_with(vec![content], usage(output));
    }
    outer
}

fn output_limit(tokens: u64) -> InvokeOptions {
    InvokeOptions {
        limits: Some(InvokeLimits {
            output_tokens: NonZeroU64::new(tokens),
            ..InvokeLimits::default()
        }),
        ..InvokeOptions::default()
    }
}

#[tokio::test]
async fn folds_the_inner_agent_model_usage_into_the_parent_request_total() {
    let inner = inner_agent(60);
    let seen: Arc<Mutex<Option<u64>>> = Arc::default();
    let sink = Arc::clone(&seen);
    inner.add_hook(move |event: &mut BeforeModelCallEvent<'_>| {
        *sink.lock().unwrap() = event
            .invocation_context()
            .map(|context| context.usage().tokens().output_tokens());
    });
    let mut outer = Agent::builder()
        .model(outer_model(&[(None, 60), (Some("outer-done"), 10)]))
        .tools([inner])
        .build()
        .unwrap();

    outer.invoke("run inner").await.unwrap();

    assert_eq!(*seen.lock().unwrap(), Some(60));
}

#[tokio::test]
async fn trips_a_limit_that_only_the_parent_plus_child_total_exceeds() {
    let mut outer = Agent::builder()
        .model(outer_model(&[(None, 60), (Some("outer-done"), 60)]))
        .tools([inner_agent(60)])
        .build()
        .unwrap();

    let result = outer.invoke_with("run inner", output_limit(100)).await.unwrap();

    assert_eq!(result.stop_reason(), StopReason::LimitOutputTokens);
}

#[tokio::test]
async fn returns_an_error_result_when_the_child_stops_on_the_shared_limit() {
    let mut outer = Agent::builder()
        .model(outer_model(&[(None, 60)]))
        .tools([inner_agent(0)])
        .build()
        .unwrap();
    let seen: Arc<Mutex<Option<ToolResultBlock>>> = Arc::default();
    let sink = Arc::clone(&seen);
    outer.add_hook(move |event: &mut AfterToolCallEvent| {
        *sink.lock().unwrap() = Some(event.result().clone());
    });

    let result = outer.invoke_with("run inner", output_limit(50)).await.unwrap();

    assert_eq!(result.stop_reason(), StopReason::LimitOutputTokens);
    let tool_result = seen.lock().unwrap().clone().unwrap();
    assert_eq!(tool_result.tool_use_id(), "tu-1");
    assert_eq!(*tool_result.status(), ToolResultStatus::Error);
    let message = limit_stop_message("inner", &StopReason::LimitOutputTokens);
    assert_eq!(
        content_json(&tool_result),
        json!([{"text": format!("Error: {message}")}])
    );
    assert!(tool_result.error().is_some());
}

#[derive(Clone, Debug, PartialEq)]
struct UserId(&'static str);

#[derive(Clone, Debug, PartialEq)]
struct InnerTouched(bool);

#[tokio::test]
async fn forwards_outer_invocation_state_into_the_wrapped_agent_and_reflects_inner_mutations_on_outer_result()
{
    let inner = inner_agent(0);
    let seen: Arc<Mutex<Option<UserId>>> = Arc::default();
    let sink = Arc::clone(&seen);
    inner.add_hook(move |event: &mut BeforeModelCallEvent<'_>| {
        *sink.lock().unwrap() = event.invocation_state().get::<UserId>();
        event.invocation_state().insert(InnerTouched(true));
    });
    let mut outer = Agent::builder()
        .model(outer_model(&[(None, 0), (Some("outer-done"), 0)]))
        .tools([inner])
        .build()
        .unwrap();
    let state = InvocationState::new();
    state.insert(UserId("u-1"));

    let options = InvokeOptions {
        invocation_state: state,
        ..InvokeOptions::default()
    };
    let result = outer.invoke_with("run inner", options).await.unwrap();

    assert_eq!(*seen.lock().unwrap(), Some(UserId("u-1")));
    assert_eq!(result.invocation_state().get::<UserId>(), Some(UserId("u-1")));
    assert_eq!(
        result.invocation_state().get::<InnerTouched>(),
        Some(InnerTouched(true))
    );
}

#[tokio::test]
async fn agent_tool_streams_are_send_and_the_agent_comes_back_after_a_dropped_call() {
    fn assert_send<T: Send>(_: &T) {}
    let tool = wrapped("Response");
    let mut stream = tool.stream(context("test-agent", "tool-1", json!({"input": "Hello"})));
    assert_send(&stream);
    stream.next().await.unwrap();
    assert!(tool.with_agent(|_| ()).is_none());
    drop(stream);
    assert!(tool.with_agent(|_| ()).is_some());
}
mod interrupt;

/// The cancel-signal cases of agent-as-tool.test.ts.
mod cancel {
    use tokio::sync::Notify;

    use super::*;
    use crate::cancel::{CancelHandle, CancelOptions, CancelSignal};
    use crate::hooks::InitializedEvent;
    use crate::tools::FunctionTool;
    use crate::tools::ToolError;

    /// A context whose execution signal the returned handle cancels.
    fn cancellable_context(name: &str, tool_use_id: &str, input: JsonValue) -> (ToolContext, CancelHandle) {
        let (signal, handle) = CancelSignal::new();
        let mut parent = LocalAgent::new(Arc::new(MockMessageModel::new()));
        parent.begin_cancel(Some(&signal));
        let context = ToolContext::new(
            ToolUse::new(name, tool_use_id, input),
            parent.tool_agent(),
            InvocationState::new(),
        );
        (context, handle)
    }

    fn cancelling_agent(tool: &'static str, options: CancelOptions) -> Agent {
        let model = MockMessageModel::new();
        model
            .add_turn(vec![ToolUseBlock::new(tool, "t1", json!({})).into()])
            .add_turn(vec![text("unreachable")]);
        let reply = options.message.clone().unwrap_or_else(|| "aborting".to_owned());
        let cancelling = FunctionTool::new(tool, "cancels", move |_input: JsonValue, ctx: ToolContext| {
            ctx.agent().cancel(options.clone());
            let reply = reply.clone();
            async move { Ok::<_, ToolError>(reply) }
        });
        Agent::builder()
            .model(model)
            .name("test-agent")
            .tools([cancelling])
            .printer(false)
            .build()
            .unwrap()
    }

    fn cancelled_result() -> JsonValue {
        json!([{"text": "Error: Agent 'test-agent' cancelled"}])
    }

    #[tokio::test]
    async fn returns_an_error_when_the_execution_signal_cancels_the_wrapped_agent() {
        let agent = Agent::builder()
            .model(model("Agent response"))
            .name("test-agent")
            .printer(false)
            .build()
            .unwrap();
        let (context, handle) = cancellable_context("test-agent", "tool-1", json!({"input": "Hello agent"}));
        agent.add_hook(move |_: &mut BeforeModelCallEvent<'_>| handle.cancel(CancelOptions::default()));
        let tool = agent.as_tool(AgentAsToolOptions::default()).unwrap();

        let (_, result) = collect(tool.stream(context)).await;

        assert_eq!(*result.status(), ToolResultStatus::Error);
        assert_eq!(content_json(&result), cancelled_result());
    }

    // No TS case: TS forwards only the `AbortSignal`, so a parent's deferral never reaches the child.
    #[tokio::test]
    async fn a_deferred_parent_cancel_lets_the_wrapped_agent_finish() {
        let model = MockMessageModel::new();
        model
            .add_turn(vec![ToolUseBlock::new("step", "c1", json!({})).into()])
            .add_turn(vec![text("child final answer")]);
        let (context, handle) = cancellable_context("test-agent", "tool-1", json!({"input": "go"}));
        let step = FunctionTool::new("step", "a step", move |_input: JsonValue, _ctx: ToolContext| {
            handle.cancel(CancelOptions {
                message: Some("parent stop".to_owned()),
                after_current_tools: true,
            });
            async { Ok::<_, ToolError>("stepped") }
        });
        let agent = Agent::builder()
            .model(model)
            .name("test-agent")
            .tools([step])
            .printer(false)
            .build()
            .unwrap();
        let tool = agent.as_tool(AgentAsToolOptions::default()).unwrap();

        let (_, result) = collect(tool.stream(context)).await;

        assert_eq!(*result.status(), ToolResultStatus::Success);
        assert_eq!(content_json(&result), json!([{"text": "child final answer"}]));
    }

    #[tokio::test]
    async fn returns_a_success_result_when_the_wrapped_agent_ends_via_a_deferred_cancel_stop_tool() {
        let options = CancelOptions {
            message: Some("Answer is 42".to_owned()),
            after_current_tools: true,
        };
        let tool = cancelling_agent("stop", options)
            .as_tool(AgentAsToolOptions::default())
            .unwrap();

        let (_, result) =
            collect(tool.stream(context("test-agent", "tool-1", json!({"input": "Hello agent"})))).await;

        assert_eq!(*result.status(), ToolResultStatus::Success);
        assert_eq!(content_json(&result), json!([{"text": "Answer is 42"}]));
    }

    #[tokio::test]
    async fn returns_an_error_when_the_wrapped_agent_is_cancelled_immediately_from_a_tool() {
        let tool = cancelling_agent("abort", CancelOptions::default())
            .as_tool(AgentAsToolOptions::default())
            .unwrap();

        let (_, result) =
            collect(tool.stream(context("test-agent", "tool-1", json!({"input": "Hello agent"})))).await;

        assert_eq!(*result.status(), ToolResultStatus::Error);
        assert_eq!(content_json(&result), cancelled_result());
    }

    // TS delays a plugin's `initAgent`; an async `InitializedEvent` hook holds initialization the same way.
    #[tokio::test]
    async fn does_not_persist_cancellation_when_the_signal_aborts_during_initialization() {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let agent = Agent::builder()
            .model(model("Agent response"))
            .name("test-agent")
            .printer(false)
            .build()
            .unwrap();
        let (hook_started, hook_release) = (Arc::clone(&started), Arc::clone(&release));
        agent.add_async_hook::<InitializedEvent<'static>>(move |_event| {
            let (started, release) = (Arc::clone(&hook_started), Arc::clone(&hook_release));
            Box::pin(async move {
                started.notify_one();
                release.notified().await;
            })
        });
        let tool = Arc::new(
            agent
                .as_tool(AgentAsToolOptions::builder().preserve_context(true).build())
                .unwrap(),
        );
        let (context, handle) = cancellable_context("test-agent", "tool-1", json!({"input": "Hello agent"}));

        let execution = tokio::spawn(collect(tool.stream(context)));
        started.notified().await;
        handle.cancel(CancelOptions::default());
        release.notify_one();
        let (_, result) = execution.await.unwrap();

        assert_eq!(content_json(&result), cancelled_result());
        assert_eq!(message_count(&tool), 0);
    }
}
