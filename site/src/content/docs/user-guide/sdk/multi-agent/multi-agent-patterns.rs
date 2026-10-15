//! Rust snippets for `multi-agent-patterns.mdx`.

async fn agents_as_tools_hub_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:agents_as_tools_hub]
    use std::sync::Arc;

    use strands_agents::agent::AgentAsToolOptions;
    use strands_agents::models::{BedrockModel, BedrockModelConfig, Model};
    use strands_agents::Agent;

    let model: Arc<dyn Model> =
        Arc::new(BedrockModel::new(BedrockModelConfig::default()).await?);

    // A specialized agent, wrapped as a tool for the orchestrator
    let weather_agent = Agent::builder()
        .model(Arc::clone(&model))
        .name("weather_agent")
        .description("Answers questions about the weather.")
        .system_prompt("You are a weather specialist.")
        .build()?;

    // The orchestrator calls the specialist when the query fits
    let orchestrator = Agent::builder()
        .model(model)
        .system_prompt("Route weather questions to weather_agent; answer the rest yourself.")
        .tools([weather_agent.as_tool(AgentAsToolOptions::default())])
        .build()?;

    orchestrator
        .invoke("What should I pack for Seattle this weekend?")
        .await?;
    // --8<-- [end:agents_as_tools_hub]
    Ok(())
}
