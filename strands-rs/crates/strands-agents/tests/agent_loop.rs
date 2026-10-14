//! Core event-loop behaviour through the public API (`agent.test.ts`).

use std::sync::{Arc, Mutex};

use futures::StreamExt;
use serde_json::json;
use strands_agents::agent::InvokeOptions;
use strands_agents::hooks::{
    AfterInvocationEvent, AfterToolsEvent, BeforeInvocationEvent, BeforeToolsEvent, MessageAddedEvent,
    ModelMessageEvent,
};
use strands_agents::test_support::{MockMessageModel, MockResponse};
use strands_agents::tools::{tool, ToolRef};
use strands_agents::types::{ContentBlock, Role, StopReason, ToolResultStatus, ToolUseBlock};
use strands_agents::{Agent, AgentStreamEvent, Message};

fn text_model(texts: &[&str]) -> Arc<MockMessageModel> {
    MockMessageModel::shared(
        texts
            .iter()
            .map(|t| MockResponse::message(Message::assistant(*t)))
            .collect(),
    )
}

fn tool_use_message(name: &str, id: &str, input: serde_json::Value) -> Message {
    Message::new(
        Role::Assistant,
        vec![ContentBlock::ToolUse(ToolUseBlock::new(name, id, input))],
    )
}

fn echo_tool() -> ToolRef {
    Arc::new(
        tool("echo")
            .description("Echoes its input")
            .handler(|input, _ctx| async move { Ok(json!({ "echoed": input })) }),
    )
}

#[tokio::test]
async fn returns_the_model_response() {
    let agent = Agent::new(text_model(&["Hello there"])).await.unwrap();

    let result = agent.invoke("Hi").await.unwrap();

    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(result.to_string(), "Hello there");
    let messages = agent.messages();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, Role::User);
    assert_eq!(
        messages[1].content,
        vec![ContentBlock::Text("Hello there".to_owned())]
    );
}

#[tokio::test]
async fn appends_input_messages_once_per_invocation() {
    let agent = Agent::new(text_model(&["one", "two"])).await.unwrap();

    agent.invoke("first").await.unwrap();
    agent.invoke("second").await.unwrap();

    let roles: Vec<Role> = agent.messages().iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        vec![Role::User, Role::Assistant, Role::User, Role::Assistant]
    );
}

#[tokio::test]
async fn runs_tools_and_feeds_results_back_to_the_model() {
    let model = MockMessageModel::shared(vec![
        MockResponse::with_stop_reason(
            tool_use_message("echo", "t1", json!({ "value": 1 })),
            StopReason::ToolUse,
        ),
        MockResponse::message(Message::assistant("done")),
    ]);
    let agent = Agent::builder()
        .model(model.clone())
        .tool(echo_tool())
        .build()
        .await
        .unwrap();

    let result = agent.invoke("run the tool").await.unwrap();

    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(result.to_string(), "done");
    assert_eq!(model.call_count(), 2);

    let messages = agent.messages();
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[2].role, Role::User);
    let ContentBlock::ToolResult(tool_result) = &messages[2].content[0] else {
        panic!("expected a tool result block, got {:?}", messages[2].content[0]);
    };
    assert_eq!(tool_result.tool_use_id, "t1");
    assert_eq!(tool_result.status, ToolResultStatus::Success);
}

#[tokio::test]
async fn unknown_tool_produces_an_error_result_and_keeps_looping() {
    let model = MockMessageModel::shared(vec![
        MockResponse::with_stop_reason(tool_use_message("nope", "t1", json!({})), StopReason::ToolUse),
        MockResponse::message(Message::assistant("recovered")),
    ]);
    let agent = Agent::builder()
        .model(model)
        .tool(echo_tool())
        .build()
        .await
        .unwrap();

    let result = agent.invoke("call a hallucinated tool").await.unwrap();

    assert_eq!(result.to_string(), "recovered");
    let messages = agent.messages();
    let ContentBlock::ToolResult(tool_result) = &messages[2].content[0] else {
        panic!("expected a tool result block");
    };
    assert_eq!(tool_result.status, ToolResultStatus::Error);
}

