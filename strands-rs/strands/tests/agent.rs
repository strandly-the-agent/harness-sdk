//! Ported from strands-ts/src/agent/__tests__/agent.test.ts (the loop, tools, input forms and
//! structured output cases that need no later subsystem).

#![expect(clippy::unwrap_used, reason = "test code")]

use std::sync::{Arc, Mutex};

use futures::StreamExt;
use serde_json::json;
use strands::agent::InvokeLimits;
use strands::error::Error;
use strands::hooks::{AfterToolsEvent, BeforeToolCallEvent, EndTurn};
use strands::models::model::Model;
use strands::testing::{HookRecorder, MockModel, MockTurnOptions, test_agent};
use strands::tools::FunctionTool;
use strands::tools::executors::ToolExecutorStrategy;
use strands::types::messages::{Role, TextBlock, ToolUseBlock};
use strands::types::tools::ToolResultStatus;
use strands::{Agent, AgentStreamEvent, ContentBlock, InvokeOptions, Message, StopReason, ToolError};

fn text(text: &str) -> ContentBlock {
    ContentBlock::from(text)
}

fn tool_use(name: &str, id: &str, input: serde_json::Value) -> ContentBlock {
    ToolUseBlock::new(name, id, input).into()
}

fn echo_tool() -> FunctionTool {
    FunctionTool::new("echo", "Echoes its input", |input, _context| async move {
        Ok::<_, ToolError>(input)
    })
}

fn agent_with(model: &Arc<MockModel>) -> Agent {
    test_agent(Arc::clone(model))
        .tools([echo_tool()])
        .build()
        .unwrap()
}

