//! `GoalLoop` through the agent loop (`strands-ts/src/vended-plugins/goal/goal.test.ts`).

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::json;
use strands_agents::agent::InvokeInput;
use strands_agents::error::{Error, Result};
use strands_agents::plugins::vended::{
    GoalLoop, GoalResult, GoalStopReason, JudgeConfig, ValidationContext, ValidationOutcome,
};
use strands_agents::test_support::{MockMessageModel, MockResponse};
use strands_agents::types::{ContentBlock, Role, StopReason, ToolUseBlock};
use strands_agents::{Agent, Message};

fn replies(texts: &[&str]) -> Arc<MockMessageModel> {
    MockMessageModel::shared(
        texts
            .iter()
            .map(|text| MockResponse::message(Message::assistant(*text)))
            .collect(),
    )
}

/// The judge replies the only way it is allowed to: through the structured-output tool.
fn verdict(passed: bool, feedback: Option<&str>) -> MockResponse {
    let input = match feedback {
        Some(feedback) => json!({ "passed": passed, "feedback": feedback }),
        None => json!({ "passed": passed }),
    };
    MockResponse::with_stop_reason(
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse(ToolUseBlock::new(
                "strands_structured_output",
                "verdict",
                input,
            ))],
        ),
        StopReason::ToolUse,
    )
}

/// Records every response the validator sees into `sink`, passing once the text contains `pass_on`.
fn recording_validator(
    pass_on: &'static str,
    sink: Arc<Mutex<Vec<String>>>,
) -> impl Fn(&ValidationContext) -> Result<ValidationOutcome> {
    move |ctx: &ValidationContext| {
        let text = ctx.response.text();
        sink.lock().unwrap().push(text.clone());
        Ok(if text.contains(pass_on) {
            ValidationOutcome::passed()
        } else {
            ValidationOutcome::failed(format!("missing {pass_on}"))
        })
    }
}

/// Overruns the budget on attempt 1 only (two messages in the transcript), then fails fast.
fn slow_failing_validator(ctx: &ValidationContext) -> BoxFuture<'_, Result<ValidationOutcome>> {
    let first_attempt = ctx.agent.messages().len() <= 2;
    Box::pin(async move {
        if first_attempt {
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        Ok(ValidationOutcome::failed("try again"))
    })
}

/// Sleeps before deciding, so the run only ends correctly if the plugin awaits the validator.
fn async_validator(ctx: &ValidationContext) -> BoxFuture<'_, Result<ValidationOutcome>> {
    let passed = ctx.response.text().contains("good");
    Box::pin(async move {
        tokio::time::sleep(Duration::from_millis(5)).await;
        Ok(if passed {
            ValidationOutcome::passed()
        } else {
            ValidationOutcome::failed("try again")
        })
    })
}

/// Fields of every attempt, for exact-vec assertions (the result types are `#[non_exhaustive]`).
fn attempt_tuples(run: &GoalResult) -> Vec<(usize, bool, Option<&str>)> {
    run.attempts
        .iter()
        .map(|attempt| (attempt.attempt, attempt.passed, attempt.feedback.as_deref()))
        .collect()
}

// ---- function validator ---------------------------------------------------

#[tokio::test]
async fn stops_after_one_attempt_when_the_validator_passes() {
    let model = replies(&["done", "unused"]);
    let goal = GoalLoop::sync_validator(|_| Ok(ValidationOutcome::passed()));
    let agent = Agent::builder()
        .model(model.clone())
        .plugin(goal.clone())
        .build()
        .unwrap();

    let result = agent.invoke("go").await.unwrap();

    assert_eq!(result.text(), "done");
    assert_eq!(model.call_count(), 1);
    let run = goal.last_result(&agent).unwrap();
    assert!(run.passed);
    assert_eq!(run.stop_reason, GoalStopReason::Satisfied);
    assert_eq!(run.attempts.len(), 1);
    assert_eq!(run.attempts[0].feedback, None);
}