#[tokio::test]
async fn emits_events_in_order() {
    let model = MockMessageModel::shared(vec![
        MockResponse::with_stop_reason(
            tool_use_message("echo", "t1", json!({ "value": 1 })),
            StopReason::ToolUse,
        ),
        MockResponse::message(Message::assistant("done")),
    ]);
    let agent = Agent::builder()
        .model(model)
        .tool(echo_tool())
        .build()
        .await
        .unwrap();

    let mut names = Vec::new();
    let mut stream = agent.stream("go", InvokeOptions::default());
    while let Some(event) = stream.next().await {
        names.push(event.unwrap().type_name());
    }

    assert_eq!(names.first(), Some(&"beforeInvocation"));
    assert_eq!(names.last(), Some(&"agentResult"));
    assert_eq!(names[names.len() - 2], "afterInvocation");
    let filtered: Vec<&&str> = names
        .iter()
        .filter(|n| {
            matches!(
                **n,
                "beforeModelCall" | "modelMessage" | "afterModelCall" | "beforeTools" | "afterTools"
            )
        })
        .collect();
    assert_eq!(
        filtered,
        vec![
            &"beforeModelCall",
            &"modelMessage",
            &"afterModelCall",
            &"beforeTools",
            &"afterTools",
            &"beforeModelCall",
            &"modelMessage",
            &"afterModelCall",
        ]
    );
}

#[tokio::test]
async fn fires_lifecycle_hooks_in_order() {
    let agent = Agent::new(text_model(&["hi"])).await.unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));

    let recorder = Arc::clone(&seen);
    agent
        .hooks()
        .add_sync_callback::<BeforeInvocationEvent, _>(move |_| {
            recorder.lock().unwrap().push("before");
            Ok(())
        });
    let recorder = Arc::clone(&seen);
    agent
        .hooks()
        .add_sync_callback::<MessageAddedEvent, _>(move |event| {
            recorder
                .lock()
                .unwrap()
                .push(if event.message.role == Role::User {
                    "user"
                } else {
                    "assistant"
                });
            Ok(())
        });
    let recorder = Arc::clone(&seen);
    agent.hooks().add_sync_callback::<ModelMessageEvent, _>(move |_| {
        recorder.lock().unwrap().push("model");
        Ok(())
    });
    let recorder = Arc::clone(&seen);
    agent
        .hooks()
        .add_sync_callback::<AfterInvocationEvent, _>(move |_| {
            recorder.lock().unwrap().push("after");
            Ok(())
        });

    agent.invoke("hi").await.unwrap();

    assert_eq!(
        *seen.lock().unwrap(),
        vec!["before", "user", "model", "assistant", "after"]
    );
}

#[tokio::test]
async fn before_invocation_hook_can_cancel_the_invocation() {
    let model = text_model(&["never reached"]);
    let agent = Agent::new(model.clone()).await.unwrap();
    agent
        .hooks()
        .add_sync_callback::<BeforeInvocationEvent, _>(|event| {
            event.cancel = "denied".into();
            Ok(())
        });

    let result = agent.invoke("hi").await.unwrap();

    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(result.to_string(), "denied");
    assert_eq!(model.call_count(), 0);
    let messages = agent.messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, Role::Assistant);
    assert_eq!(messages[0].content, vec![ContentBlock::Text("denied".to_owned())]);
}

#[tokio::test]
async fn before_tools_hook_can_cancel_the_batch() {
    let model = MockMessageModel::shared(vec![
        MockResponse::with_stop_reason(
            tool_use_message("echo", "t1", json!({ "value": 1 })),
            StopReason::ToolUse,
        ),
        MockResponse::message(Message::assistant("after cancel")),
    ]);
    let agent = Agent::builder()
        .model(model)
        .tool(echo_tool())
        .build()
        .await
        .unwrap();
    agent.hooks().add_sync_callback::<BeforeToolsEvent, _>(|event| {
        event.cancel = true.into();
        Ok(())
    });

    agent.invoke("go").await.unwrap();

    let messages = agent.messages();
    let ContentBlock::ToolResult(tool_result) = &messages[2].content[0] else {
        panic!("expected a tool result block");
    };
    assert_eq!(tool_result.status, ToolResultStatus::Error);
}