fn texts(message: &Message) -> Vec<String> {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn returns_correct_stop_reason_and_last_message() {
    let model = Arc::new(MockModel::new());
    model.add_turn(vec![text("Hello")]);
    let mut agent = agent_with(&model);

    let result = agent.invoke("Hi").await.unwrap();

    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(texts(result.last_message()), ["Hello"]);
    assert_eq!(agent.messages().len(), 2);
    assert_eq!(agent.messages()[0].role, Role::User);
}

#[tokio::test(flavor = "current_thread")]
async fn handles_tool_execution_flow() {
    let model = Arc::new(MockModel::new());
    model.add_turn(vec![tool_use("echo", "tool-1", json!({ "value": 1 }))]);
    model.add_turn(vec![text("Done")]);
    let mut agent = agent_with(&model);

    let result = agent.invoke("Use the tool").await.unwrap();

    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    let roles: Vec<Role> = agent.messages().iter().map(|message| message.role).collect();
    assert_eq!(roles, [Role::User, Role::Assistant, Role::User, Role::Assistant]);
    let ContentBlock::ToolResult(tool_result) = &agent.messages()[2].content[0] else {
        panic!("expected a tool result");
    };
    assert_eq!(tool_result.tool_use_id, "tool-1");
    assert_eq!(tool_result.status, ToolResultStatus::Success);
    assert_eq!(model.requests().len(), 2);
    assert_eq!(model.requests()[1].messages.len(), 3);
}

#[tokio::test]
async fn yields_tool_related_events() {
    let model = Arc::new(MockModel::new());
    model.add_turn(vec![tool_use("echo", "tool-1", json!({}))]);
    model.add_turn(vec![text("Done")]);
    let mut agent = agent_with(&model);

    let mut types = Vec::new();
    {
        let mut stream = agent.stream("Use the tool");
        while let Some(event) = stream.next().await {
            let event = serde_json::to_value(event.unwrap()).unwrap();
            types.push(event["type"].as_str().unwrap().to_owned());
        }
    }

    let position = |name: &str| types.iter().position(|kind| kind == name).unwrap();
    assert!(position("beforeToolsEvent") < position("beforeToolCallEvent"));
    assert!(position("beforeToolCallEvent") < position("afterToolCallEvent"));
    assert!(position("afterToolCallEvent") < position("toolResultEvent"));
    assert!(position("toolResultEvent") < position("afterToolsEvent"));
    assert_eq!(types.last().unwrap(), "agentResultEvent");
}

#[tokio::test]
async fn appends_the_tool_use_message_only_after_its_tools_finish() {
    let model = Arc::new(MockModel::new());
    model.add_turn(vec![tool_use("probe", "tool-1", json!({}))]);
    model.add_turn(vec![text("Done")]);
    let seen = Arc::new(Mutex::new(0));
    let probe_seen = Arc::clone(&seen);
    let probe = FunctionTool::new("probe", "Counts the history it sees", move |_, context| {
        let seen = Arc::clone(&probe_seen);
        async move {
            *seen.lock().unwrap() = context.agent().messages().len();
            Ok::<_, ToolError>(json!("ok"))
        }
    });
    let mut agent = test_agent(Arc::clone(&model)).tools([probe]).build().unwrap();

    agent.invoke("Go").await.unwrap();

    assert_eq!(*seen.lock().unwrap(), 1);
}

#[tokio::test]
async fn runs_the_tools_in_request_order_with_the_sequential_executor() {
    let model = Arc::new(MockModel::new());
    model.add_turn(vec![
        tool_use("echo", "tool-1", json!({ "n": 1 })),
        tool_use("echo", "tool-2", json!({ "n": 2 })),
    ]);
    model.add_turn(vec![text("Done")]);
    let recorder = HookRecorder::new();
    let mut agent = test_agent(Arc::clone(&model))
        .tools([echo_tool()])
        .tool_executor(ToolExecutorStrategy::Sequential)
        .build()
        .unwrap();
    recorder.attach(agent.hooks());

    agent.invoke("Go").await.unwrap();

    let tool_events: Vec<String> = recorder
        .events()
        .into_iter()
        .filter(|event| event.contains("ToolCall") || event == "toolResultEvent")
        .collect();
    assert_eq!(
        tool_events,
        [
            "beforeToolCallEvent",
            "afterToolCallEvent",
            "toolResultEvent",
            "beforeToolCallEvent",
            "afterToolCallEvent",
            "toolResultEvent"
        ]
    );
}

#[tokio::test]
async fn a_tool_that_panics_produces_an_error_result_and_the_loop_continues() {
    let model = Arc::new(MockModel::new());
    model.add_turn(vec![tool_use("boom", "tool-1", json!({}))]);
    model.add_turn(vec![text("Recovered")]);
    let boom = FunctionTool::new("boom", "Panics", |_, _| async move {
        if true {
            panic!("tool panicked hard");
        }
        Ok::<_, ToolError>(json!(null))
    });
    let mut agent = test_agent(Arc::clone(&model)).tools([boom]).build().unwrap();

    let result = agent.invoke("Go").await.unwrap();

    assert_eq!(texts(result.last_message()), ["Recovered"]);
    let ContentBlock::ToolResult(tool_result) = &agent.messages()[2].content[0] else {
        panic!("expected a tool result");
    };
    assert_eq!(tool_result.status, ToolResultStatus::Error);
    assert_eq!(
        serde_json::to_value(&tool_result.content).unwrap(),
        json!([{ "text": "tool panicked hard" }])
    );
}

#[tokio::test]
async fn reports_a_tool_missing_from_the_registry() {
    let model = Arc::new(MockModel::new());
    model.add_turn(vec![tool_use("missing", "tool-1", json!({}))]);
    model.add_turn(vec![text("Done")]);
    let mut agent = agent_with(&model);

    agent.invoke("Go").await.unwrap();

    let ContentBlock::ToolResult(tool_result) = &agent.messages()[2].content[0] else {
        panic!("expected a tool result");
    };
    assert_eq!(
        serde_json::to_value(&tool_result.content).unwrap(),
        json!([{ "text": "Tool 'missing' not found in registry" }])
    );
}

#[tokio::test]
async fn cancels_a_tool_call_from_a_before_tool_call_hook() {
    let model = Arc::new(MockModel::new());
    model.add_turn(vec![tool_use("echo", "tool-1", json!({}))]);
    model.add_turn(vec![text("Done")]);
    let mut agent = agent_with(&model);
    agent.add_hook(|event: &mut BeforeToolCallEvent| {
        if event.tool_use().name == "echo" {
            event.set_cancel(Some("echo is disabled".into()));
        }
    });

    agent.invoke("Go").await.unwrap();

    let ContentBlock::ToolResult(tool_result) = &agent.messages()[2].content[0] else {
        panic!("expected a tool result");
    };
    assert_eq!(tool_result.status, ToolResultStatus::Error);
    assert_eq!(
        serde_json::to_value(&tool_result.content).unwrap(),
        json!([{ "text": "echo is disabled" }])
    );
}

#[tokio::test]
async fn ends_the_turn_after_tools_when_a_hook_sets_end_turn() {
    let model = Arc::new(MockModel::new());
    model.add_turn(vec![tool_use("echo", "tool-1", json!({}))]);
    let mut agent = agent_with(&model);
    agent.add_hook(|event: &mut AfterToolsEvent<'_>| event.set_end_turn(Some(EndTurn::DefaultMessage)));

    let result = agent.invoke("Go").await.unwrap();

    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(
        texts(result.last_message()),
        ["Turn ended early by hook after tool execution"]
    );
    assert_eq!(model.requests().len(), 1);
    assert_eq!(agent.messages().len(), 4);
}

#[tokio::test]
async fn stops_at_the_turn_limit() {
    let model = Arc::new(MockModel::new());
    model.add_turn(vec![tool_use("echo", "tool-1", json!({}))]);
    let mut agent = agent_with(&model);
    let options = InvokeOptions {
        limits: Some(InvokeLimits {
            turns: std::num::NonZeroU64::new(1),
            ..InvokeLimits::default()
        }),
        ..InvokeOptions::default()
    };

    let result = agent.invoke_with("Go", options).await.unwrap();

    assert_eq!(result.stop_reason(), StopReason::LimitTurns);
    assert_eq!(result.last_message().role, Role::User);
    assert_eq!(model.requests().len(), 1);
    assert_eq!(result.invocation_context().unwrap().usage().turns_taken, 1);
}

#[tokio::test]
async fn records_usage_in_the_invocation_context() {
    let model = Arc::new(MockModel::new());
    let usage = strands::models::streaming::Usage::new(10, 5, 15);
    model.add_turn_with(
        vec![text("Hi")],
        MockTurnOptions {
            usage: Some(usage),
            ..MockTurnOptions::default()
        },
    );
    let mut agent = agent_with(&model);

    let result = agent.invoke("Hello").await.unwrap();

    assert_eq!(result.invocation_context().unwrap().usage().usage, usage);
}

#[tokio::test]
async fn propagates_a_model_error_after_the_after_invocation_event() {
    let model = Arc::new(MockModel::new());
    model.add_error(|| strands::error::ModelError::new("model unavailable").into());
    let recorder = HookRecorder::new();
    let mut agent = test_agent(Arc::clone(&model))
        .retry_strategy(Vec::new())
        .build()
        .unwrap();
    recorder.attach(agent.hooks());

    let error = agent.invoke("Hello?").await.unwrap_err();

    assert_eq!(error.to_string(), "model unavailable");
    assert_eq!(
        recorder.events().iter().rev().take(2).collect::<Vec<_>>(),
        ["afterInvocationEvent", "afterModelCallEvent"]
    );
}

#[tokio::test]
async fn rejects_a_blank_prompt() {
    let mut agent = agent_with(&Arc::new(MockModel::new()));

    let error = agent.invoke("   ").await.unwrap_err();

    assert!(matches!(error, Error::InvalidInput(_)));
    assert_eq!(
        error.to_string(),
        "Input prompt is blank. Provide non-blank text or at least one non-text content block."
    );
}

#[tokio::test]
async fn handles_empty_content_block_array() {
    let model = Arc::new(MockModel::new());
    model.add_turn(vec![text("Continuing")]);
    let mut agent = test_agent(Arc::clone(&model))
        .messages(vec![Message::new(
            Role::User,
            vec![TextBlock::new("Earlier").into()],
        )])
        .build()
        .unwrap();

    agent.invoke(Vec::<ContentBlock>::new()).await.unwrap();

    assert_eq!(model.requests()[0].messages.len(), 1);
}

#[tokio::test]
async fn appends_multiple_messages_in_order() {
    let model = Arc::new(MockModel::new());
    model.add_turn(vec![text("Ok")]);
    let mut agent = agent_with(&model);
    let messages = vec![
        Message::new(Role::User, vec![text("First")]),
        Message::new(Role::Assistant, vec![text("Second")]),
        Message::new(Role::User, vec![text("Third")]),
    ];

    agent.invoke(messages).await.unwrap();

    assert_eq!(agent.messages().len(), 4);
    assert_eq!(texts(&agent.messages()[2]), ["Third"]);
}

#[tokio::test]
async fn returns_structured_output_from_the_structured_output_tool() {
    /// The weather.
    #[derive(Debug, PartialEq, serde::Deserialize, schemars::JsonSchema)]
    struct Weather {
        city: String,
        temperature: f64,
    }
    let model = Arc::new(MockModel::new());
    model.add_turn(vec![tool_use(
        "strands_structured_output",
        "tool-1",
        json!({ "city": "Paris", "temperature": 21.5 }),
    )]);
    let mut agent = agent_with(&model);

    let result = agent
        .invoke_with(
            "Weather?",
            InvokeOptions::default().structured_output_schema::<Weather>(),
        )
        .await
        .unwrap();

    assert_eq!(result.stop_reason(), StopReason::ToolUse);
    assert_eq!(
        result.structured_output::<Weather>().unwrap(),
        Weather {
            city: "Paris".into(),
            temperature: 21.5
        }
    );
    assert!(agent.tool_registry().get("strands_structured_output").is_none());
}

#[tokio::test]
async fn forces_the_structured_output_tool_after_a_plain_text_answer() {
    /// A person.
    #[derive(serde::Deserialize, schemars::JsonSchema)]
    struct Person {
        name: String,
    }
    let model = Arc::new(MockModel::new());
    model.add_turn(vec![text("Here you go")]);
    model.add_turn(vec![tool_use(
        "strands_structured_output",
        "tool-1",
        json!({ "name": "Ada" }),
    )]);
    let mut agent = agent_with(&model);

    let result = agent
        .invoke_with(
            "Who?",
            InvokeOptions::default().structured_output_schema::<Person>(),
        )
        .await
        .unwrap();

    assert_eq!(result.structured_output::<Person>().unwrap().name, "Ada");
    let requests = model.requests();
    assert_eq!(requests[0].tool_choice, None);
    assert_eq!(
        serde_json::to_value(&requests[1].tool_choice).unwrap(),
        json!({ "tool": { "name": "strands_structured_output" } })
    );
    assert_eq!(requests[1].messages.len(), 1);
}

#[tokio::test]
async fn cancel_while_idle_then_invoke_completes_normally() {
    let model = Arc::new(MockModel::new());
    model.add_turn(vec![text("Hi")]);
    let mut agent = agent_with(&model);

    agent.cancel_handle().cancel(strands::CancelOptions::default());
    let result = agent.invoke("Hello").await.unwrap();

    assert_eq!(result.stop_reason(), StopReason::EndTurn);
}

#[tokio::test]
async fn cancels_through_the_invoke_cancel_signal() {
    let model = Arc::new(MockModel::new());
    model.add_turn(vec![text("Hi")]);
    let mut agent = agent_with(&model);
    let (signal, handle) = strands::CancelSignal::new();
    handle.cancel(strands::CancelOptions::default());

    let result = agent
        .invoke_with(
            "Hello",
            InvokeOptions {
                cancel_signal: Some(signal),
                ..InvokeOptions::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(result.stop_reason(), StopReason::Cancelled);
    assert_eq!(texts(result.last_message()), ["Cancelled by user"]);
    assert_eq!(model.call_count(), 0);
}

#[test]
fn agent_and_its_futures_are_send() {
    fn assert_send<T: Send>(_: &T) {}
    fn assert_sync<T: Sync>(_: &T) {}
    let mut agent = agent_with(&Arc::new(MockModel::new()));
    assert_sync(&agent);
    assert_send(&agent);
    assert_send(&agent.invoke("Hi"));
    assert_send(&agent.stream("Hi"));
    let model: Arc<dyn Model> = Arc::new(MockModel::new());
    assert_send(&model);
    let _ =
        AgentStreamEvent::BeforeInvocation(strands::agent::stream_event::BeforeInvocationEventData::new());
}
