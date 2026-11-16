//! Amazon Bedrock: a `BedrockModel` with an explicit model id, region and inference settings.
//! `Agent::builder()` without a model uses a `BedrockModel` with the default settings.
//!
//! AWS credentials come from the default chain (environment, profile, instance role). The
//! region comes from the options, then `AWS_REGION`, then the profile.
//!
//! ```sh
//! cargo run -p strands --example bedrock
//! ```

#![expect(clippy::print_stdout, reason = "examples print their results")]

use strands::models::bedrock::{BedrockModel, BedrockModelConfig, BedrockModelOptions};
use strands::{Agent, AgentConfig};

#[tokio::main]
async fn main() -> Result<(), strands::Error> {
    let model = BedrockModel::new(BedrockModelOptions {
        region: Some("us-west-2".to_owned()),
        config: BedrockModelConfig {
            model_id: Some("us.anthropic.claude-haiku-4-5-20251001-v1:0".to_owned()),
            max_tokens: Some(512),
            temperature: Some(0.2),
            ..Default::default()
        },
        ..Default::default()
    });

    let mut agent = Agent::new(AgentConfig {
        model: Some(model.into()),
        system_prompt: Some("You are a concise assistant.".into()),
        ..Default::default()
    })?;

    let result = agent
        .invoke("Name three uses of Amazon Bedrock, one line each.")
        .await?;
    let usage = result.metrics().accumulated_usage();
    println!(
        "\n{:?}: {} input and {} output tokens",
        result.stop_reason(),
        usage.input_tokens(),
        usage.output_tokens()
    );

    agent.shutdown().await
}
