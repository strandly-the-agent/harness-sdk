//! Streaming an invocation and printing text deltas as they arrive.
//!
//! Run with `cargo run --example streaming` (needs AWS credentials).

use std::io::Write;

use futures::StreamExt;
use strands_agents::models::{BedrockModel, BedrockModelConfig};
use strands_agents::Agent;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::new(model)?;

    let mut stream = agent.stream("Write a haiku about Rust.");
    while let Some(event) = stream.next().await {
        let event = event?;
        if let Some(text) = event.text_delta() {
            print!("{text}");
            std::io::stdout().flush()?;
        }
        if let Some(result) = event.result() {
            println!("\nstop reason: {:?}", result.stop_reason);
        }
    }

    Ok(())
}
