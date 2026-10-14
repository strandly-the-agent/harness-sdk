//! Agent with one typed tool, built with `FunctionTool::builder`.
//!
//! Run with `cargo run --example weather_agent` (needs AWS credentials).

use serde::Deserialize;
use strands_agents::models::BedrockModel;
use strands_agents::prelude::*;

#[derive(Deserialize, schemars::JsonSchema)]
struct Location {
    /// City to report the weather for.
    city: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let weather = FunctionTool::builder("get_weather")
        .description("Get the current weather for a city")
        .typed_handler(|input: Location, _ctx| async move { Ok(format!("{}: 22°C, sunny", input.city)) });

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .tool(weather)
        .build()
        .await?;

    println!(
        "{}",
        agent.invoke("What is the weather in Seattle?").await?.text()
    );
    Ok(())
}
