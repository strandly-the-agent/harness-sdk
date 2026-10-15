//! LLM steering through the agent loop: guide, proceed, confirm (approve and deny), and a tool
//! ledger that accumulates across turns of one invocation.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use strands_agents::interventions::vended::steering::{LlmSteeringHandler, DEFAULT_LLM_STEERING_NAME};
use strands_agents::interventions::{CONFIRMATION_FAILED_PREFIX, GUIDANCE_PREFIX};
use strands_agents::test_support::{MockMessageModel, MockResponse};
use strands_agents::tools::FunctionTool;
use strands_agents::types::{ContentBlock, Role, StopReason, ToolUseBlock};
use strands_agents::{Agent, InterruptResponseContent, InvokeInput, Message};

/// Assistant message asking for `searchWeb` with an empty input.
fn tool_use_message(id: &str) -> Message {
    Message::new(
        Role::Assistant,
        vec![ContentBlock::ToolUse(ToolUseBlock::new(
            "searchWeb",
            id,
            json!({}),
        ))],
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

/// A steering model that answers each evaluation with the next `decision`.
fn judge(decisions: Vec<Value>) -> Arc<MockMessageModel> {
    model(
        decisions
            .into_iter()
            .map(|decision| {
                Message::new(
                    Role::Assistant,
                    vec![ContentBlock::ToolUse(ToolUseBlock::new(
                        "strands_structured_output",
                        "inner-1",
                        decision,
                    ))],
                )
            })
            .collect(),
    )
}

fn search_tool(runs: Arc<AtomicUsize>) -> Arc<FunctionTool> {
    Arc::new(
        FunctionTool::builder("searchWeb")
            .description("Searches the web")
            .input_schema(json!({ "type": "object", "properties": {} }))
            .handler(move |_input, _ctx| {
                let runs = Arc::clone(&runs);
                async move {
                    runs.fetch_add(1, Ordering::SeqCst);
                    Ok(json!("results"))
                }
            }),
    )
}

fn agent(model: Arc<MockMessageModel>, handler: LlmSteeringHandler, runs: Arc<AtomicUsize>) -> Agent {
    Agent::builder()
        .model(model)
        .tool(search_tool(runs))
        .intervention(handler)
        .build()
        .unwrap()
}

#[tokio::test]
async fn guide_cancels_the_tool_and_the_parent_model_sees_the_guidance() {
    let runs = Arc::new(AtomicUsize::new(0));
    let parent = model(vec![
        tool_use_message("call-1"),
        Message::assistant("using the docs instead"),
    ]);
    let judge = judge(vec![
        json!({ "type": "guide", "reason": "use the local docs, not the web" }),
    ]);
    let agent = agent(
        Arc::clone(&parent),
        LlmSteeringHandler::new("You are a steering agent.").model(Arc::clone(&judge)),
        Arc::clone(&runs),
    );

    let result = agent.invoke("look it up").await.unwrap();

    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(result.text(), "using the docs instead");
    assert_eq!(runs.load(Ordering::SeqCst), 0, "guided tool must not run");
    assert_eq!(judge.call_count(), 1);

    let second_turn = format!("{:?}", parent.calls()[1].0);
    assert!(
        second_turn.contains(GUIDANCE_PREFIX) && second_turn.contains("use the local docs, not the web"),
        "{second_turn}"
    );
    assert!(
        second_turn.contains(DEFAULT_LLM_STEERING_NAME),
        "guidance should name the handler: {second_turn}"
    );
}

#[tokio::test]
async fn proceed_lets_the_tool_run() {
    let runs = Arc::new(AtomicUsize::new(0));
    let parent = model(vec![tool_use_message("call-1"), Message::assistant("found it")]);
    let judge = judge(vec![json!({ "type": "proceed", "reason": "harmless lookup" })]);
    let agent = agent(
        Arc::clone(&parent),
        LlmSteeringHandler::new("You are a steering agent.").model(Arc::clone(&judge)),
        Arc::clone(&runs),
    );

    let result = agent.invoke("look it up").await.unwrap();

    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(result.text(), "found it");
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(judge.call_count(), 1);
}

#[tokio::test]
async fn confirm_interrupts_and_the_humans_answer_decides() {
    for (answer, expect_run) in [("yes", true), ("no", false)] {
        let runs = Arc::new(AtomicUsize::new(0));
        let parent = model(vec![tool_use_message("call-1"), Message::assistant("finished")]);
        // The gate is re-evaluated when the invocation resumes, hence two decisions.
        let decision = json!({ "type": "confirm", "reason": "searching the web needs approval" });
        let judge = judge(vec![decision.clone(), decision]);
        let agent = agent(
            Arc::clone(&parent),
            LlmSteeringHandler::new("You are a steering agent.").model(Arc::clone(&judge)),
            Arc::clone(&runs),
        );

        let interrupted = agent.invoke("look it up").await.unwrap();
        assert_eq!(interrupted.stop_reason, StopReason::Interrupt);
        assert_eq!(
            interrupted.interrupts[0].id,
            format!("hook:beforeToolCall:call-1:{DEFAULT_LLM_STEERING_NAME}")
        );
        assert_eq!(
            interrupted.interrupts[0].reason,
            Some(json!("searching the web needs approval"))
        );
        assert_eq!(runs.load(Ordering::SeqCst), 0);

        let resumed = agent
            .invoke(InvokeInput::InterruptResponses(vec![
                InterruptResponseContent::new(&interrupted.interrupts[0].id, json!(answer)),
            ]))
            .await
            .unwrap();

        assert_eq!(resumed.stop_reason, StopReason::EndTurn);
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
async fn the_tool_ledger_accumulates_across_turns_of_one_invocation() {
    let runs = Arc::new(AtomicUsize::new(0));
    let parent = model(vec![
        tool_use_message("call-1"),
        tool_use_message("call-2"),
        Message::assistant("done"),
    ]);
    let judge = judge(vec![
        json!({ "type": "proceed", "reason": "first lookup is fine" }),
        json!({ "type": "proceed", "reason": "second lookup is fine" }),
    ]);
    // Default providers: the handler attaches a `ToolLedgerProvider` of its own.
    let agent = agent(
        Arc::clone(&parent),
        LlmSteeringHandler::new("You are a steering agent.").model(Arc::clone(&judge)),
        Arc::clone(&runs),
    );

    let result = agent.invoke("look it up twice").await.unwrap();
    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    assert_eq!(judge.call_count(), 2);

    let first = judge.calls()[0].0[0].text();
    assert!(first.contains("\"id\": \"call-1\""), "{first}");
    assert!(!first.contains("call-2"), "{first}");

    let second = judge.calls()[1].0[0].text();
    assert!(
        second.contains("\"id\": \"call-1\"") && second.contains("\"status\": \"success\""),
        "the finished call should still be in the ledger: {second}"
    );
    assert!(
        second.contains("\"id\": \"call-2\"") && second.contains("\"status\": \"pending\""),
        "the call under evaluation should be pending: {second}"
    );
}
