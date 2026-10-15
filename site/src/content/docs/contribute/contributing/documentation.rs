//! Rust snippets for `contribute/contributing/documentation.mdx`.

async fn basic_agent() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:basic_agent]
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    // Good: Start simple
    let agent = Agent::new(BedrockModel::default_model().await?)?;
    agent.invoke("Hello, world!").await?;

    // Then show configuration
    let configured = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .system_prompt("You are a helpful assistant.")
        .build()?;
    configured.invoke("What's the weather like?").await?;
    // --8<-- [end:basic_agent]

    Ok(())
}
