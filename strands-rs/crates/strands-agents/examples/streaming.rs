//! Streaming an invocation and printing text deltas as they arrive.
//!
//! Run with `cargo run --example streaming` (needs AWS credentials).

use std::io::Write;
use std::sync::Arc;

use futures::StreamExt;
use strands_agents::models::{BedrockModel, BedrockModelConfig, ContentBlockDelta, ModelStreamEvent};
use strands_agents::{Agent, AgentStreamEvent, InvokeOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::new(Arc::new(model)).await?;

    let mut stream = agent.stream("Write a haiku about Rust.", InvokeOptions::default());
    while let Some(event) = stream.next().await {
        match event? {
            AgentStreamEvent::ModelStreamUpdate(update) => {
                if let ModelStreamEvent::ContentBlockDelta {
                    delta: ContentBlockDelta::Text { text },
                } = update.event
                {
                    print!("{text}");
                    std::io::stdout().flush()?;
                }
            }
            AgentStreamEvent::AgentResult(event) => {
                println!("\nstop reason: {:?}", event.result.stop_reason);
            }
            _ => {}
        }
    }

    Ok(())
}
