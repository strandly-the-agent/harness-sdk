//! The harness agent in one call.

use strands_harness::{create_harness, HarnessAgentOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let agent = create_harness(HarnessAgentOptions::default()).await?;
    let result = agent.invoke("What files are in the current directory?").await?;
    println!("{}", result.text());
    Ok(())
}