#[tokio::test]
async fn resumes_with_the_feedback_and_counts_the_attempts() {
    let model = replies(&["nope", "all good"]);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let goal = GoalLoop::sync_validator(recording_validator("good", seen.clone()));
    let agent = Agent::builder()
        .model(model.clone())
        .plugin(goal.clone())
        .build()
        .unwrap();

    agent.invoke("go").await.unwrap();

    assert_eq!(model.call_count(), 2);
    assert_eq!(
        &*seen.lock().unwrap(),
        &["nope".to_owned(), "all good".to_owned()]
    );

    let run = goal.last_result(&agent).unwrap();
    assert!(run.passed);
    assert_eq!(run.stop_reason, GoalStopReason::Satisfied);
    assert_eq!(run.attempts.len(), 2);
    assert_eq!(run.attempts[0].attempt, 1);
    assert!(!run.attempts[0].passed);
    assert_eq!(run.attempts[0].feedback.as_deref(), Some("missing good"));
    assert!(run.attempts[1].passed);

    // The retry is a user message carrying the feedback, and attempt 1 stays in history.
    let messages = agent.messages();
    assert_eq!(
        messages.iter().filter(|m| m.role == Role::User).count(),
        2,
        "{messages:?}"
    );
    let retry = messages
        .iter()
        .find(|m| m.role == Role::User && m.text().contains("Feedback"))
        .unwrap();
    assert!(retry.text().contains("missing good"));
    assert!(messages.iter().any(|m| m.text() == "nope"));
}

#[tokio::test]
async fn stops_at_max_attempts_without_passing() {
    let model = replies(&["bad"]);
    let goal = GoalLoop::sync_validator(|_| Ok(ValidationOutcome::failed("try harder"))).with_max_attempts(3);
    let agent = Agent::builder()
        .model(model.clone())
        .plugin(goal.clone())
        .build()
        .unwrap();

    agent.invoke("go").await.unwrap();

    assert_eq!(model.call_count(), 3);
    let run = goal.last_result(&agent).unwrap();
    assert!(!run.passed);
    assert_eq!(run.stop_reason, GoalStopReason::MaxAttempts);
    assert_eq!(run.attempts.len(), 3);
}

#[tokio::test]
async fn a_validator_error_becomes_failing_feedback() {
    let model = replies(&["bad"]);
    let goal = GoalLoop::sync_validator(|_| Err(Error::other("validator exploded"))).with_max_attempts(1);
    let agent = Agent::builder()
        .model(model)
        .plugin(goal.clone())
        .build()
        .unwrap();

    agent.invoke("go").await.unwrap();

    let run = goal.last_result(&agent).unwrap();
    assert!(!run.passed);
    assert_eq!(run.stop_reason, GoalStopReason::MaxAttempts);
    assert!(run.attempts[0]
        .feedback
        .as_deref()
        .unwrap()
        .contains("validator exploded"));
}

#[tokio::test]
async fn an_async_validator_stops_the_run_on_the_wall_clock_budget() {
    let model = replies(&["bad", "still bad"]);
    // Attempt 1 alone overruns the budget, so attempt 2 is invoked but never validated.
    let goal = GoalLoop::validator(slow_failing_validator).with_timeout(Duration::from_millis(10));
    let agent = Agent::builder()
        .model(model.clone())
        .plugin(goal.clone())
        .build()
        .unwrap();

    agent.invoke("go").await.unwrap();

    let run = goal.last_result(&agent).unwrap();
    assert!(!run.passed);
    assert_eq!(run.stop_reason, GoalStopReason::Timeout);
    assert_eq!(attempt_tuples(&run), vec![(1, false, Some("try again"))]);
    assert_eq!(model.call_count(), 2);
}

#[tokio::test]
async fn awaits_an_async_validator_before_deciding() {
    let model = replies(&["bad", "good"]);
    let goal = GoalLoop::validator(async_validator);
    let agent = Agent::builder()
        .model(model.clone())
        .plugin(goal.clone())
        .build()
        .unwrap();

    agent.invoke("go").await.unwrap();

    let run = goal.last_result(&agent).unwrap();
    assert!(run.passed);
    assert_eq!(run.stop_reason, GoalStopReason::Satisfied);
    assert_eq!(
        attempt_tuples(&run),
        vec![(1, false, Some("try again")), (2, true, None)]
    );
    assert_eq!(model.call_count(), 2);
}

