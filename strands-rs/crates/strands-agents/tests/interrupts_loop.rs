//! Interrupts through the public API: raise, stop, resume, and finish without a second model call.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::json;
use strands_agents::agent::INTERRUPT_RESUME_REQUIRED_MESSAGE;
use strands_agents::hooks::BeforeToolCallEvent;
use strands_agents::interventions::vended::HumanInTheLoop;
use strands_agents::interventions::{InterventionHandler, CONFIRMATION_FAILED_PREFIX};
use strands_agents::test_support::{MockMessageModel, MockResponse};
use strands_agents::tools::FunctionTool;
use strands_agents::types::{ContentBlock, Role, StopReason, ToolUseBlock};
use strands_agents::{Agent, InterruptParams, InterruptResponseContent, InvokeInput, Message};

/// Assistant message asking for `tools`, each with an empty input.
fn tool_use_message(tools: &[(&str, &str)]) -> Message {
    Message::new(
        Role::Assistant,
        tools
            .iter()
            .map(|(name, id)| ContentBlock::ToolUse(ToolUseBlock::new(*name, *id, json!({}))))
            .collect(),
    )
}

/// `approve`: interrupts for an approval, then returns what the human said.
fn approve_tool(runs: Arc<AtomicUsize>) -> Arc<FunctionTool> {
    Arc::new(
        FunctionTool::builder("approve")
            .description("Interrupts for approval, then reports the answer")
            .input_schema(json!({ "type": "object", "properties": {} }))
            .handler(move |_input, ctx| {
                let runs = Arc::clone(&runs);
                async move {
                    let answer: String = ctx.interrupt(
                        InterruptParams::new("confirm").with_reason("approve the write?"),
                    )?;
                    runs.fetch_add(1, Ordering::SeqCst);
                    Ok(json!(format!("did it: {answer}")))
                }
            }),
    )
}

/// A tool that always succeeds, counting its runs.
fn sibling_tool(name: &'static str, runs: Arc<AtomicUsize>) -> Arc<FunctionTool> {
    Arc::new(
        FunctionTool::builder(name)
            .description("Always succeeds")
            .input_schema(json!({ "type": "object", "properties": {} }))
            .handler(move |_input, _ctx| {
                let runs = Arc::clone(&runs);
                async move {
                    runs.fetch_add(1, Ordering::SeqCst);
                    Ok(json!("sibling done"))
                }
            }),
    )
}

fn model(messages: Vec<Message>) -> Arc<MockMessageModel> {
    MockMessageModel::shared(
        messages
            .into_iter()
            .map(MockResponse::message)
            .collect::<Vec<_>>(),
    )
}

#[tokio::test]
async fn tool_interrupt_stops_the_loop_and_resume_finishes_the_tool() {
    let runs = Arc::new(AtomicUsize::new(0));
    let model = model(vec![
        tool_use_message(&[("approve", "call-1")]),
        Message::assistant("all done"),
    ]);
    let agent = Agent::builder()
        .model_arc(Arc::clone(&model) as Arc<dyn strands_agents::models::Model>)
        .tool_arc(approve_tool(Arc::clone(&runs)))
        .build()
        .await
        .unwrap();

    let interrupted = agent.invoke("write the file").await.unwrap();
    assert_eq!(interrupted.stop_reason, StopReason::Interrupt);
    assert_eq!(interrupted.interrupts.len(), 1);
    let interrupt = &interrupted.interrupts[0];
    assert_eq!(interrupt.id, "tool:call-1:confirm");
    assert_eq!(interrupt.name, "confirm");
    assert_eq!(interrupt.reason, Some(json!("approve the write?")));
    assert_eq!(runs.load(Ordering::SeqCst), 0);
    // Nothing is appended: no dangling `toolUse` without its `toolResult`.
    assert_eq!(agent.messages().len(), 1);

    let resumed = agent
        .invoke(InvokeInput::InterruptResponses(vec![
            InterruptResponseContent::new(&interrupt.id, json!("yes")),
        ]))
        .await
        .unwrap();
    assert_eq!(resumed.stop_reason, StopReason::EndTurn);
    assert_eq!(resumed.text(), "all done");
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert!(agent.unanswered_interrupts().is_empty());
    // One call for the tool-use turn, one after the tool finished: the resume itself replays the
    // stored assistant message instead of calling the model again.
    assert_eq!(model.call_count(), 2);
    let results = agent
        .messages()
        .iter()
        .flat_map(|m| m.content.clone())
        .filter_map(|block| match block {
            ContentBlock::ToolResult(result) => Some(result),
            _ => None,
        })
        .count();
    assert_eq!(results, 1);
}

