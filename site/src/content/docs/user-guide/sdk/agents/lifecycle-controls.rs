//! Snippets for `lifecycle-controls.mdx`.

use std::time::Duration;

async fn limits_basic() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:limits_basic]
    use strands_agents::models::BedrockModel;
    use strands_agents::{Agent, InvokeOptions, Limits, StopReason};

    let agent = Agent::new(BedrockModel::default_model().await?)?;

    let limits = Limits::default()
        .with_turns(5)
        .with_output_tokens(2_000)
        .with_total_tokens(10_000);

    let result = agent
        .invoke_with(
            "Summarize this document",
            InvokeOptions::default().with_limits(limits),
        )
        .await?;

    if result.stop_reason == StopReason::LimitTurns {
        println!("Hit turn budget");
    } else if result.stop_reason == StopReason::LimitTotalTokens {
        println!("Hit token budget");
    }
    // --8<-- [end:limits_basic]

    Ok(())
}

async fn cancel_timeout() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:cancel_timeout]
    use std::time::Duration;

    use strands_agents::models::BedrockModel;
    use strands_agents::{Agent, StopReason};

    let agent = Agent::new(BedrockModel::default_model().await?)?;

    // Cancel after 30 seconds. `Agent` is a cheap handle, so clone it into the task.
    let watchdog = agent.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        watchdog.cancel();
    });

    let result = agent.invoke("Analyze this large dataset").await?;

    if result.stop_reason == StopReason::Cancelled {
        println!("Agent was cancelled due to timeout");
    }
    // --8<-- [end:cancel_timeout]

    Ok(())
}

async fn cancel_external() -> Result<(), Box<dyn std::error::Error>> {
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    let agent = Agent::new(BedrockModel::default_model().await?)?;

    // --8<-- [start:cancel_external]
    use strands_agents::{InvokeOptions, StopReason};
    use tokio_util::sync::CancellationToken;

    // Cancel the invocation if it runs past a 20-second deadline
    let cancel = CancellationToken::new();
    let deadline = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(20)).await;
        deadline.cancel();
    });

    let result = agent
        .invoke_with(
            "Analyze this large dataset",
            InvokeOptions::default().with_cancel(cancel),
        )
        .await?;

    if result.stop_reason == StopReason::Cancelled {
        println!("Agent hit its deadline");
    }
    // --8<-- [end:cancel_external]

    Ok(())
}

// --8<-- [start:cancel_in_tool]
use strands_agents::{tool, ToolContext};

/// Fetch a large resource, respecting cancellation.
#[tool]
async fn fetch_resource(ctx: ToolContext, url: String) -> String {
    // Race the slow work against the agent's cancel token
    tokio::select! {
        () = ctx.cancel.cancelled() => "cancelled".to_owned(),
        () = tokio::time::sleep(Duration::from_secs(30)) => format!("fetched {url}"),
    }
}
// --8<-- [end:cancel_in_tool]

async fn concurrent_invocation() -> Result<(), Box<dyn std::error::Error>> {
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    let agent = Agent::new(BedrockModel::default_model().await?)?;

    // --8<-- [start:concurrent_invocation]
    use strands_agents::Error;

    match agent.invoke("Summarize this report").await {
        Ok(result) => println!("{result}"),
        // Another invocation is already running on this agent handle.
        Err(Error::ConcurrentInvocation(message)) => eprintln!("{message}"),
        Err(error) => return Err(error.into()),
    }
    // --8<-- [end:concurrent_invocation]

    Ok(())
}

// --8<-- [start:worked_example]
use strands_agents::models::BedrockModel;
use strands_agents::{Agent, InvokeOptions, Limits, Message, StopReason};
use tokio_util::sync::CancellationToken;

const MODEL_ID: &str = "global.anthropic.claude-sonnet-5";

#[derive(Debug)]
enum Outcome {
    Completed,
    BudgetExceeded,
    TimedOut,
}

fn classify(stop_reason: StopReason) -> Outcome {
    match stop_reason {
        StopReason::Cancelled => Outcome::TimedOut,
        StopReason::LimitTurns
        | StopReason::LimitTotalTokens
        | StopReason::LimitOutputTokens => Outcome::BudgetExceeded,
        _ => Outcome::Completed,
    }
}

async fn handle_request(
    prompt: &str,
    deadline: Duration,
) -> Result<(Outcome, Message), Box<dyn std::error::Error>> {
    // A fresh agent per request keeps concurrent calls from sharing history
    let model = BedrockModel::builder().model_id(MODEL_ID).build().await?;
    let agent = Agent::new(model)?;

    let cancel = CancellationToken::new();
    let timer = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(deadline).await;
        timer.cancel();
    });

    let options = InvokeOptions::default()
        .with_limits(Limits::default().with_turns(6).with_total_tokens(20_000))
        .with_cancel(cancel);

    let result = agent.invoke_with(prompt, options).await?;

    Ok((classify(result.stop_reason), result.last_message))
}
// --8<-- [end:worked_example]
