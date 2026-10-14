//! Remaining in-scope `agent.test.ts` behaviour not covered by `agent_loop.rs`.

use std::sync::{Arc, Mutex};

use futures::StreamExt;
use serde_json::json;
use strands_agents::agent::InvokeOptions;
use strands_agents::hooks::{
    AfterInvocationEvent, AfterModelCallEvent, AfterToolCallEvent, AfterToolsEvent, BeforeInvocationEvent,
    BeforeModelCallEvent, BeforeToolCallEvent, BeforeToolsEvent, Cancel, EndTurn, InitializedEvent,
    MessageAddedEvent,
};
use strands_agents::plugins::Plugin;
use strands_agents::test_support::{MockMessageModel, MockResponse};
use strands_agents::tools::{tool, ToolRef};
use strands_agents::types::{ContentBlock, Role, StopReason, SystemPrompt, ToolResultStatus, ToolUseBlock};
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

fn echo_tool(name: &str) -> ToolRef {
    Arc::new(
        tool(name)
            .description("Echoes its input")
            .handler(|input, _ctx| async move { Ok(json!({ "echoed": input })) }),
    )
}

fn tool_use_then_text(name: &str) -> Arc<MockMessageModel> {
    MockMessageModel::shared(vec![
        MockResponse::with_stop_reason(
            tool_use_message(name, "t1", json!({ "value": 1 })),
            StopReason::ToolUse,
        ),
        MockResponse::message(Message::assistant("done")),
    ])
}

fn result_text(message: &Message) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolResult(result) => Some(
                result
                    .content
                    .iter()
                    .map(|c| match c {
                        strands_agents::types::ToolResultContent::Text(t) => t.clone(),
                        strands_agents::types::ToolResultContent::Json(v) => v.to_string(),
                        _ => String::new(),
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn keeps_existing_messages_passed_to_the_builder() {
    let agent = Agent::builder()
        .model(text_model(&["hi"]))
        .messages(vec![
            Message::user("earlier"),
            Message::assistant("earlier reply"),
        ])
        .build()
        .await
        .unwrap();

    agent.invoke("now").await.unwrap();

    let roles: Vec<Role> = agent.messages().iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        vec![Role::User, Role::Assistant, Role::User, Role::Assistant]
    );
}

#[tokio::test]
async fn set_messages_replaces_the_conversation() {
    let agent = Agent::new(text_model(&["hi"])).await.unwrap();
    agent.invoke("one").await.unwrap();

    agent.set_messages(vec![Message::user("only")]);

    assert_eq!(agent.messages().len(), 1);
    assert_eq!(agent.messages()[0].role, Role::User);
}

#[tokio::test]
async fn state_is_readable_and_writable() {
    let agent = Agent::new(text_model(&["hi"])).await.unwrap();

    agent.state().set("key", json!("value"));

    assert_eq!(agent.state().get("key"), Some(json!("value")));
}

#[tokio::test]
async fn set_system_prompt_changes_what_the_model_sees() {
    let model = text_model(&["a", "b"]);
    let agent = Agent::builder()
        .model(Arc::clone(&model) as Arc<_>)
        .system_prompt("first")
        .build()
        .await
        .unwrap();

    agent.invoke("one").await.unwrap();
    agent.set_system_prompt(Some(SystemPrompt::Text("second".to_owned())));
    agent.invoke("two").await.unwrap();

    let prompts: Vec<Option<SystemPrompt>> =
        model.calls().into_iter().map(|(_, o)| o.system_prompt).collect();
    assert_eq!(
        prompts,
        vec![
            Some(SystemPrompt::Text("first".to_owned())),
            Some(SystemPrompt::Text("second".to_owned()))
        ]
    );
}

#[tokio::test]
async fn fires_message_added_for_each_appended_message() {
    let agent = Agent::builder()
        .model(tool_use_then_text("echo"))
        .tool(echo_tool("echo"))
        .build()
        .await
        .unwrap();

    let roles = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&roles);
    agent
        .hooks()
        .add_sync_callback(move |event: &mut MessageAddedEvent| {
            captured.lock().unwrap().push(event.message.role);
            Ok(())
        });

    agent.invoke("go").await.unwrap();

    assert_eq!(
        *roles.lock().unwrap(),
        vec![Role::User, Role::Assistant, Role::User, Role::Assistant]
    );
}

#[tokio::test]
async fn after_invocation_callbacks_run_in_reverse_order() {
    let agent = Agent::new(text_model(&["hi"])).await.unwrap();
    let order = Arc::new(Mutex::new(Vec::new()));

    for label in ["first", "second"] {
        let captured = Arc::clone(&order);
        agent
            .hooks()
            .add_sync_callback(move |_e: &mut AfterInvocationEvent| {
                captured.lock().unwrap().push(label);
                Ok(())
            });
    }
    for label in ["before-first", "before-second"] {
        let captured = Arc::clone(&order);
        agent
            .hooks()
            .add_sync_callback(move |_e: &mut BeforeInvocationEvent| {
                captured.lock().unwrap().push(label);
                Ok(())
            });
    }

    agent.invoke("go").await.unwrap();

    assert_eq!(
        *order.lock().unwrap(),
        vec!["before-first", "before-second", "second", "first"]
    );
}

#[tokio::test]
async fn after_model_call_callbacks_run_in_reverse_order() {
    let agent = Agent::new(text_model(&["hi"])).await.unwrap();
    let order = Arc::new(Mutex::new(Vec::new()));

    for label in ["first", "second"] {
        let captured = Arc::clone(&order);
        agent
            .hooks()
            .add_sync_callback(move |_e: &mut AfterModelCallEvent| {
                captured.lock().unwrap().push(label);
                Ok(())
            });
    }
    let captured = Arc::clone(&order);
    agent
        .hooks()
        .add_sync_callback(move |_e: &mut BeforeModelCallEvent| {
            captured.lock().unwrap().push("before");
            Ok(())
        });

    agent.invoke("go").await.unwrap();

    assert_eq!(*order.lock().unwrap(), vec!["before", "second", "first"]);
}

#[tokio::test]
async fn after_invocation_resume_runs_another_invocation() {
    let agent = Agent::new(text_model(&["first", "second"])).await.unwrap();
    let resumed = Arc::new(Mutex::new(false));
    let captured = Arc::clone(&resumed);

    agent
        .hooks()
        .add_sync_callback(move |event: &mut AfterInvocationEvent| {
            let mut done = captured.lock().unwrap();
            if !*done {
                *done = true;
                event.resume = Some("again".into());
            }
            Ok(())
        });

    let result = agent.invoke("go").await.unwrap();

    assert_eq!(result.to_string(), "second");
    let roles: Vec<Role> = agent.messages().iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        vec![Role::User, Role::Assistant, Role::User, Role::Assistant]
    );
}

