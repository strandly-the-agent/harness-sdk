//! `ToolAgent` view and the `call_tool` seam (design: `ToolAgent`, decision #62).

use serde_json::json;

use super::*;
use crate::tools::fixtures::{RecordingCaller, mock_agent_parts};
use crate::types::tools::ToolResultStatus;

#[tokio::test]
async fn call_tool_runs_through_the_installed_caller() {
    let result = ToolResultBlock::new("t-1", ToolResultStatus::Success, vec!["ok".into()]);
    let caller = Arc::new(RecordingCaller::new(ToolCallOutcome::Result(result.clone())));
    let agent = ToolAgent::new(mock_agent_parts(Arc::clone(&caller) as Arc<dyn ToolCaller>));
    assert_eq!(
        agent.call_tool("shell", json!({ "command": "ls" })).await,
        ToolCallOutcome::Result(result)
    );
    assert_eq!(
        caller.calls(),
        vec![("shell".to_owned(), json!({ "command": "ls" }))]
    );
}

#[tokio::test]
async fn call_tool_reports_interrupts_and_cancels_as_their_own_outcomes() {
    let interrupt = Interrupt::new("hook:beforeToolCall:t-1:approve", "approve");
    let caller = Arc::new(RecordingCaller::new(ToolCallOutcome::Interrupt(vec![
        interrupt.clone(),
    ])));
    let agent = ToolAgent::new(mock_agent_parts(caller));
    assert_eq!(
        agent.call_tool("shell", json!({})).await,
        ToolCallOutcome::Interrupt(vec![interrupt])
    );

    let cancelled = ToolResultBlock::new("t-1", ToolResultStatus::Error, vec!["shell is disabled".into()]);
    let caller = Arc::new(RecordingCaller::new(ToolCallOutcome::Cancelled(
        cancelled.clone(),
    )));
    let agent = ToolAgent::new(mock_agent_parts(caller));
    assert_eq!(
        agent.call_tool("shell", json!({})).await,
        ToolCallOutcome::Cancelled(cancelled)
    );
}

#[test]
fn clones_share_the_agent_handles() {
    let caller = Arc::new(RecordingCaller::new(ToolCallOutcome::Interrupt(Vec::new())));
    let mut parts = mock_agent_parts(caller);
    let context = InvocationContext::new();
    parts.invocation_context = Some(context.clone());
    let agent = ToolAgent::new(parts);
    let view = agent.clone();
    agent.app_state().set("region", "eu").unwrap();
    assert_eq!(
        view.app_state().get::<String>("region").unwrap().as_deref(),
        Some("eu")
    );
    assert_eq!(view.id(), "mock-agent");
    assert_eq!(view.name(), "Strands Agent");
    assert!(view.messages().is_empty());
    assert!(view.tool_registry().list().is_empty());
    assert!(Arc::ptr_eq(view.model(), view.aux_model()));
    assert!(view.invocation_context().unwrap().same_request(&context));
    assert!(!InvocationContext::new().same_request(&context));
    let _ = view.cancel_handle().signal();
}
