//! Rust examples for the Amazon Bedrock model provider documentation.
//! These examples demonstrate common usage patterns for the BedrockModel.

use strands_agents::Error;

async fn custom_credentials() -> Result<(), Error> {
    // --8<-- [start:custom_credentials]
    // AWS credentials come from the AWS SDK for Rust's default provider chain
    // (environment variables, a shared profile, SSO, or the instance role).
    // See https://docs.aws.amazon.com/sdk-for-rust/latest/dg/credproviders.html
    use strands_agents::models::BedrockModel;

    let bedrock_model = BedrockModel::builder()
        .model_id("global.anthropic.claude-sonnet-5")
        .region("us-west-2")
        .build()
        .await?;
    // --8<-- [end:custom_credentials]
    let _ = bedrock_model;
    Ok(())
}

async fn basic_default() -> Result<(), Error> {
    // --8<-- [start:basic_default]
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    let agent = Agent::new(BedrockModel::default_model().await?)?;

    let response = agent.invoke("Tell me about Amazon Bedrock.").await?;
    // --8<-- [end:basic_default]
    let _ = response;
    Ok(())
}

async fn basic_model_id() -> Result<(), Error> {
    // --8<-- [start:basic_model_id]
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    // Create an agent using the model
    let model = BedrockModel::with_model_id("global.anthropic.claude-sonnet-5").await?;
    let agent = Agent::new(model)?;

    let response = agent.invoke("Tell me about Amazon Bedrock.").await?;
    // --8<-- [end:basic_model_id]
    let _ = response;
    Ok(())
}

async fn basic_model_instance() -> Result<(), Error> {
    // --8<-- [start:basic_model_instance]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    // Create a Bedrock model instance
    let bedrock_model = BedrockModel::new(BedrockModelConfig {
        model_id: Some("us.amazon.nova-premier-v1:0".to_owned()),
        temperature: Some(0.3),
        top_p: Some(0.8),
        ..Default::default()
    })
    .await?;

    // Create an agent using the BedrockModel instance
    let agent = Agent::new(bedrock_model)?;

    // Use the agent
    let response = agent.invoke("Tell me about Amazon Bedrock.").await?;
    // --8<-- [end:basic_model_instance]
    let _ = response;
    Ok(())
}

async fn configuration() -> Result<(), Error> {
    // --8<-- [start:configuration]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;
    use std::time::Duration;

    // Create a configured Bedrock model
    let bedrock_model = BedrockModel::builder()
        .region("us-east-1") // Specify a different region than the default
        .request_timeout(Duration::from_secs(600))
        .config(BedrockModelConfig {
            model_id: Some("global.anthropic.claude-sonnet-5".to_owned()),
            temperature: Some(0.3),
            stop_sequences: Some(vec!["###".to_owned(), "END".to_owned()]),
            ..Default::default()
        })
        .build()
        .await?;

    // Create an agent with the configured model
    let agent = Agent::new(bedrock_model)?;

    // Use the agent
    let response = agent
        .invoke("Write a short story about an AI assistant.")
        .await?;
    // --8<-- [end:configuration]
    let _ = response;
    Ok(())
}

async fn streaming() -> Result<(), Error> {
    // --8<-- [start:streaming]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};

    // Streaming model (default)
    let streaming_model = BedrockModel::new(BedrockModelConfig {
        model_id: Some("global.anthropic.claude-sonnet-5".to_owned()),
        stream: Some(true), // This is the default
        ..Default::default()
    })
    .await?;

    // Non-streaming model
    let non_streaming_model = BedrockModel::new(BedrockModelConfig {
        model_id: Some("us.meta.llama3-2-90b-instruct-v1:0".to_owned()),
        stream: Some(false), // Disable streaming
        ..Default::default()
    })
    .await?;
    // --8<-- [end:streaming]
    let _ = (streaming_model, non_streaming_model);
    Ok(())
}
