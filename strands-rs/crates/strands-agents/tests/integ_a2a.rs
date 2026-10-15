//! Live A2A integration test: a served agent backed by real Bedrock, asked a question over the
//! A2A protocol by an [`A2AAgent`]. `#[ignore]`d; run with:
//!
//! ```text
//! AWS_REGION=us-west-2 cargo test --all-features --test integ_a2a -- --ignored --nocapture
//! ```

#![cfg(all(feature = "a2a", feature = "bedrock"))]

use strands_agents::a2a::{A2AAgent, A2AExecutor, A2AServer};
use strands_agents::models::{BedrockModel, BedrockModelConfig};
use strands_agents::Agent;

const MODEL_ID: &str = "us.anthropic.claude-haiku-4-5-20251001-v1:0";

#[tokio::test]
#[ignore = "live Bedrock: serves a real agent over A2A and invokes it with A2AAgent"]
async fn a2a_agent_asks_a_bedrock_backed_served_agent_a_question() {
    let model = match BedrockModel::new(BedrockModelConfig {
        model_id: Some(MODEL_ID.to_owned()),
        max_tokens: Some(256),
        ..Default::default()
    })
    .await
    {
        Ok(model) => model,
        Err(error) => {
            eprintln!("skipping: no Bedrock credentials ({error})");
            return;
        }
    };
    let model = std::sync::Arc::new(model);

    let handle = A2AServer::builder("Capital Expert")
        .description("Answers geography questions")
        .executor(A2AExecutor::with_factory(move |_| {
            Agent::builder()
                .model(model.clone() as std::sync::Arc<dyn strands_agents::models::Model>)
                .system_prompt("Answer in one short sentence.")
                .build()
                .unwrap()
        }))
        .build()
        .unwrap()
        .serve()
        .await
        .unwrap();

    let remote = A2AAgent::builder(format!("http://{}", handle.local_addr()))
        .continue_context(true)
        .build();
    let card = remote.agent_card().await.unwrap().clone();
    assert_eq!(card.name, "Capital Expert");

    let result = remote.invoke("What is the capital of France?").await.unwrap();
    println!("remote answer: {}", result.text());

    assert!(
        result.text().to_lowercase().contains("paris"),
        "expected Paris, got {}",
        result.text()
    );
    assert!(remote.context_id().is_some(), "the server assigned a context");
    remote.reset_context();
    assert!(remote.context_id().is_none());
    handle.shutdown().await;
}