#[tokio::test]
async fn the_resume_prompt_can_be_overridden() {
    let model = replies(&["bad", "good"]);
    let goal = GoalLoop::sync_validator(|ctx: &ValidationContext| {
        Ok(ValidationOutcome::from(ctx.response.text().contains("good")))
    })
    .with_resume_prompt_template(|feedback| {
        InvokeInput::Text(format!("retry now ({})", feedback.unwrap_or("no feedback")))
    });
    let agent = Agent::builder()
        .model(model)
        .plugin(goal.clone())
        .build()
        .unwrap();

    agent.invoke("go").await.unwrap();

    let messages = agent.messages();
    assert!(messages
        .iter()
        .any(|m| m.role == Role::User && m.text() == "retry now (no feedback)"));
    assert!(goal.last_result(&agent).unwrap().passed);
}

// ---- last_result lifecycle ------------------------------------------------

#[tokio::test]
async fn last_result_is_none_before_any_run_and_per_agent() {
    let goal = GoalLoop::sync_validator(|_| Ok(ValidationOutcome::passed()));
    let attached = Agent::builder()
        .model(replies(&["done"]))
        .plugin(goal.clone())
        .build()
        .unwrap();
    let other = Agent::builder().model(replies(&["done"])).build().unwrap();

    assert!(goal.last_result(&attached).is_none());
    assert!(goal.last_result(&other).is_none());

    attached.invoke("go").await.unwrap();

    assert!(goal.last_result(&attached).is_some());
    assert!(goal.last_result(&other).is_none());
}

#[tokio::test]
async fn a_second_invocation_replaces_the_previous_result() {
    let model = MockMessageModel::shared(vec![
        MockResponse::message(Message::assistant("bad")),
        MockResponse::message(Message::assistant("good")),
        MockResponse::message(Message::assistant("good")),
    ]);
    let goal = GoalLoop::sync_validator(|ctx: &ValidationContext| {
        Ok(ValidationOutcome::from(ctx.response.text() == "good"))
    });
    let agent = Agent::builder()
        .model(model)
        .plugin(goal.clone())
        .build()
        .unwrap();

    agent.invoke("first").await.unwrap();
    assert_eq!(goal.last_result(&agent).unwrap().attempts.len(), 2);

    agent.invoke("second").await.unwrap();
    let run = goal.last_result(&agent).unwrap();
    assert_eq!(run.attempts.len(), 1, "a new run starts from scratch");
    assert!(run.passed);
}

#[tokio::test]
async fn only_one_goal_loop_can_attach_to_an_agent() {
    let agent = Agent::builder()
        .model(replies(&["done"]))
        .plugin(GoalLoop::sync_validator(|_| Ok(ValidationOutcome::passed())))
        .plugin(GoalLoop::sync_validator(|_| Ok(ValidationOutcome::passed())).with_name("second"))
        .build()
        .unwrap();
    let error = agent.initialize().await.unwrap_err();

    assert!(error.to_string().contains("only one GoalLoop"), "{error}");
}

#[tokio::test]
async fn max_attempts_below_one_is_rejected_at_attach_time() {
    let agent = Agent::builder()
        .model(replies(&["done"]))
        .plugin(GoalLoop::sync_validator(|_| Ok(ValidationOutcome::passed())).with_max_attempts(0))
        .build()
        .unwrap();
    let error = agent.initialize().await.unwrap_err();

    assert!(error.to_string().contains("at least 1"), "{error}");
}

// ---- preserve_context -----------------------------------------------------

#[tokio::test]
async fn preserve_context_true_keeps_the_earlier_attempts_in_the_prompt() {
    let model = replies(&["bad", "good"]);
    let goal = GoalLoop::sync_validator(|ctx: &ValidationContext| {
        Ok(ValidationOutcome::from(ctx.response.text() == "good"))
    });
    let agent = Agent::builder()
        .model(model.clone())
        .plugin(goal)
        .build()
        .unwrap();

    agent.invoke("go").await.unwrap();

    let second_call = &model.calls()[1].0;
    assert!(second_call.iter().any(|m| m.text() == "bad"));
}

