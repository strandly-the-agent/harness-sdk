//! Live Bedrock tests for the model router. They call the real service and are `#[ignore]`d; run
//! with:
//!
//! ```text
//! AWS_REGION=us-west-2 cargo test --all-features --test integ_model_router -- --ignored
//! ```

#![cfg(feature = "bedrock")]

use std::sync::Arc;

use strands_agents::models::routing::{ClassifierStrategy, FallbackStrategy, ModelRouter, RoutingCandidate};
use strands_agents::models::{BedrockModel, BedrockModelConfig, Model};
use strands_agents::Agent;

const HAIKU: &str = "us.anthropic.claude-haiku-4-5-20251001-v1:0";
const MISSING: &str = "us.anthropic.does-not-exist-v1:0";

async fn bedrock(model_id: &str) -> Arc<dyn Model> {
    let config = BedrockModelConfig {
        model_id: Some(model_id.to_owned()),
        max_tokens: Some(64),
        ..Default::default()
    };
    Arc::new(BedrockModel::new(config).await.expect("bedrock client"))
}

#[tokio::test]
#[ignore = "live Bedrock"]
async fn falls_back_from_an_unknown_model_id_to_haiku() {
    let healthy = bedrock(HAIKU).await;
    let router = Arc::new(
        ModelRouter::new(vec![
            RoutingCandidate::new(bedrock(MISSING).await).with_name("missing"),
            RoutingCandidate::new(Arc::clone(&healthy)).with_name("haiku"),
        ])
        .unwrap()
        .with_strategy(FallbackStrategy),
    );
    let agent = Agent::builder()
        .model_router(Arc::clone(&router))
        .system_prompt("Answer with a single word.")
        .build()
        .unwrap();

    let result = agent.invoke("What is the capital of France?").await.unwrap();

    println!("answer: {}", result.last_message.text());
    assert!(
        result.last_message.text().to_lowercase().contains("paris"),
        "unexpected answer: {}",
        result.last_message.text()
    );
    let routed = router
        .routed_model(&agent, &result.invocation_state)
        .expect("a routed model");
    assert_eq!(routed.model_id().as_deref(), Some(HAIKU));
}

#[tokio::test]
#[ignore = "live Bedrock"]
async fn classifier_picks_a_candidate_from_its_description() {
    let coding = bedrock(HAIKU).await;
    let poetry = bedrock(HAIKU).await;
    let router = Arc::new(
        ModelRouter::new(vec![
            RoutingCandidate::new(Arc::clone(&coding))
                .with_name("coding")
                .with_description("Best for writing and debugging source code."),
            RoutingCandidate::new(Arc::clone(&poetry))
                .with_name("poetry")
                .with_description("Best for creative writing, poems and prose."),
        ])
        .unwrap()
        .with_strategy(ClassifierStrategy::new(bedrock(HAIKU).await)),
    );
    let agent = Agent::builder()
        .model_router(Arc::clone(&router))
        .system_prompt("Be brief.")
        .build()
        .unwrap();

    let result = agent
        .invoke("Write a two-line poem about the sea.")
        .await
        .unwrap();

    println!("answer: {}", result.last_message.text());
    assert!(!result.last_message.text().is_empty());
    let routed = router
        .routed_model(&agent, &result.invocation_state)
        .expect("a routed model");
    println!("routed: {:?}", routed.model_id());
    assert!(Arc::ptr_eq(&routed, &poetry) || Arc::ptr_eq(&routed, &coding));
}
