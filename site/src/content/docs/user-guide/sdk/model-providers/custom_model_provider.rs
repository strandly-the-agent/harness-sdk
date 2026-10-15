//! Rust examples for the custom model provider documentation.

// --8<-- [start:create_model_class]
use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use strands_agents::models::{
    CountTokensOptions, Model, ModelEventStream, StreamOptions,
};
use strands_agents::types::Message;
use strands_agents::{Error, Result};

/// Configuration for the custom model.
#[derive(Clone, Debug, Default)]
pub struct CustomModelConfig {
    pub api_key: Option<String>,
    pub model_id: Option<String>,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    // Add any additional configuration parameters specific to your model
}

/// Custom model provider implementation.
pub struct CustomModel {
    config: CustomModelConfig,
    // Plus whatever your API needs: an HTTP client, a transport, credentials.
}

impl CustomModel {
    pub fn new(config: CustomModelConfig) -> Result<Self> {
        let api_key = config
            .api_key
            .clone()
            .or_else(|| std::env::var("CUSTOM_API_KEY").ok())
            .ok_or_else(|| Error::model("no API key configured for CustomModel"))?;
        let _ = api_key;
        Ok(Self { config })
    }
}
// --8<-- [end:create_model_class]

// --8<-- [start:implement_stream]
#[async_trait]
impl Model for CustomModel {
    fn model_id(&self) -> Option<String> {
        self.config.model_id.clone()
    }

    fn context_window_limit(&self) -> Option<u64> {
        Some(128_000)
    }

    fn stream<'a>(
        &'a self,
        messages: &'a [Message],
        options: &'a StreamOptions,
    ) -> ModelEventStream<'a> {
        use strands_agents::models::{ContentBlockDelta, ModelStreamEvent};
        use strands_agents::types::{Role, StopReason};

        // Format the request for your API, then translate the response back into
        // `ModelStreamEvent`s. `options.system_prompt`, `options.tool_specs`, and
        // `options.cancel` carry the rest of the request.
        let _ = (messages, options);

        stream::iter(vec![
            Ok(ModelStreamEvent::MessageStart { role: Role::Assistant }),
            Ok(ModelStreamEvent::ContentBlockStart { start: None }),
            Ok(ModelStreamEvent::ContentBlockDelta {
                delta: ContentBlockDelta::Text { text: "Hello".to_owned() },
            }),
            Ok(ModelStreamEvent::ContentBlockStop {}),
            Ok(ModelStreamEvent::MessageStop {
                stop_reason: StopReason::EndTurn,
                additional_model_response_fields: None,
            }),
        ])
        .boxed()
    }

    async fn count_tokens(
        &self,
        messages: &[Message],
        options: &CountTokensOptions,
    ) -> Result<u64> {
        // Omit this to keep the trait's character-based heuristic.
        Ok(strands_agents::models::estimate_tokens_heuristic(messages, options))
    }
}
// --8<-- [end:implement_stream]

async fn usage_example() -> Result<()> {
    // --8<-- [start:usage_example]
    use strands_agents::Agent;

    let model = CustomModel::new(CustomModelConfig {
        api_key: Some("<KEY>".to_owned()),
        model_id: Some("your-model-id".to_owned()),
        max_tokens: Some(1000),
        temperature: Some(0.7),
        ..Default::default()
    })?;

    let agent = Agent::new(model)?;
    println!("{}", agent.invoke("Hello, how can you help me today?").await?);
    // --8<-- [end:usage_example]
    Ok(())
}
