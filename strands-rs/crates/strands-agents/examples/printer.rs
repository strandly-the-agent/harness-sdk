//! Printing an invocation with the built-in `AgentPrinter`.
//!
//! Run with `cargo run --example printer` (needs AWS credentials).

use strands_agents::models::{BedrockModel, BedrockModelConfig};
use strands_agents::{Agent, AgentPrinter};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::new(model)?;

    let mut printer = AgentPrinter::new();
    let result = printer.print(agent.stream("Write a haiku about Rust.")).await?;
    println!("stop reason: {:?}", result.stop_reason);

    Ok(())
}
