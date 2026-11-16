//! Live model-routing test against Bedrock (TS checklist:
//! strands-ts/test/integ/models/classifier-strategy.test.ts). It uses the default AWS
//! credential chain and skips with a message when no credentials resolve. Run with
//! `cargo test -p strands --all-features -- --ignored live_`.
#![cfg(feature = "bedrock")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test code"
)]

mod live_support;

use std::sync::{Arc, Mutex};

use live_support::{REGION, credentials_available};
use serde_json::json;
use strands::hooks::AfterModelCallEvent;
use strands::models::bedrock::{BedrockModel, BedrockModelConfig, BedrockModelOptions};
use strands::models::model::Model;
use strands::models::routing::{ClassifierStrategy, ModelRouter, ModelRouterOptions, RoutingCandidate};
use strands::{Agent, StopReason};

const HAIKU_MODEL_ID: &str = "us.anthropic.claude-haiku-4-5-20251001-v1:0";
const NOVA_LITE_MODEL_ID: &str = "us.amazon.nova-lite-v1:0";
const NOVA_PRO_MODEL_ID: &str = "us.amazon.nova-pro-v1:0";

fn model(model_id: &str, max_tokens: u64, temperature: Option<f64>) -> Arc<dyn Model> {
    Arc::new(BedrockModel::new(BedrockModelOptions {
        config: BedrockModelConfig {
            model_id: Some(model_id.into()),
            max_tokens: Some(max_tokens),
            temperature,
            ..Default::default()
        },
        region: Some(REGION.into()),
        ..Default::default()
    }))
}

fn candidate(model_id: &str, name: &str, description: &str) -> RoutingCandidate {
    let metadata = json!({"provider": "bedrock", "modelId": model_id});
    RoutingCandidate::new(model(model_id, 512, None))
        .with_name(name)
        .with_description(description)
        .with_metadata(metadata.as_object().unwrap().clone())
}

#[tokio::test]
#[ignore = "live bedrock"]
async fn live_classifies_and_serves_a_request_on_the_expected_candidate() {
    if !credentials_available().await {
        return;
    }
    let router = ModelRouter::with_options(
        [
            candidate(
                HAIKU_MODEL_ID,
                "fast model",
                "Best suited to concise factual questions and routine requests.",
            ),
            candidate(
                NOVA_PRO_MODEL_ID,
                "advanced model",
                "Best suited to complex systems design with several interacting constraints.",
            ),
            candidate(
                NOVA_LITE_MODEL_ID,
                "balanced model",
                "Best suited to summaries and moderately complex general requests.",
            ),
        ],
        ModelRouterOptions {
            strategy: Some(Arc::new(ClassifierStrategy::new(model(
                HAIKU_MODEL_ID,
                64,
                Some(0.0),
            )))),
            ..Default::default()
        },
    )
    .unwrap();
    let mut agent = Agent::builder()
        .model(router)
        .system_prompt("You are just an agent")
        .printer(false)
        .build()
        .unwrap();
    let served = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&served);
    agent.add_hook(move |event: &mut AfterModelCallEvent<'_>| {
        seen.lock().unwrap().push(event.model().model_id());
    });
    let request = "Design a backward-compatible migration from region-local to globally unique idempotency keys for an \
                   active-active payment service. Account for concurrent requests, mixed-version deployments, rollback, \
                   reconciliation, and monitoring. Return exactly three concise bullets.";
    let result = agent.invoke(request).await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert!(!result.to_string().trim().is_empty());
    let usage = result.metrics().accumulated_usage();
    assert!(usage.input_tokens() > 0 && usage.output_tokens() > 0);
    // The classifier picked the advanced candidate, and that model served the request.
    let served = served.lock().unwrap().clone();
    assert_eq!(
        served.first(),
        Some(&Some(NOVA_PRO_MODEL_ID.to_owned())),
        "{served:?}"
    );
    agent.shutdown().await.unwrap();
}