#[tokio::test]
async fn hook_interrupt_stops_the_loop_before_the_tool_runs() {
    let runs = Arc::new(AtomicUsize::new(0));
    let model = model(vec![
        tool_use_message(&[("ok", "call-1")]),
        Message::assistant("done"),
    ]);
    let agent = Agent::builder()
        .model_arc(Arc::clone(&model) as Arc<dyn strands_agents::models::Model>)
        .tool_arc(sibling_tool("ok", Arc::clone(&runs)))
        .build()
        .await
        .unwrap();
    let _cleanup = agent
        .hooks()
        .add_sync_callback(|event: &mut BeforeToolCallEvent| {
            let _: serde_json::Value = event.interrupt(InterruptParams::new("gate"))?;
            Ok(())
        });

    let interrupted = agent.invoke("go").await.unwrap();
    assert_eq!(interrupted.stop_reason, StopReason::Interrupt);
    assert_eq!(
        interrupted.interrupts[0].id,
        "hook:beforeToolCall:call-1:gate"
    );
    assert_eq!(runs.load(Ordering::SeqCst), 0);

    let resumed = agent
        .invoke(InvokeInput::InterruptResponses(vec![
            InterruptResponseContent::new(&interrupted.interrupts[0].id, json!(true)),
        ]))
        .await
        .unwrap();
    assert_eq!(resumed.stop_reason, StopReason::EndTurn);
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn invoking_with_new_input_while_interrupted_is_rejected() {
    let model = model(vec![tool_use_message(&[("approve", "call-1")])]);
    let agent = Agent::builder()
        .model_arc(Arc::clone(&model) as Arc<dyn strands_agents::models::Model>)
        .tool_arc(approve_tool(Arc::new(AtomicUsize::new(0))))
        .build()
        .await
        .unwrap();

    assert_eq!(
        agent.invoke("write the file").await.unwrap().stop_reason,
        StopReason::Interrupt
    );
    let error = agent.invoke("something else").await.unwrap_err();
    assert_eq!(error.to_string(), INTERRUPT_RESUME_REQUIRED_MESSAGE);
}

#[tokio::test]
async fn a_sibling_interrupt_keeps_the_completed_tool_result() {
    let approve_runs = Arc::new(AtomicUsize::new(0));
    let sibling_runs = Arc::new(AtomicUsize::new(0));
    let model = model(vec![
        tool_use_message(&[("approve", "call-1"), ("ok", "call-2")]),
        Message::assistant("both done"),
    ]);
    let agent = Agent::builder()
        .model_arc(Arc::clone(&model) as Arc<dyn strands_agents::models::Model>)
        .tool_arc(approve_tool(Arc::clone(&approve_runs)))
        .tool_arc(sibling_tool("ok", Arc::clone(&sibling_runs)))
        .build()
        .await
        .unwrap();

    let interrupted = agent.invoke("do both").await.unwrap();
    assert_eq!(interrupted.stop_reason, StopReason::Interrupt);
    // The sibling ran to completion even though its batch mate interrupted.
    assert_eq!(sibling_runs.load(Ordering::SeqCst), 1);

    let resumed = agent
        .invoke(InvokeInput::InterruptResponses(vec![
            InterruptResponseContent::new(&interrupted.interrupts[0].id, json!("yes")),
        ]))
        .await
        .unwrap();
    assert_eq!(resumed.stop_reason, StopReason::EndTurn);
    assert_eq!(approve_runs.load(Ordering::SeqCst), 1);
    // The sibling is *not* re-run on resume: its stored result is spliced back in.
    assert_eq!(sibling_runs.load(Ordering::SeqCst), 1);

    let results: Vec<_> = agent
        .messages()
        .iter()
        .flat_map(|m| m.content.clone())
        .filter_map(|block| match block {
            ContentBlock::ToolResult(result) => Some(result.tool_use_id),
            _ => None,
        })
        .collect();
    assert_eq!(results, vec!["call-1".to_owned(), "call-2".to_owned()]);
}

#[tokio::test]
async fn hitl_with_a_scripted_ask_approves_and_denies_inline() {
    for (answer, expect_run) in [("y", true), ("n", false)] {
        let runs = Arc::new(AtomicUsize::new(0));
        let model = model(vec![
            tool_use_message(&[("ok", "call-1")]),
            Message::assistant("finished"),
        ]);
        let hitl: Arc<dyn InterventionHandler> = Arc::new(HumanInTheLoop::new().ask(Arc::new(
            move |prompt: String| {
                assert_eq!(prompt, "Approve \"ok\"?\n  Input: {}");
                Box::pin(async move { Ok(json!(answer)) }) as _
            },
        )));
        let agent = Agent::builder()
            .model_arc(Arc::clone(&model) as Arc<dyn strands_agents::models::Model>)
            .tool_arc(sibling_tool("ok", Arc::clone(&runs)))
            .interventions([hitl])
            .unwrap()
            .build()
            .await
            .unwrap();

        let result = agent.invoke("go").await.unwrap();
        assert_eq!(result.stop_reason, StopReason::EndTurn);
        assert_eq!(runs.load(Ordering::SeqCst), usize::from(expect_run));
        if !expect_run {
            let denied = agent
                .messages()
                .iter()
                .flat_map(|m| m.content.clone())
                .any(|block| match block {
                    ContentBlock::ToolResult(result) => {
                        format!("{:?}", result.content).contains(CONFIRMATION_FAILED_PREFIX)
                    }
                    _ => false,
                });
            assert!(denied, "expected a CONFIRMATION_FAILED tool result");
        }
    }
}

#[tokio::test]
async fn hitl_without_an_ask_interrupts_and_resumes() {
    let runs = Arc::new(AtomicUsize::new(0));
    let model = model(vec![
        tool_use_message(&[("ok", "call-1")]),
        Message::assistant("finished"),
    ]);
    let hitl: Arc<dyn InterventionHandler> = Arc::new(HumanInTheLoop::new());
    let agent = Agent::builder()
        .model_arc(Arc::clone(&model) as Arc<dyn strands_agents::models::Model>)
        .tool_arc(sibling_tool("ok", Arc::clone(&runs)))
        .interventions([hitl])
        .unwrap()
        .build()
        .await
        .unwrap();

    let interrupted = agent.invoke("go").await.unwrap();
    assert_eq!(interrupted.stop_reason, StopReason::Interrupt);
    assert_eq!(
        interrupted.interrupts[0].id,
        "hook:beforeToolCall:call-1:strands:human-in-the-loop"
    );
    assert_eq!(
        interrupted.interrupts[0].reason,
        Some(json!("Approve \"ok\"?\n  Input: {}"))
    );
    assert_eq!(runs.load(Ordering::SeqCst), 0);

    let resumed = agent
        .invoke(InvokeInput::InterruptResponses(vec![
            InterruptResponseContent::new(&interrupted.interrupts[0].id, json!("yes")),
        ]))
        .await
        .unwrap();
    assert_eq!(resumed.stop_reason, StopReason::EndTurn);
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn hitl_allowed_tools_skip_approval_entirely() {
    let runs = Arc::new(AtomicUsize::new(0));
    let model = model(vec![
        tool_use_message(&[("ok", "call-1")]),
        Message::assistant("finished"),
    ]);
    let hitl: Arc<dyn InterventionHandler> = Arc::new(HumanInTheLoop::new().allowed_tools(["*"]));
    let agent = Agent::builder()
        .model_arc(Arc::clone(&model) as Arc<dyn strands_agents::models::Model>)
        .tool_arc(sibling_tool("ok", Arc::clone(&runs)))
        .interventions([hitl])
        .unwrap()
        .build()
        .await
        .unwrap();

    let result = agent.invoke("go").await.unwrap();
    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}
