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

async fn multimodal_full() -> Result<(), Error> {
    // --8<-- [start:multimodal_full]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::types::{
        ContentBlock, DocumentBlock, DocumentFormat, DocumentSource,
    };
    use strands_agents::Agent;

    let bedrock_model = BedrockModel::new(BedrockModelConfig {
        model_id: Some("global.anthropic.claude-sonnet-5".to_owned()),
        ..Default::default()
    })
    .await?;

    let agent = Agent::new(bedrock_model)?;

    let document_bytes = b"Once upon a time...".to_vec();

    // Send multimodal content directly to invoke
    let response = agent
        .invoke(vec![
            ContentBlock::Document(DocumentBlock {
                format: DocumentFormat::Txt,
                name: "example".to_owned(),
                source: DocumentSource::Bytes(document_bytes),
                citations: None,
                context: None,
            }),
            ContentBlock::Text("Tell me about the document.".to_owned()),
        ])
        .await?;
    // --8<-- [end:multimodal_full]
    let _ = response;
    Ok(())
}

async fn s3_location() -> Result<(), Error> {
    // --8<-- [start:s3_location]
    use strands_agents::models::BedrockModel;
    use strands_agents::types::{
        ContentBlock, DocumentBlock, DocumentFormat, DocumentSource, S3Location,
    };
    use strands_agents::Agent;

    let agent = Agent::new(BedrockModel::default_model().await?)?;

    let response = agent
        .invoke(vec![
            ContentBlock::Document(DocumentBlock {
                format: DocumentFormat::Pdf,
                name: "report.pdf".to_owned(),
                source: DocumentSource::Location(S3Location {
                    uri: "s3://my-bucket/documents/report.pdf".to_owned(),
                    // Optional: for cross-account access
                    bucket_owner: Some("123456789012".to_owned()),
                }),
                citations: None,
                context: None,
            }),
            ContentBlock::Text("Summarize this document.".to_owned()),
        ])
        .await?;
    // --8<-- [end:s3_location]
    let _ = response;
    Ok(())
}

async fn guardrails() -> Result<(), Error> {
    // --8<-- [start:guardrails]
    use strands_agents::models::{
        BedrockGuardrailConfig, BedrockGuardrailRedactionConfig, BedrockModel,
        BedrockModelConfig, GuardrailStreamProcessingMode, GuardrailTraceMode,
    };
    use strands_agents::Agent;

    // Using guardrails with BedrockModel
    let bedrock_model = BedrockModel::new(BedrockModelConfig {
        model_id: Some("global.anthropic.claude-sonnet-5".to_owned()),
        guardrail_config: Some(BedrockGuardrailConfig {
            guardrail_identifier: "your-guardrail-id".to_owned(),
            guardrail_version: "DRAFT".to_owned(),
            // Options: Enabled, Disabled, EnabledFull
            trace: Some(GuardrailTraceMode::Enabled),
            // Options: Sync, Async
            stream_processing_mode: Some(GuardrailStreamProcessingMode::Sync),
            redaction: Some(BedrockGuardrailRedactionConfig {
                input: Some(true), // Default: true
                // Custom redaction message
                input_message: Some("[User input redacted.]".to_owned()),
                output: Some(false), // Default: false
                output_message: Some("[Assistant output redacted.]".to_owned()),
            }),
            // Only evaluate the latest user message (default: false)
            guard_latest_user_message: Some(true),
        }),
        ..Default::default()
    })
    .await?;

    let guardrail_agent = Agent::new(bedrock_model)?;

    let response = guardrail_agent
        .invoke("Can you tell me about the Strands Harness SDK?")
        .await?;
    // --8<-- [end:guardrails]
    let _ = response;
    Ok(())
}

async fn update_config() -> Result<(), Error> {
    // --8<-- [start:update_config]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};

    // Create the model with initial configuration
    let bedrock_model = BedrockModel::new(BedrockModelConfig {
        model_id: Some("global.anthropic.claude-sonnet-5".to_owned()),
        temperature: Some(0.7),
        ..Default::default()
    })
    .await?;

    // Update configuration later; `None` fields are left alone
    bedrock_model.update_config(BedrockModelConfig {
        temperature: Some(0.3),
        ..Default::default()
    });
    // --8<-- [end:update_config]
    Ok(())
}
