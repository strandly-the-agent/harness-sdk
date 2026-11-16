//! Anthropic: an `AnthropicModel` over the Messages API.
//!
//! The API key comes from `ANTHROPIC_API_KEY` (or `AnthropicModel::builder().api_key(..)`).
//!
//! ```sh
//! cargo run -p strands --example anthropic --features anthropic
//! ```

#![expect(clippy::print_stdout, reason = "examples print their results")]

use strands::models::AnthropicModel;
use strands::{Agent, AgentConfig};

#[tokio::main]
async fn main() -> Result<(), strands::Error> {
    let mut agent = Agent::new(AgentConfig {
        model: Some(
            AnthropicModel::builder()
                .model_id("claude-sonnet-4-6")
                .max_tokens(512)
                .build()?
                .into(),
        ),
        system_prompt: Some("You are a concise assistant.".into()),
        ..Default::default()
    })?;

    let result = agent
        .invoke("Explain what a tool call is in two sentences.")
        .await?;
    println!("\n{:?}", result.stop_reason());

    agent.shutdown().await
}
