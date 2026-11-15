//! Ported from strands-ts/src/agent/__tests__/agent-as-tool.test.ts,
//! agent-as-tool.invocation-context.test.ts and agent-as-tool.invocation-state.test.ts

#![expect(clippy::unwrap_used, clippy::panic, reason = "test code")]

use std::num::NonZeroU64;
use std::sync::Arc;

use futures::StreamExt;
use serde_json::{Value, json};
use strands::agent::{AgentAsTool, AgentAsToolOptions, DELEGATION_DESCRIPTION_SUFFIX, InvokeLimits};
use strands::hooks::BeforeModelCallEvent;
use strands::models::streaming::Usage;
use strands::testing::{MockModel, MockTurnOptions, test_agent, tool_context};
use strands::tools::{ToolSource, ToolStreamItem};
use strands::types::messages::{ToolResultBlock, ToolUseBlock};
use strands::types::tools::{ToolResultStatus, ToolUse};
use strands::{
    Agent, CancelOptions, CancelSignal, ContentBlock, InvokeOptions, StopReason, Tool, ToolOutcome,
};

fn child(name: &str, model: MockModel) -> Agent {
    test_agent(model).name(name).build().unwrap()
}

fn replying(text: &str) -> MockModel {
    let model = MockModel::new();
    model.add_turn(vec![ContentBlock::from(text)]);
    model
}

async fn run(tool: &AgentAsTool, context: strands::ToolContext) -> (Vec<Value>, ToolResultBlock) {
    let mut events = Vec::new();
    let mut stream = tool.stream(context);
    while let Some(item) = stream.next().await {
        match item {
            ToolStreamItem::Event(event) => events.push(event.data.unwrap_or_default()),
            ToolStreamItem::Outcome(ToolOutcome::Result(result)) => return (events, result),
            other => panic!("unexpected item {other:?}"),
        }
    }
    panic!("no result");
}

fn call(input: &str) -> ToolUse {
    ToolUse::new("test-agent", "tool-1", json!({ "input": input }))
}

fn text_of(result: &ToolResultBlock) -> Value {
    serde_json::to_value(&result.content).unwrap()
}

#[test]
fn uses_agent_name_as_default_tool_name() {
    let tool = child("my-agent", MockModel::new()).as_tool(AgentAsToolOptions::default());
    assert_eq!(tool.name(), "my-agent");
}

#[test]
fn allows_overriding_the_tool_name() {
    let tool = child("my-agent", MockModel::new()).as_tool(AgentAsToolOptions {
        name: Some("custom".into()),
        ..AgentAsToolOptions::default()
    });
    assert_eq!(tool.name(), "custom");
}

#[test]
fn uses_agent_description_as_default_tool_description() {
    let agent = test_agent(MockModel::new())
        .name("my-agent")
        .description("Researches topics")
        .build()
        .unwrap();
    assert_eq!(
        agent.as_tool(AgentAsToolOptions::default()).description(),
        "Researches topics"
    );
}

#[test]
fn falls_back_to_generic_description_when_agent_has_no_description() {
    let tool = child("my-agent", MockModel::new()).as_tool(AgentAsToolOptions::default());
    assert_eq!(
        tool.description(),
        "Use the my-agent agent by providing a natural language input"
    );
}

#[test]
fn allows_overriding_the_tool_description() {
    let tool = child("my-agent", MockModel::new()).as_tool(AgentAsToolOptions {
        description: Some("Custom".into()),
        ..AgentAsToolOptions::default()
    });
    assert_eq!(tool.description(), "Custom");
}

#[test]
fn has_correct_tool_spec_shape() {
    let tool = child("my-agent", MockModel::new()).as_tool(AgentAsToolOptions::default());
    assert_eq!(
        serde_json::to_value(tool.tool_spec()).unwrap(),
        json!({
            "name": "my-agent",
            "description": "Use the my-agent agent by providing a natural language input",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "input": { "type": "string", "description": "The natural language input to send to the agent." },
                },
                "required": ["input"],
            },
        })
    );
}

