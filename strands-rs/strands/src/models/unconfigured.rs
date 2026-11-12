//! A stand-in for a model the build can't provide.

use std::fmt;
use std::sync::{Mutex, PoisonError};

use crate::error::{DefaultNotConfiguredError, Result};
use crate::models::{BaseModelConfig, BoxStream, Model, ModelRequest, ModelStreamEvent};

/// Fails every call with [`DefaultNotConfiguredError`], so building an agent stays infallible
/// and the error surfaces on first use.
pub(crate) struct UnconfiguredModel {
    config: Mutex<BaseModelConfig>,
    message: String,
}

impl UnconfiguredModel {
    pub(crate) fn new(model_id: Option<String>, message: impl Into<String>) -> Self {
        Self {
            config: Mutex::new(BaseModelConfig {
                model_id,
                ..Default::default()
            }),
            message: message.into(),
        }
    }
}

/// The default model when none is configured (TS `new BedrockModel()`).
pub(crate) fn default_model() -> UnconfiguredModel {
    bedrock_model(None)
}

/// The model for a model id string (TS `new BedrockModel({ modelId })`).
pub(crate) fn bedrock_model(model_id: Option<String>) -> UnconfiguredModel {
    let message = if cfg!(feature = "bedrock") {
        "no model configured: the Bedrock provider is not available in this build yet; pass a model to the agent"
    } else {
        "no model configured: pass a model to the agent or enable the strands feature \"bedrock\""
    };
    UnconfiguredModel::new(model_id, message)
}

impl Model for UnconfiguredModel {
    fn stream(&self, _request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>> {
        let error = DefaultNotConfiguredError::new(self.message.clone());
        Box::pin(futures::stream::once(async move { Err(error.into()) }))
    }

    fn get_config(&self) -> BaseModelConfig {
        self.config.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn update_config(&self, config: BaseModelConfig) {
        self.config
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .merge(config);
    }
}

impl fmt::Debug for UnconfiguredModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnconfiguredModel").finish_non_exhaustive()
    }
}
