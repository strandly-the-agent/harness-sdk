//! Minimal agent on the default Bedrock model.
//!
//! Run with `cargo run --example basic` (needs AWS credentials).

use std::sync::Arc;

use strands_agents::models::{BedrockModel, BedrockModelConfig};
use strands_agents::Agent;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::new(Arc::new(model)).await?;

    let result = agent
        .invoke("In one sentence: what is a model-driven agent?")
        .await?;
    println!("{result}");

    Ok(())
}
