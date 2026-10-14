//! `Agent::as_tool` through the real loop: a parent agent calls a child agent as a tool.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::json;
use strands_agents::agent::AgentAsToolOptions;
use strands_agents::test_support::{MockMessageModel, MockResponse};
use strands_agents::tools::FunctionTool;
use strands_agents::types::{ContentBlock, Role, StopReason, ToolUseBlock};
use strands_agents::{Agent, InterruptParams, InterruptResponseContent, InvokeInput, Message};

fn model(messages: Vec<Message>) -> Arc<MockMessageModel> {
    MockMessageModel::shared(messages.into_iter().map(MockResponse::message).collect())
}

/// Assistant message calling `name` with `input`.
fn tool_use(name: &str, id: &str, input: serde_json::Value) -> Message {
    Message::new(
        Role::Assistant,
        vec![ContentBlock::ToolUse(ToolUseBlock::new(name, id, input))],
    )
}

/// `approve`: interrupts for an approval, then reports the answer.
fn approve_tool(runs: Arc<AtomicUsize>) -> Arc<FunctionTool> {
    Arc::new(
        FunctionTool::builder("approve")
            .description("Interrupts for approval, then reports the answer")
            .input_schema(json!({ "type": "object", "properties": {} }))
            .handler(move |_input, ctx| {
                let runs = Arc::clone(&runs);
                async move {
                    let answer: String = ctx.interrupt(InterruptParams::new("confirm"))?;
                    runs.fetch_add(1, Ordering::SeqCst);
                    Ok(json!(format!("did it: {answer}")))
                }
            }),
    )
}

#[tokio::test]
async fn parent_calls_the_child_agent_tool_and_gets_its_text_back() {
    let child = Agent::builder()
        .name("researcher")
        .description("Researches things")
        .model(model(vec![Message::assistant("the answer is 42")]))
        .build()
        .unwrap();

    let parent = Agent::builder()
        .model(model(vec![
            tool_use("researcher", "call-1", json!({ "prompt": "what is the answer?" })),
            Message::assistant("the researcher says 42"),
        ]))
        .tool(child.as_tool(AgentAsToolOptions::default()))
        .build()
        .unwrap();

    let result = parent.invoke("ask the researcher").await.unwrap();

    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(result.text(), "the researcher says 42");
    // The child's text came back as the tool result the parent fed to the model.
    let tool_result = parent
        .messages()
        .iter()
        .flat_map(|m| m.content.clone())
        .find_map(|block| match block {
            ContentBlock::ToolResult(r) => Some(r),
            _ => None,
        })
        .expect("tool result in the conversation");
    assert_eq!(tool_result.text(), "the answer is 42");
    // The child ran its own turn.
    assert_eq!(child.messages().len(), 2);
    assert_eq!(child.messages()[0].text(), "what is the answer?");
}

#[tokio::test]
async fn a_child_interrupt_is_namespaced_in_the_parent_and_resumes_the_child() {
    let runs = Arc::new(AtomicUsize::new(0));
    let child = Agent::builder()
        .name("worker")
        .model(model(vec![
            tool_use("approve", "child-1", json!({})),
            Message::assistant("all done"),
        ]))
        .tool(approve_tool(Arc::clone(&runs)))
        .build()
        .unwrap();

    let parent = Agent::builder()
        .model(model(vec![
            tool_use("worker", "call:1", json!({ "prompt": "do the thing" })),
            Message::assistant("the worker finished"),
        ]))
        .tool(child.as_tool(AgentAsToolOptions::default()))
        .build()
        .unwrap();

    let interrupted = parent.invoke("delegate").await.unwrap();

    assert_eq!(interrupted.stop_reason, StopReason::Interrupt);
    assert_eq!(interrupted.interrupts.len(), 1);
    // Namespaced per tool call, with the tool use id percent-encoded.
    assert_eq!(
        interrupted.interrupts[0].id,
        "agent_as_tool:call%3A1:tool:child-1:confirm"
    );
    assert_eq!(interrupted.interrupts[0].name, "confirm");
    assert_eq!(runs.load(Ordering::SeqCst), 0);

    let resumed = parent
        .invoke(InvokeInput::InterruptResponses(vec![
            InterruptResponseContent::new(&interrupted.interrupts[0].id, json!("yes")),
        ]))
        .await
        .unwrap();

    assert_eq!(resumed.stop_reason, StopReason::EndTurn);
    assert_eq!(resumed.text(), "the worker finished");
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert!(parent.unanswered_interrupts().is_empty());
}
