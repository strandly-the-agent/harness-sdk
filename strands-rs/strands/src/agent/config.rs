//! Agent configuration (TS `AgentConfig` in `agent/agent.ts`) and the model a config names.

use std::sync::{Arc, Mutex, PoisonError};

use crate::error::{DefaultNotConfiguredError, Error, Result};
use crate::models::model::{AggregatedEvent, BaseModelConfig, BoxStream, Model, ModelRequest};
use crate::models::streaming::ModelStreamEvent;
use crate::tools::{ToolExecutor, ToolSource};
use crate::types::json::JsonObject;
use crate::types::messages::{Message, SystemPrompt};

/// The model an agent runs with (TS `AgentConfig.model`: a model instance or a Bedrock model id).
///
/// ```
/// use strands::agent::ModelSource;
/// use strands::testing::MockMessageModel;
///
/// assert!(matches!(ModelSource::from(MockMessageModel::new()), ModelSource::Model(_)));
/// assert!(matches!(ModelSource::from("global.anthropic.claude-sonnet-4-6"), ModelSource::Id(_)));
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ModelSource {
    /// A model instance.
    Model(Arc<dyn Model>),
    /// A Bedrock model id; creates a `BedrockModel`.
    Id(String),
}

impl From<Arc<dyn Model>> for ModelSource {
    fn from(model: Arc<dyn Model>) -> Self {
        Self::Model(model)
    }
}

impl<M: Model> From<M> for ModelSource {
    fn from(model: M) -> Self {
        Self::Model(Arc::new(model))
    }
}

impl<M: Model> From<Arc<M>> for ModelSource {
    fn from(model: Arc<M>) -> Self {
        Self::Model(model)
    }
}

impl From<&str> for ModelSource {
    fn from(model_id: &str) -> Self {
        Self::Id(model_id.to_owned())
    }
}

impl From<String> for ModelSource {
    fn from(model_id: String) -> Self {
        Self::Id(model_id)
    }
}

impl ModelSource {
    pub(crate) fn into_model(self) -> Result<Arc<dyn Model>> {
        match self {
            Self::Model(model) => Ok(model),
            Self::Id(model_id) => Err(Error::other(format!(
                "model_id=<{model_id}> | a model id string needs the Bedrock model provider, {BEDROCK_UNAVAILABLE}"
            ))),
        }
    }
}

#[cfg(feature = "bedrock")]
const BEDROCK_UNAVAILABLE: &str = "which this version doesn't include yet; pass a model instance";
#[cfg(not(feature = "bedrock"))]
const BEDROCK_UNAVAILABLE: &str = "which needs the `bedrock` feature; enable it or pass a model instance";

/// Stands in for the default Bedrock model until a call needs it (#45): `build()` stays sync and
/// a missing provider fails the first model call, not the build.
#[derive(Debug, Default)]
pub(crate) struct DefaultModel {
    config: Mutex<BaseModelConfig>,
}

impl DefaultModel {
    fn unavailable() -> Error {
        DefaultNotConfiguredError::new(format!(
            "no model configured: the default model is a BedrockModel, {BEDROCK_UNAVAILABLE}"
        ))
        .into()
    }
}

impl Model for DefaultModel {
    fn stream(&self, _request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>> {
        Box::pin(futures::stream::once(async { Err(Self::unavailable()) }))
    }

    fn stream_aggregated(&self, _request: ModelRequest) -> BoxStream<'static, Result<AggregatedEvent>> {
        Box::pin(futures::stream::once(async { Err(Self::unavailable()) }))
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

/// Everything an agent is built from (TS `AgentConfig`); build one with [`Agent::builder`] or a
/// struct literal and [`Agent::new`].
///
/// Keys whose TS behaviour lands with later subsystems (conversation and context managers,
/// plugins, retry strategies, interventions, sessions, memory, sandbox, storage, background tasks,
/// structured output, telemetry attributes and the printer) are added with them, so no key is
/// accepted and ignored.
///
/// ```
/// use strands::agent::AgentConfig;
/// use strands::testing::MockMessageModel;
/// use strands::Agent;
///
/// let agent = Agent::new(AgentConfig {
///     model: Some(MockMessageModel::new().into()),
///     system_prompt: Some("You are a concise assistant.".into()),
///     ..Default::default()
/// })?;
/// assert_eq!(agent.name(), "Strands Agent");
/// # Ok::<(), strands::Error>(())
/// ```
///
/// [`Agent::builder`]: crate::Agent::builder
/// [`Agent::new`]: crate::Agent::new
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(
    builder_type(
        name = AgentBuilder,
        doc {
            /// Builds an [`Agent`](crate::Agent); start with [`Agent::builder`](crate::Agent::builder).
        }
    ),
    start_fn(name = builder, vis = "pub(crate)"),
    finish_fn(name = into_config, vis = "pub(crate)"),
    on(String, into)
)]
pub struct AgentConfig {
    /// Tools to register; nested lists are flattened. The builder's `.tools(..)` extends the list.
    #[builder(field)]
    pub tools: Vec<ToolSource>,
    /// The model (default: a `BedrockModel` with the default model id, resolved on first use).
    #[builder(into)]
    pub model: Option<ModelSource>,
    /// The model for auxiliary side calls (summarization, memory extraction, ...); defaults to
    /// `model`.
    #[builder(into)]
    pub aux_model: Option<ModelSource>,
    /// Messages seeding the conversation history.
    #[builder(default)]
    pub messages: Vec<Message>,
    /// The system prompt.
    #[builder(into)]
    pub system_prompt: Option<SystemPrompt>,
    /// Initial app state.
    #[builder(default)]
    pub app_state: JsonObject,
    /// Initial model state.
    #[builder(default)]
    pub model_state: JsonObject,
    /// The agent name (default `Strands Agent`).
    pub name: Option<String>,
    /// What the agent does.
    pub description: Option<String>,
    /// The agent id (default `agent`).
    pub id: Option<String>,
    /// How the tool calls of one model turn run (default concurrent).
    #[builder(into)]
    pub tool_executor: Option<ToolExecutor>,
}

impl<S: agent_builder::State> AgentBuilder<S> {
    /// Adds tools; calling it again extends the list rather than replacing it.
    ///
    /// Mixed tool types need [`tool_list!`](crate::tool_list) or [`ToolSource::from`].
    pub fn tools(mut self, tools: impl IntoIterator<Item = impl Into<ToolSource>>) -> Self {
        self.tools.extend(tools.into_iter().map(Into::into));
        self
    }

    /// Builds the agent; no model, tool or plugin initialization runs until the first call.
    ///
    /// # Errors
    ///
    /// As [`Agent::new`](crate::Agent::new).
    pub fn build(self) -> Result<crate::Agent>
    where
        S: agent_builder::IsComplete,
    {
        crate::Agent::new(self.into_config())
    }
}