#[test]
fn throws_when_registered_with_agent_name_containing_spaces() {
    let tool = child("my agent", MockModel::new());
    let error = test_agent(MockModel::new()).tools([tool]).build().unwrap_err();
    assert!(matches!(error, strands::Error::ToolValidation(_)));
}

#[test]
fn accepts_valid_name_with_hyphens_and_underscores() {
    let tool = child("my-agent_1", MockModel::new());
    assert!(test_agent(MockModel::new()).tools([tool]).build().is_ok());
}

#[tokio::test]
async fn invokes_the_wrapped_agent_and_returns_text_result() {
    let tool = child("test-agent", replying("Agent response")).as_tool(AgentAsToolOptions::default());

    let (_, result) = run(&tool, tool_context(call("Hello agent")).build()).await;

    assert_eq!(result.tool_use_id, "tool-1");
    assert_eq!(result.status, ToolResultStatus::Success);
    assert_eq!(text_of(&result), json!([{ "text": "Agent response" }]));
}

#[tokio::test]
async fn returns_an_error_when_the_execution_signal_cancels_the_wrapped_agent() {
    let agent = child("test-agent", replying("Agent response"));
    let (signal, handle) = CancelSignal::new();
    agent.add_hook(move |_: &mut BeforeModelCallEvent<'_>| handle.cancel(CancelOptions::default()));
    let tool = agent.as_tool(AgentAsToolOptions::default());

    let (_, result) = run(
        &tool,
        tool_context(call("Hello agent")).cancel_signal(signal).build(),
    )
    .await;

    assert_eq!(result.status, ToolResultStatus::Error);
    assert_eq!(
        text_of(&result),
        json!([{ "text": "Error: Agent 'test-agent' cancelled" }])
    );
}

#[tokio::test]
async fn yields_tool_stream_events_wrapping_sub_agent_events() {
    let tool = child("test-agent", replying("Hi")).as_tool(AgentAsToolOptions::default());

    let (events, _) = run(&tool, tool_context(call("Hello")).build()).await;

    assert_eq!(events.first().unwrap()["type"], "beforeInvocationEvent");
    assert_eq!(events.last().unwrap()["type"], "agentResultEvent");
}

#[tokio::test]
async fn unwraps_tool_stream_update_event_by_yielding_inner_tool_stream_event_directly() {
    let model = MockModel::new();
    model.add_turn(vec![ToolUseBlock::new("progress", "inner-1", json!({})).into()]);
    model.add_turn(vec![ContentBlock::from("done")]);
    let progress = strands::tools::FunctionTool::new("progress", "Reports progress", |_, _| async move {
        Ok::<_, strands::ToolError>(json!("ok"))
    });
    let tool = test_agent(model)
        .name("test-agent")
        .tools([progress])
        .build()
        .unwrap()
        .as_tool(AgentAsToolOptions::default());

    let (events, result) = run(&tool, tool_context(call("Go")).build()).await;

    assert!(events.iter().any(|event| event["type"] == "toolResultEvent"));
    assert_eq!(text_of(&result), json!([{ "text": "done" }]));
}

#[tokio::test]
async fn returns_error_result_on_agent_failure() {
    let model = MockModel::new();
    model.add_error(|| strands::error::ModelError::new("Model failed").into());
    let agent = test_agent(model)
        .name("test-agent")
        .retry_strategy(Vec::new())
        .build()
        .unwrap();
    let tool = agent.as_tool(AgentAsToolOptions::default());

    let (_, result) = run(&tool, tool_context(call("Hello")).build()).await;

    assert_eq!(result.status, ToolResultStatus::Error);
    assert_eq!(text_of(&result), json!([{ "text": "Error: Model failed" }]));
}

#[tokio::test]
async fn returns_error_result_when_agent_is_already_busy() {
    let model = MockModel::with_sleeper(Arc::new(strands::retry::TokioSleeper));
    model.add_turn_with(
        vec![ContentBlock::from("slow")],
        MockTurnOptions {
            delay: Some(std::time::Duration::from_secs(60)),
            ..MockTurnOptions::default()
        },
    );
    let tool = Arc::new(child("test-agent", model).as_tool(AgentAsToolOptions::default()));
    let mut first = tool.stream(tool_context(call("first")).build());
    loop {
        tokio::select! {
            item = first.next() => assert!(!matches!(item, Some(ToolStreamItem::Outcome(_)) | None)),
            () = tokio::time::sleep(std::time::Duration::from_millis(20)) => break,
        }
    }

    let (_, result) = run(&tool, tool_context(call("second")).build()).await;

    assert_eq!(
        text_of(&result),
        json!([{ "text": "Error: Agent 'test-agent' is already processing a request" }])
    );
}

#[tokio::test]
async fn resets_agent_state_between_invocations_when_false_default() {
    let model = MockModel::new();
    model.add_turn(vec![ContentBlock::from("one")]);
    model.add_turn(vec![ContentBlock::from("two")]);
    let model = Arc::new(model);
    let tool = test_agent(Arc::clone(&model))
        .name("test-agent")
        .build()
        .unwrap()
        .as_tool(AgentAsToolOptions::default());

    run(&tool, tool_context(call("first")).build()).await;
    run(&tool, tool_context(call("second")).build()).await;

    assert_eq!(model.requests()[1].messages.len(), 1);
}

#[tokio::test]
async fn preserves_agent_state_across_invocations_when_true() {
    let model = MockModel::new();
    model.add_turn(vec![ContentBlock::from("one")]);
    model.add_turn(vec![ContentBlock::from("two")]);
    let model = Arc::new(model);
    let tool = test_agent(Arc::clone(&model))
        .name("test-agent")
        .build()
        .unwrap()
        .as_tool(AgentAsToolOptions {
            preserve_context: true,
            ..AgentAsToolOptions::default()
        });

    run(&tool, tool_context(call("first")).build()).await;
    run(&tool, tool_context(call("second")).build()).await;

    assert_eq!(model.requests()[1].messages.len(), 3);
}

#[tokio::test]
async fn snapshots_at_construction_time_not_first_invocation() {
    let model = MockModel::new();
    model.add_turn(vec![ContentBlock::from("one")]);
    model.add_turn(vec![ContentBlock::from("two")]);
    let agent = test_agent(model).name("test-agent").build().unwrap();
    agent.app_state().set("phase", "initial").unwrap();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let record = Arc::clone(&seen);
    agent.add_hook(move |event: &mut strands::hooks::BeforeInvocationEvent<'_>| {
        record
            .lock()
            .unwrap()
            .push(event.agent().app_state().get_value("phase"));
        event.agent().app_state().set("phase", "changed").unwrap();
    });
    let tool = agent.as_tool(AgentAsToolOptions::default());

    run(&tool, tool_context(call("first")).build()).await;
    run(&tool, tool_context(call("second")).build()).await;

    assert_eq!(
        *seen.lock().unwrap(),
        [Some(json!("initial")), Some(json!("initial"))]
    );
}

