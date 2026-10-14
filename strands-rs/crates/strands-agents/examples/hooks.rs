//! Observing an invocation with hook callbacks.
//!
//! Run with `cargo run --example hooks` (needs AWS credentials).

use strands_agents::hooks::{AfterModelCallEvent, BeforeToolCallEvent};
use strands_agents::models::{BedrockModel, BedrockModelConfig};
use strands_agents::tools::{FunctionTool, ToolContext};
use strands_agents::types::JsonValue;
use strands_agents::Agent;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let clock = FunctionTool::builder("clock")
        .description("The current time")
        .handler(|_input: JsonValue, _ctx: ToolContext| async move { Ok("12:00") });

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).tool(clock).build()?;

    agent
        .hooks()
        .add_sync_callback(|event: &mut BeforeToolCallEvent| {
            println!("calling tool {}", event.tool_use.name);
            Ok(())
        });

    agent.hooks().add_callback(|event: &mut AfterModelCallEvent| {
        let attempt = event.attempt_count;
        Box::pin(async move {
            println!("model call finished (attempt {attempt})");
            Ok(())
        })
    });

    let result = agent.invoke("What time is it?").await?;
    println!("{result}");

    Ok(())
}
