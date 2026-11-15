//! The headline example (PROMPT.md section 13): a `#[tool]`, the default model, `invoke`, a
//! scoped `stream` and `shutdown`. It needs a configured default model to run.

#![expect(clippy::print_stdout, reason = "examples print their results")]

use futures::StreamExt;
use strands::{Agent, AgentStreamEvent, tool};

/// Get the current weather for a city.
#[tool]
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
