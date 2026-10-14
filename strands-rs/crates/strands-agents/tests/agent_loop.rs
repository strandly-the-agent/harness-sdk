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
use strands_agents::types::{ContentBlock, Role, StopReason, ToolResultStatus, ToolUseBlock, Usage};
use strands_agents::{Agent, AgentStreamEvent, Limits, Message};

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

// ---- limits (§9.4 step 9) -------------------------------------------------

fn tool_use_responses(count: usize, usage: Option<Usage>) -> Arc<MockMessageModel> {
    MockMessageModel::shared(
        (0..count)
            .map(|i| {
                let message = tool_use_message("echo", &format!("t{i}"), json!({ "value": i }));
                match &usage {
                    Some(usage) => MockResponse::Message {
                        message,
                        stop_reason: Some(StopReason::ToolUse),
                        usage: Some(usage.clone()),
                    },
                    None => MockResponse::with_stop_reason(message, StopReason::ToolUse),
                }
            })
            .collect(),
    )
}

async fn looping_agent(model: Arc<MockMessageModel>) -> Agent {
    Agent::builder()
        .model(model)
        .tool(echo_tool())
        .build()
        .await
        .unwrap()
}

#[tokio::test]
async fn stops_when_the_turn_limit_is_reached() {
    let model = tool_use_responses(5, None);
    let agent = looping_agent(Arc::clone(&model)).await;

    let result = agent
        .invoke_with(
            "go",
            InvokeOptions {
                limits: Some(Limits {
                    turns: Some(2),
                    ..Limits::default()
                }),
                ..InvokeOptions::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(result.stop_reason, StopReason::LimitTurns);
    assert_eq!(model.call_count(), 2);
    assert_eq!(result.last_message, *agent.messages().last().unwrap());
    assert_eq!(result.last_message.role, Role::User);
}

#[tokio::test]
async fn stops_when_the_output_token_limit_is_reached() {
    let usage = Usage {
        input_tokens: 5,
        output_tokens: 10,
        total_tokens: 15,
        ..Usage::default()
    };
    let model = tool_use_responses(5, Some(usage));
    let agent = looping_agent(Arc::clone(&model)).await;

    let result = agent
        .invoke_with(
            "go",
            InvokeOptions {
                limits: Some(Limits {
                    output_tokens: Some(10),
                    ..Limits::default()
                }),
                ..InvokeOptions::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(result.stop_reason, StopReason::LimitOutputTokens);
    assert_eq!(model.call_count(), 1);
}

#[tokio::test]
async fn stops_when_the_total_token_limit_is_reached() {
    let usage = Usage {
        input_tokens: 10,
        output_tokens: 5,
        total_tokens: 15,
        ..Usage::default()
    };
    let model = tool_use_responses(5, Some(usage));
    let agent = looping_agent(Arc::clone(&model)).await;

    let result = agent
        .invoke_with(
            "go",
            InvokeOptions {
                limits: Some(Limits {
                    total_tokens: Some(30),
                    ..Limits::default()
                }),
                ..InvokeOptions::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(result.stop_reason, StopReason::LimitTotalTokens);
    assert_eq!(model.call_count(), 2);
}

#[tokio::test]
async fn turns_limit_wins_over_token_limits() {
    let usage = Usage {
        input_tokens: 10,
        output_tokens: 10,
        total_tokens: 20,
        ..Usage::default()
    };
    let model = tool_use_responses(5, Some(usage));
    let agent = looping_agent(Arc::clone(&model)).await;

    let result = agent
        .invoke_with(
            "go",
            InvokeOptions {
                limits: Some(Limits {
                    turns: Some(1),
                    output_tokens: Some(1),
                    total_tokens: Some(1),
                }),
                ..InvokeOptions::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(result.stop_reason, StopReason::LimitTurns);
}

#[tokio::test]
async fn rejects_non_positive_limits() {
    let agent = Agent::new(text_model(&["hi"])).await.unwrap();

    let error = agent
        .invoke_with(
            "go",
            InvokeOptions {
                limits: Some(Limits {
                    turns: Some(0),
                    ..Limits::default()
                }),
                ..InvokeOptions::default()
            },
        )
        .await
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "limits.turns must be a positive finite number, got 0"
    );
}

// ---- cancellation (§9.4 steps 14, 23) -------------------------------------

#[tokio::test]
async fn cancel_before_the_model_call_returns_cancelled() {
    let model = text_model(&["never reached"]);
    let agent = Agent::new(model.clone()).await.unwrap();
    let token = tokio_util::sync::CancellationToken::new();
    token.cancel();

    let result = agent
        .invoke_with(
            "go",
            InvokeOptions {
                cancel: Some(token),
                ..InvokeOptions::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(result.stop_reason, StopReason::Cancelled);
    assert_eq!(result.to_string(), "Cancelled by user");
    assert_eq!(model.call_count(), 0);
    assert!(agent.messages().is_empty());
}

#[tokio::test]
async fn cancel_after_a_tool_use_reply_leaves_no_orphan_tool_uses() {
    let model = MockMessageModel::shared(vec![
        MockResponse::with_stop_reason(
            tool_use_message("echo", "t1", json!({ "value": 1 })),
            StopReason::ToolUse,
        ),
        MockResponse::message(Message::assistant("never reached")),
    ]);
    let agent = looping_agent(Arc::clone(&model)).await;
    let cancelling = agent.clone();
    agent.hooks().add_sync_callback(move |_: &mut ModelMessageEvent| {
        cancelling.cancel();
        Ok(())
    });

    let result = agent.invoke("go").await.unwrap();

    assert_eq!(result.stop_reason, StopReason::Cancelled);
    assert_eq!(model.call_count(), 1);
    let messages = agent.messages();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[1].role, Role::Assistant);
    assert_eq!(messages[2].role, Role::User);
    match &messages[2].content[0] {
        ContentBlock::ToolResult(result) => {
            assert_eq!(result.tool_use_id, "t1");
            assert_eq!(result.status, ToolResultStatus::Error);
            assert_eq!(result.text(), "Tool execution cancelled");
        }
        other => panic!("expected a tool result, got {other:?}"),
    }
    assert_eq!(result.last_message, messages[1]);
}

#[tokio::test]
async fn cancel_during_tool_execution_stops_after_the_results() {
    let model = MockMessageModel::shared(vec![
        MockResponse::with_stop_reason(tool_use_message("cancels", "t1", json!({})), StopReason::ToolUse),
        MockResponse::message(Message::assistant("never reached")),
    ]);
    let agent = Agent::builder().model(model.clone()).build().await.unwrap();
    let cancelling = agent.clone();
    let cancel_tool: ToolRef = Arc::new(
        tool("cancels")
            .description("Cancels the invocation, then succeeds")
            .handler(move |_input: serde_json::Value, _ctx| {
                let agent = cancelling.clone();
                async move {
                    agent.cancel();
                    Ok(json!({ "ok": true }))
                }
            }),
    );
    agent
        .with_tool_registry(|registry| registry.add(cancel_tool))
        .unwrap();

    let result = agent.invoke("go").await.unwrap();

    assert_eq!(result.stop_reason, StopReason::Cancelled);
    assert_eq!(result.to_string(), "Cancelled by user");
    let messages = agent.messages();
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[2].role, Role::User);
    assert_eq!(messages[3], result.last_message);
    assert_eq!(model.call_count(), 1);
}

// ---- structured output (§9.4 steps 3, 13, 20, 28) -------------------------

#[derive(Debug, serde::Deserialize, schemars::JsonSchema, PartialEq)]
struct Person {
    name: String,
    age: u64,
}

fn structured_tool_use(id: &str, input: serde_json::Value) -> Message {
    tool_use_message("strands_structured_output", id, input)
}

#[tokio::test]
async fn structured_output_when_the_model_calls_the_tool() {
    let model = MockMessageModel::shared(vec![MockResponse::with_stop_reason(
        structured_tool_use("s1", json!({ "name": "Ada", "age": 36 })),
        StopReason::ToolUse,
    )]);
    let agent = Agent::new(model.clone()).await.unwrap();

    let person: Person = agent.invoke_structured("who?").await.unwrap();

    assert_eq!(
        person,
        Person {
            name: "Ada".to_owned(),
            age: 36
        }
    );
    assert!(agent
        .tools()
        .iter()
        .all(|t| t.name() != "strands_structured_output"));
}

#[tokio::test]
async fn structured_output_is_forced_after_a_plain_text_reply() {
    let model = MockMessageModel::shared(vec![
        MockResponse::message(Message::assistant("Ada is 36")),
        MockResponse::with_stop_reason(
            structured_tool_use("s1", json!({ "name": "Ada", "age": 36 })),
            StopReason::ToolUse,
        ),
    ]);
    let agent = Agent::new(model.clone()).await.unwrap();

    let person: Person = agent.invoke_structured("who?").await.unwrap();

    assert_eq!(person.name, "Ada");
    assert_eq!(model.call_count(), 2);
    let calls = model.calls();
    assert_eq!(calls[0].1.tool_choice, None);
    assert_eq!(
        calls[1].1.tool_choice,
        Some(strands_agents::types::ToolChoice::Tool {
            name: "strands_structured_output".to_owned()
        })
    );
    let messages = agent.messages();
    assert!(messages.iter().all(|m| !m.text().contains("Ada is 36")));
}

#[tokio::test]
async fn structured_output_fails_when_the_model_never_calls_the_tool() {
    let model = MockMessageModel::shared(vec![
        MockResponse::message(Message::assistant("first")),
        MockResponse::message(Message::assistant("second")),
    ]);
    let agent = Agent::new(model.clone()).await.unwrap();

    let error = agent.invoke_structured::<Person>("who?").await.unwrap_err();

    assert_eq!(
        error.to_string(),
        "The model failed to invoke the structured output tool even after it was forced."
    );
    assert_eq!(model.call_count(), 2);
    assert!(agent
        .tools()
        .iter()
        .all(|t| t.name() != "strands_structured_output"));
}

#[tokio::test]
async fn structured_output_schema_is_reported_on_the_result() {
    let model = MockMessageModel::shared(vec![MockResponse::with_stop_reason(
        structured_tool_use("s1", json!({ "name": "Ada", "age": 36 })),
        StopReason::ToolUse,
    )]);
    let agent = Agent::new(model.clone()).await.unwrap();

    let result = agent
        .invoke_with(
            "who?",
            InvokeOptions {
                structured_output_schema: Some(json!({
                    "type": "object",
                    "properties": { "name": { "type": "string" }, "age": { "type": "number" } },
                    "required": ["name", "age"]
                })),
                ..InvokeOptions::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(result.stop_reason, StopReason::ToolUse);
    assert_eq!(
        result.structured_output,
        Some(json!({ "name": "Ada", "age": 36 }))
    );
}

// ---- projected input tokens, maxTokens (§10 step 6, §8.6) -----------------

#[tokio::test]
async fn projects_input_tokens_for_the_model_call() {
    let agent = Agent::new(text_model(&["hi"])).await.unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&seen);
    agent
        .hooks()
        .add_sync_callback(move |event: &mut strands_agents::hooks::BeforeModelCallEvent| {
            captured.lock().unwrap().push(event.projected_input_tokens);
            Ok(())
        });

    agent.invoke("count my tokens").await.unwrap();

    let projected = seen.lock().unwrap().clone();
    assert_eq!(projected.len(), 1);
    assert!(projected[0].is_some_and(|tokens| tokens > 0));
}

#[tokio::test]
async fn max_tokens_from_the_model_surfaces_as_an_error() {
    let model = MockMessageModel::shared(vec![MockResponse::with_stop_reason(
        Message::assistant("partial answer"),
        StopReason::MaxTokens,
    )]);
    let agent = Agent::new(model.clone()).await.unwrap();

    let error = agent.invoke("go").await.unwrap_err();

    match error {
        strands_agents::Error::MaxTokens { partial_message, .. } => {
            assert_eq!(partial_message.text(), "partial answer")
        }
        other => panic!("expected MaxTokens, got {other:?}"),
    }
    let messages = agent.messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, Role::User);
}

// ---- cancellation and invocation-lock regressions -------------------------

#[tokio::test]
async fn a_panicking_hook_releases_the_invocation_lock() {
    let agent = Agent::new(text_model(&["after the panic"])).await.unwrap();
    let panicked = Arc::new(Mutex::new(false));
    let flag = Arc::clone(&panicked);
    agent
        .hooks()
        .add_sync_callback(move |_: &mut BeforeInvocationEvent| {
            if !*flag.lock().unwrap() {
                *flag.lock().unwrap() = true;
                panic!("boom");
            }
            Ok(())
        });

    let error = agent.invoke("go").await.unwrap_err();

    assert!(
        error.to_string().contains("agent invocation task failed"),
        "unexpected error: {error}"
    );
    assert!(!agent.is_invoking());
    let result = agent.invoke("again").await.unwrap();
    assert_eq!(result.to_string(), "after the panic");
}

#[tokio::test]
async fn dropping_the_stream_cancels_the_invocation() {
    let model = MockMessageModel::shared(vec![
        MockResponse::with_stop_reason(tool_use_message("slow", "t1", json!({})), StopReason::ToolUse),
        MockResponse::message(Message::assistant("never reached")),
    ]);
    let agent = Agent::builder().model(model.clone()).build().await.unwrap();
    let slow: ToolRef = Arc::new(tool("slow").description("Sleeps before succeeding").handler(
        |_input: serde_json::Value, _ctx| async move {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            Ok(json!({ "ok": true }))
        },
    ));
    agent.with_tool_registry(|registry| registry.add(slow)).unwrap();

    let mut stream = agent.stream("go", InvokeOptions::default());
    stream.next().await.unwrap().unwrap();
    drop(stream);

    let released = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while agent.is_invoking() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(released.is_ok(), "invocation outlived the dropped stream");
    assert!(agent.cancel_token().is_cancelled());
    assert!(
        model.call_count() <= 1,
        "model kept running: {}",
        model.call_count()
    );
}

#[tokio::test]
async fn cancel_right_after_stream_cancels_the_new_invocation() {
    let model = text_model(&["never reached"]);
    let agent = Agent::builder().model(model.clone()).build().await.unwrap();

    let mut stream = agent.stream("go", InvokeOptions::default());
    agent.cancel();

    let mut result = None;
    while let Some(event) = stream.next().await {
        if let Some(agent_result) = event.unwrap().result() {
            result = Some(agent_result.clone());
        }
    }
    assert_eq!(result.unwrap().stop_reason, StopReason::Cancelled);
    assert_eq!(model.call_count(), 0);
}

#[tokio::test]
async fn a_tool_cancellation_leaves_results_for_every_tool_use() {
    let model = MockMessageModel::shared(vec![
        MockResponse::with_stop_reason(
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::ToolUse(ToolUseBlock::new("cancels", "t1", json!({}))),
                    ContentBlock::ToolUse(ToolUseBlock::new("echo", "t2", json!({ "value": 1 }))),
                ],
            ),
            StopReason::ToolUse,
        ),
        MockResponse::message(Message::assistant("never reached")),
    ]);
    let agent = Agent::builder().model(model.clone()).build().await.unwrap();
    let cancelling = agent.clone();
    let cancels: ToolRef = Arc::new(
        tool("cancels")
            .description("Cancels the invocation and reports cancellation")
            .handler(move |_input: serde_json::Value, _ctx| {
                let agent = cancelling.clone();
                async move {
                    agent.cancel();
                    Err::<serde_json::Value, _>(strands_agents::Error::Cancelled)
                }
            }),
    );
    agent
        .with_tool_registry(|registry| registry.add_or_replace([cancels, echo_tool()]))
        .unwrap();

    let result = agent.invoke("go").await.unwrap();

    assert_eq!(result.stop_reason, StopReason::Cancelled);
    assert_eq!(result.to_string(), "Cancelled by user");
    let messages = agent.messages();
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[1].role, Role::Assistant);
    assert_eq!(messages[1].content.len(), 2);
    assert_eq!(messages[2].role, Role::User);
    let ids: Vec<String> = messages[2]
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolResult(result) => Some(result.tool_use_id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(ids, vec!["t1".to_owned(), "t2".to_owned()]);
    match &messages[2].content[0] {
        ContentBlock::ToolResult(result) => {
            assert_eq!(result.status, ToolResultStatus::Error);
            assert_eq!(result.text(), "Tool execution cancelled");
        }
        other => panic!("expected a tool result, got {other:?}"),
    }
    assert_eq!(model.call_count(), 1);
}
