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

// --8<-- [start:tool_update_config]
use strands_agents::models::{BedrockModel, BedrockModelConfig};
use strands_agents::tools::ToolContext;
use strands_agents::tool;

/// Update the temperature of the agent.
#[tool]
async fn update_temperature(
    /// Temperature value for the model to use.
    temperature: f32,
    context: ToolContext,
) -> Result<String, Error> {
    // `Agent::model()` hands out an `Arc<dyn Model>`, so swap in a model built with the
    // new temperature instead of mutating the running one.
    let model = BedrockModel::new(BedrockModelConfig {
        model_id: Some("global.anthropic.claude-sonnet-5".to_owned()),
        temperature: Some(temperature),
        ..Default::default()
    })
    .await?;
    context.agent.set_model(model);
    Ok(format!("Temperature updated to {temperature}"))
}
// --8<-- [end:tool_update_config]

async fn tool_update_config() -> Result<(), Error> {
    // --8<-- [start:tool_update_config_agent]
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    let agent = Agent::builder()
        .model(BedrockModel::with_model_id("global.anthropic.claude-sonnet-5").await?)
        .tool(UpdateTemperatureTool::new())
        .build()?;
    // --8<-- [end:tool_update_config_agent]
    let _ = agent;
    Ok(())
}

async fn reasoning() -> Result<(), Error> {
    // --8<-- [start:reasoning]
    use serde_json::json;
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    // Create a Bedrock model with reasoning configuration
    let bedrock_model = BedrockModel::new(BedrockModelConfig {
        model_id: Some("global.anthropic.claude-sonnet-5".to_owned()),
        additional_request_fields: Some(json!({
            "thinking": {
                "type": "enabled",
                "budget_tokens": 4096, // Minimum of 1,024
            }
        })),
        ..Default::default()
    })
    .await?;

    // Create an agent with the reasoning-enabled model
    let agent = Agent::new(bedrock_model)?;

    // Ask a question that requires reasoning
    let response = agent
        .invoke(
            "If a train travels at 120 km/h and needs to cover 450 km, \
             how long will the journey take?",
        )
        .await?;
    // --8<-- [end:reasoning]
    let _ = response;
    Ok(())
}

async fn structured_output() -> Result<(), Error> {
    // --8<-- [start:structured_output]
    use schemars::JsonSchema;
    use serde::Deserialize;
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    /// Analyze product information from text.
    #[derive(Debug, Deserialize, JsonSchema)]
    struct ProductAnalysis {
        /// Product name.
        name: String,
        /// Product category.
        category: String,
        /// Price in USD.
        price: f64,
        /// Key product features.
        features: Vec<String>,
        /// Customer rating 1-5.
        rating: Option<f64>,
    }

    let agent = Agent::new(BedrockModel::default_model().await?)?;

    let product: ProductAnalysis = agent
        .structured_output(
            r#"Analyze this product: The UltraBook Pro is a premium laptop computer
            priced at $1,299. It features a 15-inch 4K display, 16GB RAM, 512GB SSD,
            and 12-hour battery life. Customer reviews average 4.5 stars."#,
        )
        .await?;

    println!("Product: {}", product.name);
    println!("Category: {}", product.category);
    println!("Price: ${}", product.price);
    println!("Features: {}", product.features.join(", "));
    println!("Rating: {:?}", product.rating);
    // --8<-- [end:structured_output]
    Ok(())
}

async fn native_token_count() -> Result<(), Error> {
    // --8<-- [start:native_token_count]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};

    let model = BedrockModel::new(BedrockModelConfig {
        model_id: Some("global.anthropic.claude-sonnet-5".to_owned()),
        use_native_token_count: Some(true),
        ..Default::default()
    })
    .await?;
    // --8<-- [end:native_token_count]
    let _ = model;
    Ok(())
}

async fn mantle_config() -> Result<(), Error> {
    // --8<-- [start:mantle_config]
    use strands_agents::models::{
        BedrockMantleConfig, OpenAIApi, OpenAIModel, OpenAIModelConfig,
    };
    use strands_agents::Agent;

    let model = OpenAIModel::new(OpenAIModelConfig {
        api: OpenAIApi::Responses,
        model_id: Some("openai.gpt-oss-120b".to_owned()),
        mantle: Some(BedrockMantleConfig {
            region: Some("us-east-1".to_owned()),
            ..Default::default()
        }),
        ..Default::default()
    })?;

    let agent = Agent::new(model)?;
    println!("{}", agent.invoke("What is 2+2?").await?);
    // --8<-- [end:mantle_config]
    Ok(())
}

async fn bedrock_mantle() -> Result<(), Error> {
    // --8<-- [start:bedrock_mantle]
    use strands_agents::models::{OpenAIApi, OpenAIModel, OpenAIModelConfig};
    use strands_agents::Agent;

    let region = "us-east-1";
    let model = OpenAIModel::new(OpenAIModelConfig {
        api: OpenAIApi::Responses,
        model_id: Some("openai.gpt-oss-120b".to_owned()),
        api_key: Some("<BEDROCK_API_KEY>".to_owned()),
        base_url: Some(format!("https://bedrock-mantle.{region}.api.aws/v1")),
        ..Default::default()
    })?;

    let agent = Agent::new(model)?;
    println!("{}", agent.invoke("What is 2+2?").await?);
    // --8<-- [end:bedrock_mantle]
    Ok(())
}

async fn invalid_model_id() -> Result<(), Error> {
    // --8<-- [start:invalid_model_id]
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    let model_id = "anthropic.claude-3-5-sonnet-20241022-v2:0";
    let model = BedrockModel::with_model_id(model_id).await?;
    let agent = Agent::new(model)?;
    // --8<-- [end:invalid_model_id]
    let _ = agent;
    Ok(())
}

async fn cache_config_arn() -> Result<(), Error> {
    // --8<-- [start:cache_config_arn]
    use strands_agents::models::{
        BedrockModel, BedrockModelConfig, CacheConfig, CacheStrategy,
    };
    use strands_agents::Agent;

    let bedrock_model = BedrockModel::new(BedrockModelConfig {
        model_id: Some(
            "arn:aws:bedrock:us-east-1:123456789012:application-inference-profile/abc123"
                .to_owned(),
        ),
        cache_config: Some(CacheConfig {
            strategy: Some(CacheStrategy::Anthropic),
            ..Default::default()
        }),
        ..Default::default()
    })
    .await?;

    let agent = Agent::new(bedrock_model)?;
    // --8<-- [end:cache_config_arn]
    let _ = agent;
    Ok(())
}
