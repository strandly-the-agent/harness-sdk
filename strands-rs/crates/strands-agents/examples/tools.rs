//! An agent with a `#[tool]`-generated tool.
//!
//! Run with `cargo run --example tools` (needs AWS credentials).

use strands_agents::models::{BedrockModel, BedrockModelConfig};
use strands_agents::{tool, Agent};

/// Current weather for a city.
#[tool]
async fn weather(city: String) -> String {
    format!("It is 22°C and sunny in {city}.")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .tool(WeatherTool::new())
        .build()?;

    let result = agent.invoke("What is the weather in Seattle?").await?;
    println!("{result}");

    Ok(())
}
