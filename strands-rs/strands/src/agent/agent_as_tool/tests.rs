//! Ported from strands-ts/src/agent/__tests__/agent-as-tool.test.ts,
//! strands-ts/src/agent/__tests__/agent-as-tool.invocation-context.test.ts and
//! strands-ts/src/agent/__tests__/agent-as-tool.invocation-state.test.ts
//!
//! The session-manager cases arrive with sessions (Phase 6), child interrupts and cancellation
//! with Phase 4. `preserve_context: false` restores the messages and app state captured when
//! the agent was wrapped (the TS `session` snapshot preset covers the same fields).

use std::sync::Arc;

use futures::StreamExt;
use serde_json::json;

use super::*;
use crate::agent::{AgentConfig, InvocationContext, InvocationState, InvokeLimits, InvokeOptions};
use crate::models::{Model, Usage};
use crate::testing::{MockModel, tool_context};
use crate::types::{ContentBlock, Role, ToolUseBlock};

fn child(model: &Arc<MockModel>, name: &str, description: Option<&str>) -> Agent {
    Agent::new(AgentConfig {
        model: Some((Arc::clone(model) as Arc<dyn Model>).into()),
        name: Some(name.into()),
        description: description.map(Into::into),
        printer: Some(false),
        ..Default::default()
    })
    .unwrap()
}

fn replying(text: &str) -> Arc<MockModel> {
    let model = Arc::new(MockModel::new());
    model.add_turn(vec![text.into()]);
    model
}

async fn run(tool: &AgentAsTool, context: ToolContext) -> (Vec<crate::JsonValue>, ToolResultBlock) {
    let mut stream = tool.stream(context);
    let mut events = Vec::new();
    while let Some(item) = stream.next().await {
        match item {
            ToolStreamItem::Event(event) => events.push(event.data.unwrap_or_default()),
            ToolStreamItem::Outcome(ToolOutcome::Result(result)) => return (events, result),
            ToolStreamItem::Outcome(other) => panic!("unexpected {other:?}"),
        }
    }
    panic!("no outcome")
}

