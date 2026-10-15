//! Live Bedrock decision test. It calls the real service and is `#[ignore]`d; run with:
//!
//! ```text
//! AWS_REGION=us-west-2 cargo test --all-features -p strands-agents --test integ_decisions -- --ignored
//! ```
//!
//! Usable AWS credentials are required: `BedrockModel::new` only builds a client and never
//! resolves them, so a run without credentials fails at the call rather than skipping (same as
//! `integ_bedrock.rs`). Throttling is skipped, not failed.
//!
//! Observed flake (provider-side, ~2 in 30 runs): the model answers with XML-wrapped tool input
//! (`<parameter name="answer">true`), which fails as an invalid answer set. Re-run.

#![cfg(feature = "bedrock")]

use strands_agents::decisions::{
    Answer, DecisionModelExt, LlmDecisionModel, Question, Questions, Uncertainty,
};
use strands_agents::models::{BedrockModel, BedrockModelConfig};
use strands_agents::Error;

const MODEL_ID: &str = "us.anthropic.claude-haiku-4-5-20251001-v1:0";

#[tokio::test]
#[ignore = "live Bedrock"]
async fn answers_boolean_and_choice_questions_on_live_bedrock() {
    let model = BedrockModel::new(BedrockModelConfig {
        model_id: Some(MODEL_ID.to_owned()),
        max_tokens: Some(512),
        ..Default::default()
    })
    .await
    .expect("bedrock client");

    let decision = LlmDecisionModel::builder()
        .model(model)
        .build()
        .await
        .expect("decision model");

    // Uncertainty is disabled on both questions: `uncertain` is otherwise a legal answer, and
    // `as_bool`/`as_choice` return `None` for it, which would make the assertions below flaky.
    let questions = Questions::new()
        .with(
            "is_complaint",
            Question::boolean("Is the customer complaining about a product?")
                .with_uncertainty(Uncertainty::Disabled),
        )
        .with(
            "sentiment",
            Question::choice(
                "What is the customer's sentiment?",
                ["positive", "neutral", "negative"],
            )
            .with_uncertainty(Uncertainty::Disabled),
        );

    let result = match decision
        .ask(
            "The customer wrote: 'My laptop arrived with a cracked screen and I want my money back.'",
            &questions,
        )
        .await
    {
        Ok(result) => result,
        Err(error @ Error::ModelThrottled { .. }) => {
            eprintln!("skipping: Bedrock throttled the request ({error})");
            return;
        }
        Err(error) => panic!("live decision failed: {error}"),
    };

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
