//! Rust snippets for `executors.mdx`.

use strands_agents::tool;

/// Get the weather for a location.
#[tool]
async fn weather(location: String) -> String {
    format!("Sunny in {location}")
}

/// Get the current time for a location.
#[tool]
async fn time(location: String) -> String {
    format!("12:00 PM in {location}")
}

/// Take a screenshot.
#[tool]
async fn screenshot() -> String {
    "Screenshot taken".to_owned()
}

/// Send an email.
#[tool]
async fn email(to: String) -> String {
    format!("Email sent to {to}")
}

async fn concurrent_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:concurrent]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::tools::ConcurrentToolExecutor;
    use strands_agents::Agent;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .tool(WeatherTool::new())
        .tool(TimeTool::new())
        .tool_executor(ConcurrentToolExecutor)
        .build()?;
    // Concurrent execution is the default; omit tool_executor for the same result.

    agent.invoke("What is the weather and time in New York?").await?;
    // --8<-- [end:concurrent]
    Ok(())
}

async fn sequential_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:sequential]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::tools::SequentialToolExecutor;
    use strands_agents::Agent;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .tool(ScreenshotTool::new())
        .tool(EmailTool::new())
        .tool_executor(SequentialToolExecutor)
        .build()?;

    agent.invoke("Take a screenshot and email it to my friend").await?;
    // --8<-- [end:sequential]
    Ok(())
}
