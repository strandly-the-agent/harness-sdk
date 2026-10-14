//! An agent with a typed `FunctionTool`.
//!
//! Run with `cargo run --example tools` (needs AWS credentials).

use schemars::JsonSchema;
use serde::Deserialize;
use strands_agents::models::{BedrockModel, BedrockModelConfig};
use strands_agents::tools::{FunctionTool, ToolContext};
use strands_agents::Agent;

#[derive(Deserialize, JsonSchema)]
struct WeatherInput {
    /// City to look up.
    city: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let weather = FunctionTool::builder("weather")
        .description("Current weather for a city")
        .typed_handler(|input: WeatherInput, _ctx: ToolContext| async move {
            Ok(format!("It is 22°C and sunny in {}.", input.city))
        });

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).tool(weather).build().await?;

    let result = agent.invoke("What is the weather in Seattle?").await?;
    println!("{result}");

    Ok(())
}
