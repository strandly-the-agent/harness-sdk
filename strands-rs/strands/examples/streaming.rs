//! Streaming: `Agent::stream` yields the agent's events as they happen. This prints text as the
//! model produces it, shows each tool call and its result, and reads usage from the final result.
//!
//! The default model is Amazon Bedrock's; AWS credentials and the region come from the default
//! chain.
//!
//! ```sh
//! cargo run -p strands --example streaming
//! ```

#![expect(clippy::print_stdout, reason = "examples print their results")]

use std::io::Write as _;

use futures::StreamExt;
use strands::{Agent, AgentStreamEvent, ToolError, tool};

/// Get the current weather for a city.
#[tool]
async fn get_weather(
    /// City name, for example "Seattle".
    city: String,
) -> Result<String, ToolError> {
    Ok(format!("Sunny and 21 degrees in {city}"))
}

#[tokio::main]
async fn main() -> Result<(), strands::Error> {
    let mut agent = Agent::builder()
        .system_prompt("You are a concise assistant.")
        .tools([GetWeather])
        .printer(false) // this example prints the events itself
        .build()?;

    // The stream borrows the agent, so it is scoped: it must be dropped before the agent is
    // used again.
    {
        let mut stream = agent.stream("Compare the weather in Seattle and Lisbon.");
        while let Some(event) = stream.next().await {
            match event? {
                AgentStreamEvent::ModelStreamUpdate(update) => {
                    if let Some(text) = update.text_delta() {
                        print!("{text}");
                        std::io::stdout().flush().ok();
                    }
                }
                AgentStreamEvent::BeforeToolCall(call) => {
                    println!(
                        "\n[tool call] {} {}",
                        call.tool_use().name(),
                        call.tool_use().input()
                    );
                }
                AgentStreamEvent::ToolResult(done) => {
                    println!("[tool result] {:?}", done.result().status());
                }
                AgentStreamEvent::AgentResult(result) => {
                    let usage = result.metrics().accumulated_usage();
                    println!(
                        "\n[done] {:?}, {} input and {} output tokens",
                        result.stop_reason(),
                        usage.input_tokens(),
                        usage.output_tokens()
                    );
                }
                _ => {}
            }
        }
    }

    agent.shutdown().await
}
