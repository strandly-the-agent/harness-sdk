//! Live Bedrock goal-loop tests (`strands-ts/src/vended-plugins/goal/goal.test.ts` exercised
//! against a real model). They call the real service and are `#[ignore]`d; run with:
//!
//! ```text
//! AWS_REGION=us-west-2 cargo test -p strands-agents --all-features --test integ_goal -- --ignored
//! ```

#![cfg(feature = "bedrock")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use strands_agents::models::{BedrockModel, BedrockModelConfig};
use strands_agents::plugins::vended::{GoalLoop, GoalStopReason, JudgeConfig, ValidationOutcome};
use strands_agents::Agent;

const MODEL_ID: &str = "us.anthropic.claude-haiku-4-5-20251001-v1:0";

async fn model(max_tokens: i32) -> BedrockModel {
    BedrockModel::new(BedrockModelConfig {
        model_id: Some(MODEL_ID.to_owned()),
        max_tokens: Some(max_tokens),
        ..Default::default()
    })
    .await
    .expect("bedrock client")
}

/// Natural-language goal graded by a real judge agent on a verdict nothing should get wrong.
#[tokio::test]
#[ignore = "live Bedrock"]
async fn judge_satisfies_a_trivial_goal() {
    let goal = GoalLoop::goal(
        "The assistant's final reply is exactly the single word PINEAPPLE, with nothing else.",
    )
    .with_max_attempts(3)
    .with_judge(JudgeConfig::default().with_model(model(200).await));
    let agent = Agent::builder()
        .model(model(100).await)
        .plugin(goal.clone())
        .build()
        .unwrap();

    let result = agent
        .invoke("Reply with exactly the word PINEAPPLE and nothing else.")
        .await
        .unwrap();
    println!("reply: {:?}", result.text());

    let run = goal.last_result(&agent).expect("a finished goal run");
    println!("run: {run:?}");
    assert!(run.passed, "judge should have passed the reply");
    assert_eq!(run.stop_reason, GoalStopReason::Satisfied);
    assert!(!run.attempts.is_empty());
    assert!(run.attempts.last().unwrap().passed);
}

/// A validator that rejects attempt 1 forces exactly one real retry with the feedback.
#[tokio::test]
#[ignore = "live Bedrock"]
async fn validator_rejects_the_first_attempt_and_passes_the_second() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let goal = GoalLoop::sync_validator(move |_ctx| {
        Ok(if seen.fetch_add(1, Ordering::SeqCst) == 0 {
            ValidationOutcome::failed("Not accepted. Reply with exactly the word PINEAPPLE.")
        } else {
            ValidationOutcome::passed()
        })
    })
    .with_max_attempts(3);
    let agent = Agent::builder()
        .model(model(100).await)
        .plugin(goal.clone())
        .build()
        .unwrap();

    let result = agent
        .invoke("Reply with exactly the word PINEAPPLE and nothing else.")
        .await
        .unwrap();
    println!("reply: {:?}", result.text());

    let run = goal.last_result(&agent).expect("a finished goal run");
    println!("run: {run:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 2, "validator ran twice");
    assert!(run.passed);
    assert_eq!(run.stop_reason, GoalStopReason::Satisfied);
    assert_eq!(run.attempts.len(), 2);
    assert!(!run.attempts[0].passed);
    assert!(run.attempts[1].passed);
    // The retry is a user message carrying the validator's feedback.
    assert!(agent.messages().iter().any(|m| m.text().contains("Not accepted")));
}
