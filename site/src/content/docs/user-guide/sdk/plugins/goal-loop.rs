//! Rust snippets for `goal-loop.mdx`.

// =====================
// Getting Started
// =====================

async fn getting_started() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:getting_started]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::plugins::GoalLoop;
    use strands_agents::Agent;

    let concise =
        GoalLoop::goal("At most 3 sentences, accessible to a 10-year-old, no jargon.")
            .with_max_attempts(3);

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .plugin(concise.clone())
        .build()?;
    agent.invoke("Explain how rainbows form.").await?;
    println!("{:?}", concise.last_result(&agent));

    // Typical output:
    // Some(GoalResult { passed: true, stop_reason: Satisfied, attempts: [..] })
    // --8<-- [end:getting_started]

    Ok(())
}

// =====================
// Programmatic validators
// =====================

fn word_count_validator_example() {
    // --8<-- [start:word_count_validator]
    use std::time::Duration;

    use strands_agents::plugins::{GoalLoop, ValidationContext, ValidationOutcome};
    use strands_agents::Result;

    fn word_count_validator(context: &ValidationContext) -> Result<ValidationOutcome> {
        let words = context.response.text().split_whitespace().count();
        if words <= 50 {
            return Ok(ValidationOutcome::passed());
        }
        Ok(ValidationOutcome::failed(format!(
            "Too long ({words} words). Cap at 50."
        )))
    }

    let plugin = GoalLoop::sync_validator(word_count_validator)
        .with_max_attempts(5)
        .with_timeout(Duration::from_secs(30));
    // --8<-- [end:word_count_validator]

    let _ = plugin;
}

fn async_validator_example() {
    // --8<-- [start:async_validator]
    use strands_agents::plugins::{GoalLoop, ValidationOutcome};
    use strands_agents::Error;
    use tokio::process::Command;

    let plugin = GoalLoop::validator(|_context| {
        Box::pin(async {
            let output = Command::new("npm")
                .arg("test")
                .output()
                .await
                .map_err(|error| Error::other(error.to_string()))?;
            if output.status.success() {
                return Ok(ValidationOutcome::passed());
            }
            let mut out = String::from_utf8_lossy(&output.stdout).into_owned();
            out.push_str(&String::from_utf8_lossy(&output.stderr));
            let skip = out.chars().count().saturating_sub(4000);
            let tail: String = out.chars().skip(skip).collect();
            Ok(ValidationOutcome::failed(format!("Tests failed.\n{tail}")))
        })
    })
    .with_max_attempts(10);
    // --8<-- [end:async_validator]

    let _ = plugin;
}

// =====================
// Inspecting results
// =====================

fn inspecting_results(
    plugin: &strands_agents::plugins::GoalLoop,
    agent: &strands_agents::Agent,
) {
    // --8<-- [start:inspecting_results]
    if let Some(result) = plugin.last_result(agent) {
        if !result.passed {
            println!("Stopped after {} attempts", result.attempts.len());
            println!("Reason: {:?}", result.stop_reason);
            for attempt in &result.attempts {
                println!("  #{}: {:?}", attempt.attempt, attempt.feedback);
            }
        }
    }
    // --8<-- [end:inspecting_results]
}

// =====================
// Stateless retries
// =====================

fn preserve_context_false() {
    use strands_agents::plugins::ValidationOutcome;

    // --8<-- [start:preserve_context_false]
    use strands_agents::plugins::GoalLoop;

    let plugin = GoalLoop::sync_validator(|_context| Ok(ValidationOutcome::passed()))
        .with_max_attempts(10)
        .with_preserve_context(false);
    // --8<-- [end:preserve_context_false]

    let _ = plugin;
}

// =====================
// Custom judge configuration
// =====================

async fn judge_config() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:judge_config]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::plugins::vended::goal::JudgeConfig;
    use strands_agents::plugins::GoalLoop;

    let judge_model =
        BedrockModel::new(BedrockModelConfig::new("us.amazon.nova-lite-v1:0")).await?;

    let plugin = GoalLoop::goal("Response must cite at least two sources.")
        .with_max_attempts(3)
        .with_judge(JudgeConfig::default().with_model(judge_model));
    // --8<-- [end:judge_config]

    let _ = plugin;
    Ok(())
}

// =====================
// Custom resume prompt
// =====================

fn custom_resume_prompt() {
    // --8<-- [start:custom_resume_prompt]
    use strands_agents::plugins::GoalLoop;
    use strands_agents::InvokeInput;

    let plugin = GoalLoop::goal("...")
        .with_max_attempts(3)
        .with_resume_prompt_template(|feedback| match feedback {
            None => InvokeInput::from(
                "That didn't pass. Start over from scratch with a different approach.",
            ),
            Some(feedback) => InvokeInput::from(format!(
                "Validation failed:\n{feedback}\n\n\
                 Do NOT edit your previous response. Start over \
                 from scratch and take a completely different approach."
            )),
        });
    // --8<-- [end:custom_resume_prompt]

    let _ = plugin;
}

// =====================
// Building a custom judge
// =====================

fn custom_judge() {
    // --8<-- [start:custom_judge]
    use strands_agents::plugins::vended::goal::{
        build_judge_prompt, JudgeOutcome, JUDGE_SYSTEM_PROMPT,
    };
    use strands_agents::plugins::{GoalLoop, ValidationOutcome};
    use strands_agents::Agent;

    let plugin = GoalLoop::validator(|context| {
        Box::pin(async move {
            let judge = Agent::builder()
                .model(context.agent.model())
                .system_prompt(JUDGE_SYSTEM_PROMPT)
                .build()?;

            let prompt = build_judge_prompt("Be concise.", &context.agent.messages());
            let outcome: JudgeOutcome = judge.structured_output(prompt).await?;

            Ok(ValidationOutcome {
                passed: outcome.passed,
                feedback: outcome.feedback,
            })
        })
    })
    .with_max_attempts(3);
    // --8<-- [end:custom_judge]

    let _ = plugin;
}
