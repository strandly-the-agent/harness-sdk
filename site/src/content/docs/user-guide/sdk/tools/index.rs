//! Rust snippets for `tools/index.mdx`.

async fn hub_agent() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:hub_agent]
    use strands_agents::models::BedrockModel;
    use strands_agents::{tool, Agent};

    /// Get the current weather for a city.
    #[tool]
    async fn get_weather(
        /// The name of the city.
        city: String,
    ) -> String {
        format!("It's sunny in {city}.")
    }

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .tool(GetWeatherTool::new())
        .build()?;

    agent.invoke("What's the weather in Seattle?").await?;
    // --8<-- [end:hub_agent]

    Ok(())
}