#[test]
fn sets_delegate_and_appends_description_suffix_when_delegate_is_true() {
    let tool = child("my-agent", MockModel::new()).as_tool(AgentAsToolOptions {
        delegate: true,
        ..AgentAsToolOptions::default()
    });
    assert!(tool.delegate());
    assert_eq!(
        tool.description(),
        format!(
            "Use the my-agent agent by providing a natural language input{DELEGATION_DESCRIPTION_SUFFIX}"
        )
    );
}

#[test]
fn does_not_set_delegate_or_append_description_suffix_when_delegate_is_false() {
    let tool = child("my-agent", MockModel::new()).as_tool(AgentAsToolOptions::default());
    assert!(!tool.delegate());
    assert!(!tool.description().ends_with(DELEGATION_DESCRIPTION_SUFFIX));
}

#[test]
fn auto_wraps_agent_as_agent_as_tool_when_passed_in_tools_array() {
    let parent = test_agent(MockModel::new())
        .tools([child("helper", MockModel::new())])
        .build()
        .unwrap();
    assert!(parent.tool_registry().get("helper").is_some());
}

#[test]
fn auto_wraps_agent_in_nested_tools_arrays() {
    let nested = ToolSource::from(vec![ToolSource::from(child("helper", MockModel::new()))]);
    let parent = test_agent(MockModel::new()).tools([nested]).build().unwrap();
    assert!(parent.tool_registry().get("helper").is_some());
}

