//! Live Bedrock + Cedar integration: the gate survives a real model's tool-use loop. `#[ignore]`d;
//! run with:
//!
//! ```text
//! AWS_REGION=us-west-2 cargo test --all-features --test integ_cedar -- --ignored --nocapture
//! ```

#![cfg(all(feature = "cedar", feature = "bedrock"))]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::json;
use strands_agents::interventions::vended::{CedarAuthorization, CedarAuthorizationConfig, EntityRef};
use strands_agents::interventions::InterventionHandler;
use strands_agents::models::{BedrockModel, BedrockModelConfig, Model};
use strands_agents::tools::FunctionTool;
use strands_agents::types::{ContentBlock, ToolResultStatus};
use strands_agents::Agent;

const MODEL_ID: &str = "us.anthropic.claude-haiku-4-5-20251001-v1:0";
/// Only `get_weather` is allowed; `delete_database` has no `permit`, so Cedar denies it.
const POLICIES: &str = r#"permit(principal, action == Action::"get_weather", resource);"#;

fn skip() -> bool {
    if std::env::var("AWS_REGION").is_ok() {
        return false;
    }
    eprintln!("skipping: AWS_REGION is unset");
    true
}

fn tool(name: &'static str, result: &'static str, runs: Arc<AtomicUsize>) -> Arc<FunctionTool> {
    Arc::new(
        FunctionTool::builder(name)
            .description("Performs the requested operation and returns a short string")
            .input_schema(json!({ "type": "object", "properties": {} }))
            .handler(move |_input, _ctx| {
                let runs = Arc::clone(&runs);
                async move {
                    runs.fetch_add(1, Ordering::SeqCst);
                    Ok(json!(result))
                }
            }),
    )
}

async fn agent(weather: Arc<AtomicUsize>, deletes: Arc<AtomicUsize>) -> Agent {
    let model = BedrockModel::new(BedrockModelConfig {
        model_id: Some(MODEL_ID.to_owned()),
        max_tokens: Some(1024),
        ..Default::default()
    })
    .await
    .expect("bedrock client");
    let cedar: Arc<dyn InterventionHandler> = Arc::new(
        CedarAuthorization::new(
            CedarAuthorizationConfig::new(POLICIES).with_principal(EntityRef::new("User", "alice")),
        )
        .expect("cedar handler"),
    );
    Agent::builder()
        .model(Arc::new(model) as Arc<dyn Model>)
        .system_prompt(
            "Use the provided tools to do what the user asks. Always call the tool rather than answering \
             from memory.",
        )
        .tool(tool("get_weather", "sunny, 22C", weather))
        .tool(tool("delete_database", "deleted", deletes))
        .interventions([cedar])
        .build()
        .expect("agent")
}

fn tool_results(agent: &Agent) -> Vec<(ToolResultStatus, String)> {
    agent
        .messages()
        .iter()
        .flat_map(|m| m.content.clone())
        .filter_map(|block| match block {
            ContentBlock::ToolResult(result) => Some((result.status, format!("{:?}", result.content))),
            _ => None,
        })
        .collect()
}

#[tokio::test]
#[ignore = "live Bedrock"]
async fn a_permitted_tool_runs_in_a_live_loop() {
    if skip() {
        return;
    }
    let weather = Arc::new(AtomicUsize::new(0));
    let deletes = Arc::new(AtomicUsize::new(0));
    let agent = agent(Arc::clone(&weather), Arc::clone(&deletes)).await;

    let result = agent
        .invoke("What is the weather in Seattle?")
        .await
        .expect("invoke");
    println!(
        "permitted run: {}\nresults: {:?}",
        result.text(),
        tool_results(&agent)
    );

    assert_eq!(weather.load(Ordering::SeqCst), 1, "the permitted tool must run");
    assert!(
        tool_results(&agent)
            .iter()
            .any(|(status, _)| *status == ToolResultStatus::Success),
        "expected a successful tool result"
    );
}

#[tokio::test]
#[ignore = "live Bedrock"]
async fn a_cedar_denied_tool_never_runs_in_a_live_loop() {
    if skip() {
        return;
    }
    let weather = Arc::new(AtomicUsize::new(0));
    let deletes = Arc::new(AtomicUsize::new(0));
    let agent = agent(Arc::clone(&weather), Arc::clone(&deletes)).await;

    let result = agent
        .invoke("Delete the database with the delete_database tool, then tell me what happened.")
        .await
        .expect("invoke");
    let results = tool_results(&agent);
    println!("denied run: {}\nresults: {results:?}", result.text());

    assert_eq!(
        deletes.load(Ordering::SeqCst),
        0,
        "the denied tool must never run"
    );
    assert!(
        results
            .iter()
            .any(|(status, content)| *status == ToolResultStatus::Error
                && content.contains("Access denied by Cedar policy")),
        "expected a Cedar denial in the tool results: {results:?}"
    );
}