#[tokio::test]
async fn preserve_context_false_hides_the_earlier_attempts_from_the_model() {
    let model = replies(&["bad", "good"]);
    let goal = GoalLoop::sync_validator(|ctx: &ValidationContext| {
        Ok(ValidationOutcome::from(ctx.response.text() == "good"))
    })
    .with_preserve_context(false);
    let agent = Agent::builder()
        .model(model.clone())
        .plugin(goal.clone())
        .build()
        .unwrap();

    agent.invoke("go").await.unwrap();

    let calls = model.calls();
    assert_eq!(calls.len(), 2);
    let second_call = &calls[1].0;
    assert!(second_call.iter().all(|m| m.text() != "bad"), "{second_call:?}");
    assert!(second_call.iter().any(|m| m.text() == "go"));
    assert!(second_call.iter().any(|m| m.text().contains("did not satisfy")));
    assert!(goal.last_result(&agent).unwrap().passed);
}

// ---- natural-language judge ----------------------------------------------

#[tokio::test]
async fn the_judge_drives_the_loop_on_the_aux_model() {
    let model = replies(&["first draft", "second draft"]);
    let judge = MockMessageModel::shared(vec![verdict(false, Some("too long")), verdict(true, None)]);
    let goal = GoalLoop::goal("Answer in one word.");
    let agent = Agent::builder()
        .model(model.clone())
        .aux_model(judge.clone())
        .plugin(goal.clone())
        .build()
        .unwrap();

    agent.invoke("go").await.unwrap();

    assert_eq!(model.call_count(), 2);
    assert_eq!(judge.call_count(), 2, "one fresh judge per validation");
    let run = goal.last_result(&agent).unwrap();
    assert!(run.passed);
    assert_eq!(run.stop_reason, GoalStopReason::Satisfied);
    assert_eq!(run.attempts.len(), 2);
    assert_eq!(run.attempts[0].feedback.as_deref(), Some("too long"));

    // The judge's feedback is what the agent is told to fix.
    assert!(agent
        .messages()
        .iter()
        .any(|m| m.role == Role::User && m.text().contains("too long")));
    // Each judge call sees the host transcript rendered into its prompt.
    assert!(judge.calls()[0].0[0].text().contains("Answer in one word."));
}

#[tokio::test]
async fn judge_model_overrides_the_aux_model() {
    let model = replies(&["draft"]);
    let aux = MockMessageModel::shared(vec![verdict(false, Some("unused"))]);
    let judge = MockMessageModel::shared(vec![verdict(true, None)]);
    let goal =
        GoalLoop::goal("Answer in one word.").with_judge(JudgeConfig::default().with_model(judge.clone()));
    let agent = Agent::builder()
        .model(model)
        .aux_model(aux.clone())
        .plugin(goal.clone())
        .build()
        .unwrap();

    agent.invoke("go").await.unwrap();

    assert_eq!(aux.call_count(), 0);
    assert_eq!(judge.call_count(), 1);
    assert!(goal.last_result(&agent).unwrap().passed);
}

#[tokio::test]
async fn the_judge_system_prompt_can_be_overridden() {
    let judge = MockMessageModel::shared(vec![verdict(true, None)]);
    let goal = GoalLoop::goal("Answer in one word.").with_judge(
        JudgeConfig::default()
            .with_model(judge.clone())
            .with_system_prompt("only say yes"),
    );
    let agent = Agent::builder()
        .model(replies(&["draft"]))
        .plugin(goal)
        .build()
        .unwrap();

    agent.invoke("go").await.unwrap();

    assert_eq!(
        judge.calls()[0].1.system_prompt.as_ref().unwrap().text(),
        "only say yes"
    );
}

#[tokio::test]
async fn the_judge_falls_back_to_the_host_model() {
    let model = MockMessageModel::shared(vec![
        MockResponse::message(Message::assistant("draft")),
        verdict(true, None),
    ]);
    let goal = GoalLoop::goal("Answer in one word.");
    let agent = Agent::builder()
        .model(model.clone())
        .plugin(goal.clone())
        .build()
        .unwrap();

    agent.invoke("go").await.unwrap();

    assert_eq!(model.call_count(), 2, "one agent call, one judge call");
    assert!(goal.last_result(&agent).unwrap().passed);
}

