//! Agent configuration (TS `AgentConfig` in `agent/agent.ts`).

use std::collections::HashMap;

use crate::agent::agent::Agent;
use crate::error::Result;
use crate::models::model::ModelSource;
use crate::retry::RetryStrategy;
use crate::tools::ToolSource;
use crate::tools::executors::ToolExecutor;
use crate::tools::structured_output::StructuredOutputSchema;
use crate::types::json::JsonValue;
use crate::types::messages::{Message, SystemPrompt};

/// How an agent is set up (TS `AgentConfig`).
///
/// Every field is optional. Build an agent from it with [`Agent::new`], or set the same keys
/// through [`Agent::builder`]. TS keys whose subsystem this version doesn't implement yet are
/// either absent (conversation and context managers, plugins, interventions, sessions, memory,
/// sandbox, storage, background tasks, trace attributes) or rejected by [`Agent::new`] when set
/// (`baggage_attributes`).
///
/// ```
/// use strands::{Agent, AgentConfig};
/// use strands::testing::MockModel;
///
/// let agent = Agent::new(AgentConfig {
///     model: Some(MockModel::new().into()),
///     system_prompt: Some("You are a concise assistant.".into()),
///     printer: false,
///     ..AgentConfig::default()
/// })?;
/// assert_eq!(agent.name(), "Strands Agent");
/// # Ok::<(), strands::Error>(())
/// ```
#[derive(Debug, Clone, bon::Builder)]
#[builder(
    builder_type(name = AgentBuilder, doc {
        /// Builds an [`Agent`] key by key (TS `new Agent({ .. })`); see [`AgentConfig`] for the keys.
    }),
    start_fn(name = builder, vis = "pub(crate)"),
    state_mod(vis = "pub"),
    finish_fn(name = into_config, vis = "")
)]
pub struct AgentConfig {
    /// The tools the agent can call. The builder's `tools` extends this list on every call.
    #[builder(field)]
    pub tools: Vec<ToolSource>,
    /// The model the agent uses: a model instance, or a Bedrock model id. Defaults to Bedrock,
    /// resolved on first use.
    #[builder(into)]
    pub model: Option<ModelSource>,
    /// The model for auxiliary calls outside the main loop (summarization, memory extraction,
    /// classification). Defaults to `model`.
    #[builder(into)]
    pub aux_model: Option<ModelSource>,
    /// The conversation history to start from.
    #[builder(default)]
    pub messages: Vec<Message>,
    /// The system prompt.
    #[builder(into)]
    pub system_prompt: Option<SystemPrompt>,
    /// The initial application state (TS `appState`).
    pub app_state: Option<serde_json::Map<String, JsonValue>>,
    /// The initial model state (TS `modelState`), for models that keep conversation state.
    pub model_state: Option<serde_json::Map<String, JsonValue>>,
    /// Whether to print the agent's output to stdout as it streams. Defaults to `true`; turn it
    /// off for services.
    #[builder(default = true)]
    pub printer: bool,
    /// Retry strategies for failed model calls. `None` uses `DefaultModelRetryStrategy`; an
    /// empty list disables retries (TS `null`).
    pub retry_strategy: Option<Vec<RetryStrategy>>,
    /// The type every invocation's structured output must have, unless an invocation sets its
    /// own.
    pub structured_output_schema: Option<StructuredOutputSchema>,
    /// The agent's name. Defaults to `Strands Agent`.
    #[builder(into)]
    pub name: Option<String>,
    /// What the agent does; the description agents used as tools advertise.
    #[builder(into)]
    pub description: Option<String>,
    /// The agent's id. Defaults to `agent`.
    #[builder(into)]
    pub id: Option<String>,
    /// How the agent runs the tools of one turn. Defaults to concurrent.
    #[builder(into)]
    pub tool_executor: Option<ToolExecutor>,
    /// OpenTelemetry baggage set for every invocation. Not supported yet: setting it makes
    /// [`Agent::new`] fail.
    pub baggage_attributes: Option<HashMap<String, String>>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            tools: Vec::new(),
            model: None,
            aux_model: None,
            messages: Vec::new(),
            system_prompt: None,
            app_state: None,
            model_state: None,
            printer: true,
            retry_strategy: None,
            structured_output_schema: None,
            name: None,
            description: None,
            id: None,
            tool_executor: None,
            baggage_attributes: None,
        }
    }
}

impl<S: agent_builder::State> AgentBuilder<S> {
    /// Adds tools: anything convertible to a [`ToolSource`], such as `#[tool]` structs, tool
    /// lists and agents. Calling it again adds more tools.
    pub fn tools<T: Into<ToolSource>>(mut self, tools: impl IntoIterator<Item = T>) -> Self {
        self.tools.extend(tools.into_iter().map(Into::into));
        self
    }
}

impl<S: agent_builder::IsComplete> AgentBuilder<S> {
    /// Builds the agent.
    ///
    /// # Errors
    ///
    /// As [`Agent::new`].
    pub fn build(self) -> Result<Agent> {
        Agent::new(self.into_config())
    }
}