#[tokio::test]
async fn folds_the_inner_agent_model_usage_into_the_parent_request_total() {
    let inner = MockModel::new();
    inner.add_turn_with(
        vec![ContentBlock::from("inner")],
        MockTurnOptions {
            usage: Some(Usage::new(3, 4, 7)),
            ..MockTurnOptions::default()
        },
    );
    let outer = MockModel::new();
    outer.add_turn_with(
        vec![ToolUseBlock::new("helper", "t-1", json!({ "input": "go" })).into()],
        MockTurnOptions {
            usage: Some(Usage::new(10, 5, 15)),
            ..MockTurnOptions::default()
        },
    );
    outer.add_turn_with(
        vec![ContentBlock::from("done")],
        MockTurnOptions {
            usage: Some(Usage::new(1, 1, 2)),
            ..MockTurnOptions::default()
        },
    );
    let mut parent = test_agent(outer).tools([child("helper", inner)]).build().unwrap();

    let result = parent.invoke("Hi").await.unwrap();

    assert_eq!(
        result.invocation_context().unwrap().usage().usage,
        Usage::new(14, 10, 24)
    );
}

#[tokio::test]
async fn returns_an_error_result_when_the_child_stops_on_the_shared_limit() {
    let inner = MockModel::new();
    inner.add_turn(vec![ContentBlock::from("inner")]);
    let outer = MockModel::new();
    outer.add_turn(vec![
        ToolUseBlock::new("helper", "t-1", json!({ "input": "go" })).into(),
    ]);
    outer.add_turn(vec![ContentBlock::from("done")]);
    let mut parent = test_agent(outer).tools([child("helper", inner)]).build().unwrap();
    let options = InvokeOptions {
        limits: Some(InvokeLimits {
            turns: NonZeroU64::new(1),
            ..InvokeLimits::default()
        }),
        ..InvokeOptions::default()
    };

    let result = parent.invoke_with("Hi", options).await.unwrap();

    assert_eq!(result.stop_reason(), StopReason::LimitTurns);
    let ContentBlock::ToolResult(tool_result) = &parent.messages()[2].content[0] else {
        panic!("expected a tool result");
    };
    assert_eq!(
        text_of(tool_result),
        json!([{ "text": "Error: Agent 'helper' stopped early (limitTurns): a limit on this call was reached, so its answer is incomplete" }])
    );
}

#[tokio::test]
async fn forwards_outer_invocation_state_into_the_wrapped_agent_and_reflects_inner_mutations_on_outer_result()
{
    #[derive(Clone, Debug, PartialEq)]
    struct Marker(&'static str);
    let inner = MockModel::new();
    inner.add_turn(vec![ContentBlock::from("inner")]);
    let helper = child("helper", inner);
    helper.add_hook(|event: &mut BeforeModelCallEvent<'_>| {
        event.invocation_state().insert(Marker("from inner"));
    });
    let outer = MockModel::new();
    outer.add_turn(vec![
        ToolUseBlock::new("helper", "t-1", json!({ "input": "go" })).into(),
    ]);
    outer.add_turn(vec![ContentBlock::from("done")]);
    let mut parent = test_agent(outer).tools([helper]).build().unwrap();

    let result = parent.invoke("Hi").await.unwrap();

    assert_eq!(
        result.invocation_state().get::<Marker>(),
        Some(Marker("from inner"))
    );
}