#[tokio::test]
async fn a_judge_that_replies_in_prose_is_retried_for_structured_output() {
    let judge = MockMessageModel::shared(vec![
        MockResponse::message(Message::assistant("looks fine to me")),
        verdict(true, None),
    ]);
    let goal =
        GoalLoop::goal("Answer in one word.").with_judge(JudgeConfig::default().with_model(judge.clone()));
    let agent = Agent::builder()
        .model(replies(&["draft"]))
        .plugin(goal.clone())
        .build()
        .unwrap();

    agent.invoke("go").await.unwrap();

    assert_eq!(judge.call_count(), 2);
    assert!(goal.last_result(&agent).unwrap().passed);
}

// ---- run state across agents and failed invocations ----------------------

#[tokio::test]
async fn one_plugin_instance_serves_several_agents_with_independent_run_state() {
    let goal = GoalLoop::sync_validator(|ctx: &ValidationContext| {
        Ok(ValidationOutcome::from(ctx.response.text() == "good"))
    })
    .with_max_attempts(2);
    let first = Agent::builder()
        .model(replies(&["good"]))
        .plugin(goal.clone())
        .build()
        .unwrap();
    let second = Agent::builder()
        .model(replies(&["bad"]))
        .plugin(goal.clone())
        .build()
        .unwrap();

    first.invoke("go").await.unwrap();
    second.invoke("go").await.unwrap();

    let first_run = goal.last_result(&first).unwrap();
    let second_run = goal.last_result(&second).unwrap();
    assert!(first_run.passed);
    assert_eq!(first_run.attempts.len(), 1);
    assert!(!second_run.passed);
    assert_eq!(second_run.stop_reason, GoalStopReason::MaxAttempts);
    assert_eq!(second_run.attempts.len(), 2);
}

#[tokio::test]
async fn a_failed_invocation_leaves_no_result_and_does_not_leak_into_the_next_run() {
    let model = MockMessageModel::shared(vec![
        MockResponse::Error(Error::other("model down")),
        MockResponse::message(Message::assistant("good")),
    ]);
    let goal = GoalLoop::sync_validator(|ctx: &ValidationContext| {
        Ok(ValidationOutcome::from(ctx.response.text() == "good"))
    });
    let agent = Agent::builder()
        .model(model)
        .plugin(goal.clone())
        .build()
        .unwrap();

    agent.invoke("go").await.unwrap_err();
    assert!(
        goal.last_result(&agent).is_none(),
        "no result for a run that threw"
    );

    agent.invoke("again").await.unwrap();
    let run = goal.last_result(&agent).unwrap();
    assert!(run.passed);
    assert_eq!(run.attempts.len(), 1);
}

// ---- mid-run observability ------------------------------------------------

#[tokio::test]
async fn last_result_is_none_while_the_run_is_in_flight() {
    // The validator is the only hook that runs between attempts, so it is where we probe.
    let holder: Arc<OnceLock<GoalLoop>> = Arc::new(OnceLock::new());
    let probe = holder.clone();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let goal = GoalLoop::sync_validator(move |ctx: &ValidationContext| {
        let plugin = probe.get().expect("plugin registered before the run");
        sink.lock()
            .unwrap()
            .push(plugin.last_result(&ctx.agent).is_some());
        Ok(ValidationOutcome::from(ctx.response.text() == "good"))
    });
    let _ = holder.set(goal.clone());
    let agent = Agent::builder()
        .model(replies(&["bad", "good"]))
        .plugin(goal.clone())
        .build()
        .unwrap();

    agent.invoke("go").await.unwrap();

    assert_eq!(&*seen.lock().unwrap(), &[false, false]);
    assert!(goal.last_result(&agent).is_some());
}

#[tokio::test]
async fn preserve_context_false_does_not_roll_back_the_agent_state() {
    let goal = GoalLoop::sync_validator(|ctx: &ValidationContext| {
        let runs = ctx
            .agent
            .state()
            .get("runs")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        ctx.agent.state().set("runs", json!(runs + 1));
        Ok(ValidationOutcome::from(ctx.response.text() == "good"))
    })
    .with_preserve_context(false);
    let agent = Agent::builder()
        .model(replies(&["bad", "good"]))
        .plugin(goal)
        .build()
        .unwrap();

    agent.invoke("go").await.unwrap();

    // Plugin scratch space is invisible to the model, so it survives the rewind (TS parity).
    assert_eq!(agent.state().get("runs"), Some(json!(2)));
}
