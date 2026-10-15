//! Snippets for `agent-loop.mdx`.

use std::time::Duration;

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

async fn cancel_external_signal() -> Result<(), Box<dyn std::error::Error>> {
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    let agent = Agent::new(BedrockModel::default_model().await?)?;

    // --8<-- [start:cancel_external_signal]
    use strands_agents::InvokeOptions;
    use tokio_util::sync::CancellationToken;

    // Call `cancel.cancel()` from anywhere to stop the invocation.
    let cancel = CancellationToken::new();
    let options = InvokeOptions::default().with_cancel(cancel.clone());

    let result = agent.invoke_with("Analyze this large dataset", options).await?;
    println!("{result}");
    // --8<-- [end:cancel_external_signal]

    Ok(())
}

// --8<-- [start:cancel_signal]
use strands_agents::{tool, ToolContext};

/// A task that respects cancellation.
#[tool]
async fn long_running_task(ctx: ToolContext, url: String) -> String {
    // Race the slow work (here, a sleep) against the agent's cancel token.
    tokio::select! {
        () = ctx.cancel.cancelled() => "cancelled".to_owned(),
        () = tokio::time::sleep(Duration::from_secs(30)) => format!("fetched {url}"),
    }
}
// --8<-- [end:cancel_signal]

async fn limits_basic() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:limits_basic]
    use strands_agents::models::BedrockModel;
    use strands_agents::{Agent, InvokeOptions, Limits, StopReason};

    let agent = Agent::new(BedrockModel::default_model().await?)?;

    let limits = Limits::default()
        .with_turns(5)
        .with_output_tokens(2_000)
        .with_total_tokens(10_000);
    let options = InvokeOptions::default().with_limits(limits);

    let result = agent.invoke_with("Summarize this document", options).await?;

    if result.stop_reason == StopReason::LimitTurns {
        println!("Hit turn budget");
    } else if result.stop_reason == StopReason::LimitTotalTokens {
        println!("Hit token budget");
    }
    // --8<-- [end:limits_basic]

    Ok(())
}

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