#[tokio::test]
async fn after_invocation_resume_is_ignored_when_the_invocation_errored() {
    let agent = Agent::new(MockMessageModel::shared(Vec::new())).await.unwrap();
    agent
        .hooks()
        .add_sync_callback(|_e: &mut BeforeModelCallEvent| Err(strands_agents::Error::Other("boom".into())));
    let resumes = Arc::new(Mutex::new(0usize));
    let captured = Arc::clone(&resumes);
    agent
        .hooks()
        .add_sync_callback(move |event: &mut AfterInvocationEvent| {
            *captured.lock().unwrap() += 1;
            event.resume = Some("again".into());
            Ok(())
        });

    let err = agent.invoke("go").await.unwrap_err();

    assert!(err.to_string().contains("boom"), "got {err}");
    assert_eq!(*resumes.lock().unwrap(), 1);
}

#[tokio::test]
async fn rejects_concurrent_invocations() {
    let agent = Agent::new(text_model(&["hi", "there"])).await.unwrap();
    let mut first = agent.stream("one", InvokeOptions::default());

    let err = agent.invoke("two").await.unwrap_err();
    assert!(
        err.to_string()
            .contains("Agent is already processing an invocation"),
        "got {err}"
    );

    while let Some(event) = first.next().await {
        event.unwrap();
    }
    assert!(!agent.is_invoking());
}

#[tokio::test]
async fn is_invoking_is_false_before_and_after_an_invocation() {
    let agent = Agent::new(text_model(&["hi"])).await.unwrap();
    assert!(!agent.is_invoking());
    agent.invoke("go").await.unwrap();
    assert!(!agent.is_invoking());
}

#[tokio::test]
async fn initialize_is_idempotent_and_fires_initialized_once() {
    let agent = Agent::new(text_model(&["hi"])).await.unwrap();
    let count = Arc::new(Mutex::new(0usize));
    let captured = Arc::clone(&count);
    agent.hooks().add_sync_callback(move |_e: &mut InitializedEvent| {
        *captured.lock().unwrap() += 1;
        Ok(())
    });

    agent.initialize().await.unwrap();
    agent.initialize().await.unwrap();

    assert_eq!(*count.lock().unwrap(), 0, "already initialized by build()");
}

