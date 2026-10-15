//! Cedar authorization through the real agent loop: one permitted tool, one denied.
#![cfg(feature = "cedar")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::json;
use strands_agents::interventions::vended::cedar::{CedarAuthorizationConfig, EntityRef};
use strands_agents::interventions::vended::CedarAuthorization;
use strands_agents::interventions::InterventionHandler;
use strands_agents::test_support::{MockMessageModel, MockResponse};
use strands_agents::tools::FunctionTool;
use strands_agents::types::{ContentBlock, Role, StopReason, ToolResultStatus, ToolUseBlock};
use strands_agents::{Agent, Message};

const POLICIES: &str = r#"permit(principal, action == Action::"search", resource);"#;

fn tool(name: &'static str, runs: Arc<AtomicUsize>) -> Arc<FunctionTool> {
    Arc::new(
        FunctionTool::builder(name)
            .description("Always succeeds")
            .input_schema(json!({ "type": "object", "properties": {} }))
            .handler(move |_input, _ctx| {
                let runs = Arc::clone(&runs);
                async move {
                    runs.fetch_add(1, Ordering::SeqCst);
                    Ok(json!(format!("{name} done")))
                }
            }),
    )
}

#[tokio::test]
async fn permitted_tool_runs_and_denied_tool_returns_an_error_result() {
    let searches = Arc::new(AtomicUsize::new(0));
    let deletes = Arc::new(AtomicUsize::new(0));
    let model = MockMessageModel::shared(vec![
        MockResponse::message(Message::new(
            Role::Assistant,
            vec![
                ContentBlock::ToolUse(ToolUseBlock::new("search", "call-1", json!({}))),
                ContentBlock::ToolUse(ToolUseBlock::new("delete", "call-2", json!({}))),
            ],
        )),
        MockResponse::message(Message::assistant("finished")),
    ]);
    let cedar: Arc<dyn InterventionHandler> = Arc::new(
        CedarAuthorization::new(
            CedarAuthorizationConfig::new(POLICIES).with_principal(EntityRef::new("User", "alice")),
        )
        .unwrap(),
    );
    let agent = Agent::builder()
        .model(Arc::clone(&model) as Arc<dyn strands_agents::models::Model>)
        .tool(tool("search", Arc::clone(&searches)))
        .tool(tool("delete", Arc::clone(&deletes)))
        .interventions([cedar])
        .build()
        .unwrap();

    let result = agent.invoke("go").await.unwrap();
    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(searches.load(Ordering::SeqCst), 1);
    assert_eq!(deletes.load(Ordering::SeqCst), 0, "denied tool must not run");

    let results: Vec<_> = agent
        .messages()
        .iter()
        .flat_map(|m| m.content.clone())
        .filter_map(|block| match block {
            ContentBlock::ToolResult(result) => Some(result),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 2);

    let permitted = &results[0];
    assert_eq!(permitted.tool_use_id, "call-1");
    assert_eq!(permitted.status, ToolResultStatus::Success);
    assert!(format!("{:?}", permitted.content).contains("search done"));

    let denied = &results[1];
    assert_eq!(denied.tool_use_id, "call-2");
    assert_eq!(denied.status, ToolResultStatus::Error);
    assert!(
        format!("{:?}", denied.content).contains("Access denied by Cedar policy"),
        "unexpected denial content: {:?}",
        denied.content
    );
}
