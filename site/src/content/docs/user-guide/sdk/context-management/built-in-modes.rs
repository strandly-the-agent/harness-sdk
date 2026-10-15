//! Rust snippets for `context-management/built-in-modes.mdx`.

use strands_agents::Result;

async fn auto() -> Result<()> {
    // --8<-- [start:auto]
    use strands_agents::models::BedrockModel;
    use strands_agents::{Agent, ContextManagerOption};

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .context_manager(ContextManagerOption::Auto)
        .build()?;
    // --8<-- [end:auto]
    let _ = agent;
    Ok(())
}

async fn agentic() -> Result<()> {
    // --8<-- [start:agentic]
    use strands_agents::models::BedrockModel;
    use strands_agents::{Agent, ContextManagerOption};

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .context_manager(ContextManagerOption::Agentic)
        .build()?;
    // --8<-- [end:agentic]
    let _ = agent;
    Ok(())
}

async fn disabled() -> Result<()> {
    // --8<-- [start:disabled]
    use strands_agents::models::BedrockModel;
    use strands_agents::{Agent, ContextManagerOption};

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .context_manager(ContextManagerOption::Off)
        .build()?;
    // --8<-- [end:disabled]
    let _ = agent;
    Ok(())
}
