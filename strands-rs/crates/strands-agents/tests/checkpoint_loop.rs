//! Checkpointing through the public API (`agent.checkpoint.test.ts`, `test/integ/agent.checkpoint.test.node.ts`).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::json;
use strands_agents::agent::{
    Checkpoint, CheckpointPosition, CheckpointResumeContent, InvokeInput, TakeSnapshotOptions,
};
use strands_agents::test_support::{MockMessageModel, MockResponse};
use strands_agents::tools::{function_tool, ToolRef};
use strands_agents::types::{ContentBlock, Role, StopReason, ToolUseBlock};
use strands_agents::{Agent, Message};

fn tool_use_message(id: &str) -> Message {
    Message::new(
        Role::Assistant,
        vec![ContentBlock::ToolUse(ToolUseBlock::new(
            "echo",
            id,
            json!({ "value": id }),
        ))],
    )
}

fn counting_tool() -> (ToolRef, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let tool = function_tool("echo")
        .description("Echoes its input")
        .handler(move |input, _ctx| {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(json!({ "echoed": input }))
            }
        });
    (Arc::new(tool), calls)
}

fn tool_then_text_model() -> Arc<MockMessageModel> {
    MockMessageModel::shared(vec![
        MockResponse::message(tool_use_message("call-1")),
        MockResponse::message(Message::assistant("All done")),
    ])
}

/// Resuming from `afterModel` re-invokes the model, which regenerates the same tool use (the
/// assistant message was never persisted) — `responses` mimics a deterministic model doing that.
fn regenerating_model(extra: Vec<MockResponse>) -> Arc<MockMessageModel> {
    let mut responses = vec![
        MockResponse::message(tool_use_message("call-1")),
        MockResponse::message(tool_use_message("call-1")),
    ];
    responses.extend(extra);
    MockMessageModel::shared(responses)
}

async fn checkpointing_agent(model: Arc<MockMessageModel>, tool: ToolRef) -> Agent {
    Agent::builder()
        .model_arc(model)
        .tool_ref(tool)
        .checkpointing(true)
        .build()
        .unwrap()
}

fn resume(checkpoint: Checkpoint) -> InvokeInput {
    InvokeInput::CheckpointResume(CheckpointResumeContent { checkpoint })
}

#[tokio::test]
async fn does_not_checkpoint_by_default() {
    let (tool, calls) = counting_tool();
    let agent = Agent::builder()
        .model_arc(tool_then_text_model())
        .tool_ref(tool)
        .build()
        .unwrap();

    let result = agent.invoke("Hi").await.unwrap();

    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(result.checkpoint, None);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn emits_an_after_model_checkpoint_before_running_tools() {
    let (tool, calls) = counting_tool();
    let agent = checkpointing_agent(tool_then_text_model(), tool).await;

    let result = agent.invoke("Hi").await.unwrap();

    assert_eq!(result.stop_reason, StopReason::Checkpoint);
    assert_eq!(
        result.checkpoint,
        Some(Checkpoint::new(CheckpointPosition::AfterModel, 0))
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(result.last_message.role, Role::Assistant);
}

#[tokio::test]
async fn resuming_from_after_model_runs_tools_then_emits_an_after_tools_checkpoint() {
    let (tool, calls) = counting_tool();
    let agent = checkpointing_agent(regenerating_model(vec![]), tool).await;

    let paused = agent.invoke("Hi").await.unwrap();
    let resumed = agent.invoke(resume(paused.checkpoint.unwrap())).await.unwrap();

    assert_eq!(resumed.stop_reason, StopReason::Checkpoint);
    assert_eq!(
        resumed.checkpoint,
        Some(Checkpoint::new(CheckpointPosition::AfterTools, 0))
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn resuming_from_after_tools_continues_and_increments_the_cycle_index() {
    let (tool, calls) = counting_tool();
    let agent = checkpointing_agent(
        regenerating_model(vec![MockResponse::message(tool_use_message("call-2"))]),
        tool,
    )
    .await;

    let after_model = agent.invoke("Hi").await.unwrap();
    let after_tools = agent
        .invoke(resume(after_model.checkpoint.unwrap()))
        .await
        .unwrap();
    let next_cycle = agent
        .invoke(resume(after_tools.checkpoint.unwrap()))
        .await
        .unwrap();

    assert_eq!(next_cycle.stop_reason, StopReason::Checkpoint);
    assert_eq!(
        next_cycle.checkpoint,
        Some(Checkpoint::new(CheckpointPosition::AfterModel, 1))
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn does_not_checkpoint_a_plain_end_turn_cycle() {
    let (tool, _) = counting_tool();
    let model = MockMessageModel::shared(vec![MockResponse::message(Message::assistant("Hello"))]);
    let agent = checkpointing_agent(model, tool).await;

    let result = agent.invoke("Hi").await.unwrap();

    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(result.checkpoint, None);
}

#[tokio::test]
async fn resume_requires_checkpointing_to_be_enabled() {
    let (tool, _) = counting_tool();
    let agent = Agent::builder()
        .model_arc(tool_then_text_model())
        .tool_ref(tool)
        .build()
        .unwrap();

    let error = agent
        .invoke(resume(Checkpoint::new(CheckpointPosition::AfterModel, 0)))
        .await
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "Received a checkpointResume block but the agent was created with checkpointing: false. \
         Pass checkpointing: true when constructing the Agent."
    );
}

#[tokio::test]
async fn rejects_a_checkpoint_with_an_incompatible_schema_version() {
    let error =
        Checkpoint::from_json(json!({ "position": "afterModel", "schemaVersion": "0.9" })).unwrap_err();

    assert_eq!(
        error.to_string(),
        "Checkpoints with schema version \"0.9\" are not compatible with current version 1.0."
    );
}

/// End to end: pause mid-run, snapshot, and finish the turn through a *fresh* agent that only
/// knows the snapshot and the checkpoint.
#[tokio::test]
async fn pauses_at_a_cycle_boundary_and_completes_through_a_fresh_agent() {
    let (tool, calls) = counting_tool();
    let first = checkpointing_agent(tool_then_text_model(), tool.clone()).await;

    let after_model = first.invoke("Use the echo tool").await.unwrap();
    assert_eq!(after_model.stop_reason, StopReason::Checkpoint);
    let snapshot = first.take_snapshot(TakeSnapshotOptions::default());
    let wire = serde_json::to_string(&snapshot).unwrap();
    let resume_json = after_model.checkpoint.unwrap().to_json();

    let fresh = checkpointing_agent(tool_then_text_model(), tool).await;
    fresh
        .load_snapshot(&serde_json::from_str(&wire).unwrap())
        .unwrap();
    let after_tools = fresh
        .invoke(resume(Checkpoint::from_json(resume_json).unwrap()))
        .await
        .unwrap();
    assert_eq!(
        after_tools.checkpoint,
        Some(Checkpoint::new(CheckpointPosition::AfterTools, 0))
    );

    let done = fresh
        .invoke(resume(after_tools.checkpoint.unwrap()))
        .await
        .unwrap();

    assert_eq!(done.stop_reason, StopReason::EndTurn);
    assert_eq!(done.to_string(), "All done");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "completed tools are not re-run");
    assert_eq!(fresh.messages()[0].text(), "Use the echo tool");
}