fn text(result: &ToolResultBlock) -> String {
    serde_json::to_value(&result.content).unwrap()[0]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn call(tool: &AgentAsTool, input: &str) -> ToolContext {
    tool_context(tool.name(), "t1", json!({ "input": input }))
}

#[test]
fn uses_agent_name_as_default_tool_name() {
    let tool = child(&replying("x"), "researcher", None).as_tool(AgentAsToolOptions::default());
    assert_eq!(tool.name(), "researcher");
}

#[test]
fn allows_overriding_the_tool_name() {
    let options = AgentAsToolOptions {
        name: Some("custom".into()),
        ..Default::default()
    };
    assert_eq!(
        child(&replying("x"), "researcher", None).as_tool(options).name(),
        "custom"
    );
}

#[test]
fn uses_agent_description_as_default_tool_description() {
    let tool =
        child(&replying("x"), "researcher", Some("Finds things")).as_tool(AgentAsToolOptions::default());
    assert_eq!(tool.description(), "Finds things");
}

#[test]
fn falls_back_to_generic_description_when_agent_has_no_description() {
    let tool = child(&replying("x"), "researcher", None).as_tool(AgentAsToolOptions::default());
    assert_eq!(
        tool.description(),
        "Use the researcher agent by providing a natural language input"
    );
}

#[test]
fn allows_overriding_the_tool_description() {
    let options = AgentAsToolOptions {
        description: Some("Custom".into()),
        ..Default::default()
    };
    assert_eq!(
        child(&replying("x"), "r", Some("d"))
            .as_tool(options)
            .description(),
        "Custom"
    );
}

#[test]
fn has_correct_tool_spec_shape() {
    let spec = child(&replying("x"), "researcher", Some("Finds"))
        .as_tool(AgentAsToolOptions::default())
        .tool_spec();
    assert_eq!(
        serde_json::to_value(spec).unwrap(),
        json!({
            "name": "researcher",
            "description": "Finds",
            "inputSchema": {
                "type": "object",
                "properties": { "input": { "type": "string", "description": "The natural language input to send to the agent." } },
                "required": ["input"],
            },
        })
    );
}

#[test]
fn throws_when_registered_with_agent_name_containing_spaces() {
    let tool = child(&replying("x"), "my agent", None).as_tool(AgentAsToolOptions::default());
    let mut registry = crate::tools::ToolRegistry::new();
    assert!(registry.add(Arc::new(tool)).is_err());
}

#[test]
fn accepts_valid_name_with_hyphens_and_underscores() {
    let tool = child(&replying("x"), "my-agent_1", None).as_tool(AgentAsToolOptions::default());
    let mut registry = crate::tools::ToolRegistry::new();
    registry.add(Arc::new(tool)).unwrap();
}

#[tokio::test]
async fn invokes_the_wrapped_agent_and_returns_text_result() {
    let tool = child(&replying("Hello from child"), "child", None).as_tool(AgentAsToolOptions::default());
    let (_, result) = run(&tool, call(&tool, "Hi")).await;
    assert_eq!(result.status, ToolResultStatus::Success);
    assert_eq!(text(&result), "Hello from child");
}

#[tokio::test]
async fn yields_tool_stream_events_wrapping_sub_agent_events() {
    let tool = child(&replying("Hello"), "child", None).as_tool(AgentAsToolOptions::default());
    let (events, _) = run(&tool, call(&tool, "Hi")).await;
    assert_eq!(
        events.first().map(|event| event["type"].clone()),
        Some(json!("beforeInvocationEvent"))
    );
    assert!(
        events
            .iter()
            .any(|event| event["type"] == "modelStreamUpdateEvent")
    );
}

#[tokio::test]
async fn returns_error_result_on_agent_failure() {
    let model = Arc::new(MockModel::new());
    model.add_error(|| crate::error::ModelError::new("child failed").into());
    let mut agent = child(&model, "child", None);
    agent.retry_strategies.clear();
    let tool = agent.as_tool(AgentAsToolOptions::default());
    let (_, result) = run(&tool, call(&tool, "Hi")).await;
    assert_eq!(result.status, ToolResultStatus::Error);
    assert_eq!(text(&result), "Error: child failed");
}

#[tokio::test]
async fn returns_error_result_when_agent_is_already_busy() {
    let tool = child(&replying("x"), "child", None).as_tool(AgentAsToolOptions::default());
    let guard = tool.agent().lock().await;
    let (_, result) = run(&tool, call(&tool, "Hi")).await;
    drop(guard);
    assert_eq!(
        text(&result),
        "Error: Agent 'child' is already processing a request"
    );
}

#[tokio::test]
async fn resets_agent_state_between_invocations_when_false_default() {
    let tool = child(&replying("x"), "child", None).as_tool(AgentAsToolOptions::default());
    run(&tool, call(&tool, "one")).await;
    run(&tool, call(&tool, "two")).await;
    assert_eq!(tool.agent().lock().await.messages().len(), 2);
}

#[tokio::test]
async fn preserves_agent_state_across_invocations_when_true() {
    let options = AgentAsToolOptions {
        preserve_context: true,
        ..Default::default()
    };
    let tool = child(&replying("x"), "child", None).as_tool(options);
    run(&tool, call(&tool, "one")).await;
    run(&tool, call(&tool, "two")).await;
    assert_eq!(tool.agent().lock().await.messages().len(), 4);
}

#[tokio::test]
async fn snapshots_at_construction_time_not_first_invocation() {
    let agent = Agent::new(AgentConfig {
        model: Some((replying("x") as Arc<dyn Model>).into()),
        name: Some("child".into()),
        messages: vec![
            Message::new(Role::User, vec!["seed".into()]),
            Message::new(Role::Assistant, vec!["ok".into()]),
        ],
        printer: Some(false),
        ..Default::default()
    })
    .unwrap();
    let tool = agent.as_tool(AgentAsToolOptions::default());
    tool.agent().lock().await.messages_mut().clear();
    run(&tool, call(&tool, "one")).await;
    assert_eq!(tool.agent().lock().await.messages().len(), 4);
}

#[test]
fn sets_delegate_and_appends_description_suffix_when_delegate_is_true() {
    let options = AgentAsToolOptions {
        delegate: true,
        ..Default::default()
    };
    let tool = child(&replying("x"), "child", Some("Answers.")).as_tool(options);
    assert!(tool.delegate());
    assert_eq!(
        tool.description(),
        format!("Answers.{DELEGATION_DESCRIPTION_SUFFIX}")
    );
}

#[test]
fn does_not_set_delegate_or_append_description_suffix_when_delegate_is_false() {
    let tool = child(&replying("x"), "child", Some("Answers.")).as_tool(AgentAsToolOptions::default());
    assert!(!tool.delegate());
    assert_eq!(tool.description(), "Answers.");
}

#[tokio::test]
async fn auto_wraps_agent_as_agent_as_tool_when_passed_in_tools_array() {
    let parent_model = Arc::new(MockModel::new());
    parent_model.add_turn(vec![ContentBlock::ToolUse(ToolUseBlock::new(
        "child",
        "t1",
        json!({ "input": "hi" }),
    ))]);
    parent_model.add_turn(vec!["parent done".into()]);
    let mut parent = Agent::builder()
        .model(Arc::clone(&parent_model) as Arc<dyn Model>)
        .printer(false)
        .tools(crate::tool_list![vec![child(
            &replying("child answer"),
            "child",
            None
        )]])
        .build()
        .unwrap();
    let result = parent.invoke("go").await.unwrap();
    assert_eq!(result.to_string(), "parent done");
    let ContentBlock::ToolResult(tool_result) = &parent.messages()[2].content[0] else {
        panic!("tool result expected");
    };
    assert_eq!(text(tool_result), "child answer");
}

#[tokio::test]
async fn folds_the_inner_agent_model_usage_into_the_parent_request_total() {
    let model = Arc::new(MockModel::new());
    model.add_turn_with(vec!["x".into()], None, Some(Usage::new(5, 5, 10)));
    let tool = child(&model, "child", None).as_tool(AgentAsToolOptions::default());
    let context = InvocationContext::resolve(None, None).unwrap();
    let mut ctx = call(&tool, "hi");
    ctx.invocation_context = Some(context.clone());
    run(&tool, ctx).await;
    assert_eq!(context.usage().usage, Usage::new(5, 5, 10));
    assert_eq!(context.usage().turns_taken, 1);
}

#[tokio::test]
async fn returns_an_error_result_when_the_child_stops_on_the_shared_limit() {
    let context = InvocationContext::resolve(
        None,
        Some(InvokeLimits {
            turns: Some(1),
            ..Default::default()
        }),
    )
    .unwrap();
    context.record_turn();
    let tool = child(&replying("x"), "child", None).as_tool(AgentAsToolOptions::default());
    let mut ctx = call(&tool, "hi");
    ctx.invocation_context = Some(context);
    let (_, result) = run(&tool, ctx).await;
    assert_eq!(result.status, ToolResultStatus::Error);
    assert_eq!(
        text(&result),
        "Error: Agent 'child' stopped early (limitTurns): a limit on this call was reached, so its answer is incomplete"
    );
    let _ = InvokeOptions::default();
}

#[tokio::test]
async fn forwards_outer_invocation_state_into_the_wrapped_agent_and_reflects_inner_mutations_on_outer_result()
{
    #[derive(Clone, Debug, PartialEq)]
    struct Marker(&'static str);
    let agent = child(&replying("x"), "child", None);
    agent.add_hook(|event: &mut crate::hooks::BeforeInvocationEvent<'_>| {
        event.invocation_state().insert(Marker("inner"));
    });
    let tool = agent.as_tool(AgentAsToolOptions::default());
    let state = InvocationState::new();
    let mut ctx = call(&tool, "hi");
    ctx.invocation_state = state.clone();
    run(&tool, ctx).await;
    assert_eq!(state.get::<Marker>(), Some(Marker("inner")));
}