#[tokio::test]
async fn after_tools_hook_can_end_the_turn() {
    let model = MockMessageModel::shared(vec![MockResponse::with_stop_reason(
        tool_use_message("echo", "t1", json!({ "value": 1 })),
        StopReason::ToolUse,
    )]);
    let agent = Agent::builder()
        .model(model.clone())
        .tool(echo_tool())
        .build()
        .await
        .unwrap();
    agent.hooks().add_sync_callback::<AfterToolsEvent, _>(|event| {
        event.end_turn = strands_agents::hooks::EndTurn::Text("stopping".to_owned());
        Ok(())
    });

    let result = agent.invoke("go").await.unwrap();

    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(result.to_string(), "stopping");
    assert_eq!(model.call_count(), 1);
}

#[tokio::test]
async fn after_invocation_resume_runs_another_invocation() {
    let model = text_model(&["first", "second"]);
    let agent = Agent::new(model.clone()).await.unwrap();
    let resumed = Arc::new(Mutex::new(false));
    let flag = Arc::clone(&resumed);
    agent
        .hooks()
        .add_sync_callback::<AfterInvocationEvent, _>(move |event| {
            let mut flag = flag.lock().unwrap();
            if !*flag {
                *flag = true;
                event.resume = Some("again".into());
            }
            Ok(())
        });

    let result = agent.invoke("hi").await.unwrap();

    assert_eq!(result.to_string(), "second");
    assert_eq!(model.call_count(), 2);
}

#[tokio::test]
async fn rejects_a_concurrent_invocation() {
    let agent = Agent::new(text_model(&["hello"])).await.unwrap();
    let mut first = agent.stream("hi", InvokeOptions::default());
    let mut second = agent.stream("again", InvokeOptions::default());

    let error = second
        .next()
        .await
        .expect("an event")
        .expect_err("should be rejected");
    assert!(
        matches!(error, strands_agents::Error::ConcurrentInvocation(_)),
        "unexpected error: {error}"
    );
    assert!(second.next().await.is_none());
    while first.next().await.is_some() {}
}

#[tokio::test]
async fn releases_the_invocation_lock_after_completion() {
    let agent = Agent::new(text_model(&["one", "two"])).await.unwrap();

    agent.invoke("hi").await.unwrap();
    assert!(!agent.is_invoking());
    agent.invoke("again").await.unwrap();
}

#[tokio::test]
async fn propagates_model_errors() {
    let model = MockMessageModel::shared(vec![MockResponse::Error(strands_agents::Error::other("boom"))]);
    let agent = Agent::builder()
        .model(model)
        .no_retry_strategy()
        .build()
        .await
        .unwrap();

    let error = agent.invoke("hi").await.expect_err("model error propagates");

    assert!(error.to_string().contains("boom"), "unexpected error: {error}");
}

#[tokio::test]
async fn invocation_state_is_shared_with_the_result() {
    let agent = Agent::new(text_model(&["hi"])).await.unwrap();
    let state = strands_agents::InvocationState::new();
    state.insert("key", "value".to_owned());

    let result = agent
        .invoke_with(
            "hi",
            InvokeOptions {
                invocation_state: Some(state.clone()),
                ..InvokeOptions::default()
            },
        )
        .await
        .unwrap();

    assert!(result.invocation_state.ptr_eq(&state));
}

#[tokio::test]
async fn stream_ends_with_exactly_one_agent_result() {
    let agent = Agent::new(text_model(&["hi"])).await.unwrap();

    let mut stream = agent.stream("hi", InvokeOptions::default());
    let mut results = 0;
    while let Some(event) = stream.next().await {
        if let AgentStreamEvent::AgentResult(_) = event.unwrap() {
            results += 1;
        }
    }

    assert_eq!(results, 1);
}
