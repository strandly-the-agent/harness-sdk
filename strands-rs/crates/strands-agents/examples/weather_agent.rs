//! Agent with one tool declared with `#[tool]`.
//!
//! Run with `cargo run --example weather_agent` (needs AWS credentials).

use strands_agents::models::BedrockModel;
use strands_agents::prelude::*;

/// Current weather for a city.
#[tool]
async fn weather(city: String) -> String {
    format!("It is 22°C and sunny in {city}.")
}

#[tokio::main]
async fn main() -> Result<()> {
    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .system_prompt("You are a terse assistant.")
        .tool(WeatherTool::new())
        .build()?;

    println!(
        "{}",
        agent.invoke("What is the weather in Seattle?").await?.text()
    );
    Ok(())
}