#[tokio::test]
async fn registers_tools_contributed_by_plugins() {
    struct ToolPlugin;

    #[async_trait::async_trait]
    impl Plugin for ToolPlugin {
        fn name(&self) -> &str {
            "tool-plugin"
        }

        async fn init_agent(&self, _agent: &Agent) -> strands_agents::Result<()> {
            Ok(())
        }

        fn tools(&self) -> Vec<ToolRef> {
            vec![echo_tool("plugin-echo")]
        }
    }

    let agent = Agent::builder()
        .model(text_model(&["hi"]))
        .plugin(Arc::new(ToolPlugin))
        .build()
        .await
        .unwrap();

    let names: Vec<String> = agent.tools().iter().map(|t| t.name().to_owned()).collect();
    assert!(names.contains(&"plugin-echo".to_owned()), "got {names:?}");
}

#[tokio::test]
async fn unknown_tool_produces_an_error_result() {
    let agent = Agent::builder()
        .model(tool_use_then_text("missing"))
        .tool(echo_tool("echo"))
        .build()
        .await
        .unwrap();

    agent.invoke("go").await.unwrap();

    let messages = agent.messages();
    let results = messages
        .iter()
        .find(|m| result_text(m).contains("not found"))
        .unwrap();
    assert_eq!(results.role, Role::User);
    assert!(result_text(results).contains("Tool 'missing' not found in registry"));
}

#[tokio::test]
async fn before_tool_call_cancel_with_a_custom_reason_becomes_the_error_result() {
    let agent = Agent::builder()
        .model(tool_use_then_text("echo"))
        .tool(echo_tool("echo"))
        .build()
        .await
        .unwrap();

    agent
        .hooks()
        .add_sync_callback(|event: &mut BeforeToolCallEvent| {
            event.cancel = Cancel::Reason("not allowed".to_owned());
            Ok(())
        });

    agent.invoke("go").await.unwrap();

    let messages = agent.messages();
    let result_message = messages
        .iter()
        .find(|m| m.role == Role::User && !result_text(m).is_empty());
    assert_eq!(result_text(result_message.unwrap()), "not allowed");
}

#[tokio::test]
async fn before_tool_call_selected_tool_overrides_the_tool_that_runs() {
    let agent = Agent::builder()
        .model(tool_use_then_text("echo"))
        .tool(echo_tool("echo"))
        .build()
        .await
        .unwrap();

    let replacement: ToolRef = Arc::new(
        tool("replacement")
            .description("Replacement")
            .handler(|_input, _ctx| async move { Ok(json!("replaced")) }),
    );
    agent
        .hooks()
        .add_sync_callback(move |event: &mut BeforeToolCallEvent| {
            event.selected_tool = Some(Arc::clone(&replacement));
            Ok(())
        });

    agent.invoke("go").await.unwrap();

    let messages = agent.messages();
    let result_message = messages
        .iter()
        .find(|m| m.role == Role::User && !result_text(m).is_empty())
        .unwrap();
    assert_eq!(result_text(result_message), "replaced");
}

#[tokio::test]
async fn after_tool_call_can_replace_the_result() {
    let agent = Agent::builder()
        .model(tool_use_then_text("echo"))
        .tool(echo_tool("echo"))
        .build()
        .await
        .unwrap();

    agent.hooks().add_sync_callback(|event: &mut AfterToolCallEvent| {
        event.result.content = vec![strands_agents::types::ToolResultContent::Text(
            "patched".to_owned(),
        )];
        event.result.status = ToolResultStatus::Success;
        Ok(())
    });

    agent.invoke("go").await.unwrap();

    let messages = agent.messages();
    let result_message = messages
        .iter()
        .find(|m| m.role == Role::User && !result_text(m).is_empty())
        .unwrap();
    assert_eq!(result_text(result_message), "patched");
}

