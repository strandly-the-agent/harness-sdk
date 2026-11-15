//! The section 13 quickstart: a `#[tool]`, the default model, `invoke` and a scoped `stream`.
//!
//! Without the Bedrock provider in the build the first model call fails with
//! `DefaultNotConfiguredError`; set a model with `.model(..)` to run it.

#![expect(clippy::print_stdout, reason = "an example prints its output")]

use futures::StreamExt;
use strands::{Agent, AgentStreamEvent, tool};

/// Get the current weather for a city.
#[tool]
#[expect(clippy::unused_async, reason = "a real tool would await a weather service")]
async fn get_weather(
    /// City name, for example "Seattle".
    city: String,
) -> Result<String, strands::ToolError> {
    Ok(format!("Sunny in {city}"))
}

#[tokio::main]
async fn main() -> Result<(), strands::Error> {
    let mut agent = Agent::builder() // default model: Bedrock
        .system_prompt("You are a concise assistant.")
        .tools([GetWeather])
        .build()?;

    let result = agent.invoke("What's the weather in Seattle?").await?;
    println!("{result}");

    {
        let mut stream = agent.stream("And in Lisbon?");
        while let Some(event) = stream.next().await {
            if let AgentStreamEvent::ModelStreamUpdate(update) = event?
                && let Some(text) = update.text_delta()
            {
                print!("{text}");
            }
        }
    }
    agent.shutdown().await
}
