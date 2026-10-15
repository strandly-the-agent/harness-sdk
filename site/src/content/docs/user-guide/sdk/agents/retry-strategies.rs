//! Snippets for `retry-strategies.mdx`.

async fn default_strategy() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:default_strategy]
    use strands_agents::models::BedrockModel;
    use strands_agents::retry::{
        DefaultModelRetryStrategy, DefaultModelRetryStrategyOptions,
    };
    use strands_agents::Agent;

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .retry_strategy(DefaultModelRetryStrategy::new(
            DefaultModelRetryStrategyOptions { max_attempts: 3, ..Default::default() },
        )?)
        .build()?;
    // --8<-- [end:default_strategy]

    let _ = agent;
    Ok(())
}

async fn custom_backoff() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:custom_backoff]
    use std::sync::Arc;

    use strands_agents::models::BedrockModel;
    use strands_agents::retry::{
        DefaultModelRetryStrategy, DefaultModelRetryStrategyOptions, ExponentialBackoff,
        ExponentialBackoffOptions, JitterKind,
    };
    use strands_agents::Agent;

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .retry_strategy(DefaultModelRetryStrategy::new(
            DefaultModelRetryStrategyOptions {
                max_attempts: 4,
                backoff: Arc::new(ExponentialBackoff::new(ExponentialBackoffOptions {
                    base_ms: 2_000.0,
                    max_ms: 60_000.0,
                    multiplier: 2.0,
                    jitter: JitterKind::Full,
                })),
                ..Default::default()
            },
        )?)
        .build()?;
    // --8<-- [end:custom_backoff]

    let _ = agent;
    Ok(())
}

async fn disable() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:disable]
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .no_retry_strategy()
        .build()?;
    // --8<-- [end:disable]

    let _ = agent;
    Ok(())
}

async fn custom_retryable() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:custom_retryable]
    use std::sync::Arc;

    use strands_agents::models::BedrockModel;
    use strands_agents::retry::{
        is_default_retryable, DefaultModelRetryStrategy, DefaultModelRetryStrategyOptions,
    };
    use strands_agents::{Agent, Error};

    // Retry throttles (the default retryable set) plus service-unavailable errors.
    let retryable = Arc::new(|error: &Error| {
        is_default_retryable(error)
            || matches!(error, Error::Model { message, .. }
                if message.contains("ServiceUnavailable"))
    });

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .retry_strategy(DefaultModelRetryStrategy::new(
            DefaultModelRetryStrategyOptions {
                max_attempts: 5,
                retryable,
                ..Default::default()
            },
        )?)
        .build()?;
    // --8<-- [end:custom_retryable]

    let _ = agent;
    Ok(())
}

async fn hook_retry() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:hook_retry]
    use std::time::Duration;

    use strands_agents::hooks::AfterModelCallEvent;
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    // `attempt_count` is 1-based and owned by the agent loop, so the hook needs
    // no counter of its own.
    async fn retry_failed_model_call(
        event: &mut AfterModelCallEvent,
    ) -> strands_agents::Result<()> {
        if event.error.is_some() && event.attempt_count < 3 {
            tokio::time::sleep(Duration::from_millis(2_000)).await;
            event.retry = true;
        }
        Ok(())
    }

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .hook(retry_failed_model_call)
        .build()?;
    // --8<-- [end:hook_retry]

    let _ = agent;
    Ok(())
}