#[tokio::test]
async fn after_tools_end_turn_yes_stops_the_loop() {
    let model = tool_use_then_text("echo");
    let agent = Agent::builder()
        .model(Arc::clone(&model) as Arc<_>)
        .tool(echo_tool("echo"))
        .build()
        .await
        .unwrap();
    agent.hooks().add_sync_callback(|event: &mut AfterToolsEvent| {
        event.end_turn = EndTurn::Yes;
        Ok(())
    });

    let result = agent.invoke("go").await.unwrap();

    assert_eq!(model.call_count(), 1);
    let last = agent.messages().last().unwrap().clone();
    assert_eq!(last.role, Role::Assistant);
    assert_eq!(
        last.content,
        vec![ContentBlock::Text(
            "Turn ended early by hook after tool execution".to_owned()
        )]
    );
    assert_eq!(result.stop_reason, StopReason::EndTurn);
}

#[tokio::test]
async fn after_tools_end_turn_text_appends_an_assistant_message() {
    let model = tool_use_then_text("echo");
    let agent = Agent::builder()
        .model(Arc::clone(&model) as Arc<_>)
        .tool(echo_tool("echo"))
        .build()
        .await
        .unwrap();
    agent.hooks().add_sync_callback(|event: &mut AfterToolsEvent| {
        event.end_turn = EndTurn::Text("stopping here".to_owned());
        Ok(())
    });

    let result = agent.invoke("go").await.unwrap();

    assert_eq!(model.call_count(), 1);
    let last = agent.messages().last().unwrap().clone();
    assert_eq!(last.role, Role::Assistant);
    assert_eq!(last.content, vec![ContentBlock::Text("stopping here".to_owned())]);
    assert_eq!(result.to_string(), "stopping here");
}

#[tokio::test]
async fn after_tools_end_turn_blocks_appends_the_given_blocks() {
    let model = tool_use_then_text("echo");
    let agent = Agent::builder()
        .model(Arc::clone(&model) as Arc<_>)
        .tool(echo_tool("echo"))
        .build()
        .await
        .unwrap();
    agent.hooks().add_sync_callback(|event: &mut AfterToolsEvent| {
        event.end_turn = EndTurn::Blocks(vec![ContentBlock::Text("from blocks".to_owned())]);
        Ok(())
    });

    agent.invoke("go").await.unwrap();

    let last = agent.messages().last().unwrap().clone();
    assert_eq!(last.role, Role::Assistant);
    assert_eq!(last.content, vec![ContentBlock::Text("from blocks".to_owned())]);
    assert_eq!(model.call_count(), 1);
}

#[tokio::test]
async fn before_tools_cancel_turns_every_result_into_an_error() {
    let agent = Agent::builder()
        .model(tool_use_then_text("echo"))
        .tool(echo_tool("echo"))
        .build()
        .await
        .unwrap();
    agent.hooks().add_sync_callback(|event: &mut BeforeToolsEvent| {
        event.cancel = Cancel::Reason("tools off".to_owned());
        Ok(())
    });

    agent.invoke("go").await.unwrap();

    let messages = agent.messages();
    let result_message = messages
        .iter()
        .find(|m| m.role == Role::User && !result_text(m).is_empty())
        .unwrap();
    assert_eq!(result_text(result_message), "tools off");
}

#[tokio::test]
async fn before_invocation_cancel_appends_only_an_assistant_message() {
    let model = text_model(&["never"]);
    let agent = Agent::new(Arc::clone(&model) as Arc<_>).await.unwrap();
    agent
        .hooks()
        .add_sync_callback(|event: &mut BeforeInvocationEvent| {
            event.cancel = Cancel::Yes;
            Ok(())
        });

    let result = agent.invoke("go").await.unwrap();

    assert_eq!(model.call_count(), 0);
    let messages = agent.messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, Role::Assistant);
    assert_eq!(
        messages[0].content,
        vec![ContentBlock::Text("invocation denied by hook".to_owned())]
    );
    assert_eq!(result.to_string(), "invocation denied by hook");
}

#[tokio::test]
async fn agent_result_displays_the_concatenated_text() {
    let agent = Agent::new(text_model(&["Hello there"])).await.unwrap();

    let result = agent.invoke("Hi").await.unwrap();

    assert_eq!(result.to_string(), "Hello there");
    assert_eq!(result.stop_reason, StopReason::EndTurn);
}

#[tokio::test]
async fn stream_yields_the_agent_result_last() {
    let agent = Agent::new(text_model(&["hi"])).await.unwrap();
    let mut stream = agent.stream("go", InvokeOptions::default());

    let mut last = None;
    while let Some(event) = stream.next().await {
        last = Some(event.unwrap());
    }

    assert!(matches!(last, Some(AgentStreamEvent::AgentResult(_))));
}
