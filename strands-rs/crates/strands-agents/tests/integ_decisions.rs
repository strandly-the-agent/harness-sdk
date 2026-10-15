//! Live Bedrock decision test. It calls the real service and is `#[ignore]`d; run with:
//!
//! ```text
//! AWS_REGION=us-west-2 cargo test --all-features -p strands-agents --test integ_decisions -- --ignored
//! ```

#![cfg(feature = "bedrock")]

use strands_agents::decisions::{Answer, DecisionModelExt, LlmDecisionModel, Questions};
use strands_agents::models::{BedrockModel, BedrockModelConfig};

const MODEL_ID: &str = "us.anthropic.claude-haiku-4-5-20251001-v1:0";

#[tokio::test]
#[ignore = "live Bedrock"]
async fn answers_boolean_and_choice_questions_on_live_bedrock() {
    let model = match BedrockModel::new(BedrockModelConfig {
        model_id: Some(MODEL_ID.to_owned()),
        max_tokens: Some(512),
        ..Default::default()
    })
    .await
    {
        Ok(model) => model,
        Err(error) => {
            eprintln!("skipping: no usable AWS credentials ({error})");
            return;
        }
    };

    let decision = LlmDecisionModel::builder()
        .model(model)
        .build()
        .await
        .expect("decision model");

    let result = decision
        .ask(
            "The customer wrote: 'My laptop arrived with a cracked screen and I want my money back.'",
            &Questions::new()
                .boolean("is_complaint", "Is the customer complaining about a product?")
                .choice(
                    "sentiment",
                    "What is the customer's sentiment?",
                    ["positive", "neutral", "negative"],
                ),
        )
        .await
        .expect("live decision succeeds");

    assert_eq!(
        result.answer("is_complaint").and_then(Answer::as_bool),
        Some(true),
        "answers: {:?}",
        result.answers
    );
    assert_eq!(
        result.answer("sentiment").and_then(Answer::as_choice),
        Some("negative"),
        "answers: {:?}",
        result.answers
    );
    assert!(result.usage.is_some(), "Bedrock reports token usage");
    assert!(result.metadata.latency_ms > 0, "Bedrock reports latency");
}
