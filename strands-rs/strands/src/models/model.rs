//! The [`Model`] trait, its request and config types, and prompt caching config (TS
//! `models/model.ts`).

use std::fmt;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::agent::agent_metadata::AgentMetadata;
use crate::cancel::CancelSignal;
use crate::error::Result;
use crate::logging::warn_once;
use crate::models::model_info::{
    ModelCapabilities, ModelInfo, ModelLimits, default_context_window_limit, lookup_model_info,
    merge_model_info,
};
use crate::models::streaming::{ModelMetadataEventData, ModelStreamEvent};
use crate::models::summarize::{SummarizeResult, summarize_with_stream};
use crate::state_store::StateStore;
use crate::types::json::JsonValue;
use crate::types::messages::{ContentBlock, Message, Role, StopReason, SystemPrompt};
use crate::types::tools::{ToolChoice, ToolSpec};

mod aggregate;
mod tokens;

pub(crate) use aggregate::aggregate;

/// A boxed, `Send` stream, as [`Model::stream`] returns.
pub type BoxStream<'a, T> = Pin<Box<dyn Stream<Item = T> + Send + 'a>>;

/// Configuration every model provider supports (TS `BaseModelConfig`). `None` leaves a setting to
/// the provider.
///
/// ```
/// use strands::models::model::BaseModelConfig;
///
/// let config = BaseModelConfig { model_id: Some("claude-sonnet-4-6".into()), max_tokens: Some(1024), ..Default::default() };
/// assert_eq!(config, BaseModelConfig::builder().model_id("claude-sonnet-4-6").max_tokens(1024).build());
/// ```
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, bon::Builder)]
#[serde(rename_all = "camelCase")]
pub struct BaseModelConfig {
    /// The model identifier in the provider's catalog.
    #[builder(into)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    /// Maximum number of tokens to generate in the response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    /// Controls randomness in generation; the valid range is provider-specific.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// Controls diversity through nucleus sampling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    /// Maximum context window size in tokens. Shorthand for `model_info.limits.context_window`: it
    /// wins over the shared model info, and `model_info` wins over it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window_limit: Option<u64>,
    /// Overrides for what the SDK knows about the model, winning over the shared model info and the
    /// provider. Updating it replaces the previous override entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_info: Option<ModelInfo>,
}

impl BaseModelConfig {
    /// Shallow-merges `update` into this config: every field `update` sets replaces this one's (TS
    /// `{ ...config, ...update }` in `updateConfig`).
    ///
    /// ```
    /// use strands::models::model::BaseModelConfig;
    ///
    /// let mut config = BaseModelConfig::builder().model_id("a").max_tokens(10).build();
    /// config.merge(BaseModelConfig::builder().max_tokens(20).build());
    /// assert_eq!(config, BaseModelConfig::builder().model_id("a").max_tokens(20).build());
    /// ```
    pub fn merge(&mut self, update: Self) {
        let Self {
            model_id,
            max_tokens,
            temperature,
            top_p,
            context_window_limit,
            model_info,
        } = update;
        self.model_id = model_id.or(self.model_id.take());
        self.max_tokens = max_tokens.or(self.max_tokens);
        self.temperature = temperature.or(self.temperature);
        self.top_p = top_p.or(self.top_p);
        self.context_window_limit = context_window_limit.or(self.context_window_limit);
        self.model_info = model_info.or(self.model_info);
    }
}

/// Everything one model call needs (TS `stream(messages, options)` with `StreamOptions`).
///
/// ```
/// use strands::Message;
/// use strands::models::model::ModelRequest;
/// use strands::types::messages::Role;
///
/// let request = ModelRequest {
///     messages: vec![Message::new(Role::User, vec!["Hello".into()])],
///     system_prompt: Some("Be brief.".into()),
///     ..Default::default()
/// };
/// assert!(request.tool_specs.is_empty());
/// ```
#[derive(Debug, Clone, Default, bon::Builder)]
pub struct ModelRequest {
    /// The conversation to send.
    #[builder(default)]
    pub messages: Vec<Message>,
    /// The system prompt.
    #[builder(into)]
    pub system_prompt: Option<SystemPrompt>,
    /// The tools the model may use.
    #[builder(default)]
    pub tool_specs: Vec<ToolSpec>,
    /// How the model selects tools.
    pub tool_choice: Option<ToolChoice>,
    /// Runtime state for providers that keep conversation state server-side. The provider may
    /// read and write it while streaming; the agent passes a snapshot and keeps the changes only
    /// when the call succeeds.
    #[builder(default)]
    pub model_state: StateStore,
    /// How many trailing blocks of the last user message are rebuilt on every call.
    pub dynamic_trailing_blocks: Option<usize>,
    /// Metadata of the invoking agent.
    pub agent_metadata: Option<AgentMetadata>,
    /// JSON schema this call must produce, set by the agent on its structured output retry. The
    /// response must then be a call to the tool named in `tool_choice`, or a final text response
    /// that is exactly one JSON object valid against this schema.
    pub structured_output_schema: Option<JsonValue>,
    /// Cancels the call; providers forward it to abort an in-flight request where they can.
    pub cancel_signal: Option<CancelSignal>,
}

/// Options for [`Model::count_tokens`] (TS `CountTokensOptions`).
#[derive(Debug, Clone, Default, PartialEq, bon::Builder)]
pub struct CountTokensOptions {
    /// The system prompt to include in the count.
    #[builder(into)]
    pub system_prompt: Option<SystemPrompt>,
    /// The tool specifications to include in the count.
    #[builder(default)]
    pub tool_specs: Vec<ToolSpec>,
}

/// Options for [`Model::summarize`] (TS `SummarizeOptions`).
#[derive(Debug, Clone, Default, bon::Builder)]
pub struct SummarizeOptions {
    /// The request's system prompt.
    #[builder(into)]
    pub system_prompt: Option<SystemPrompt>,
    /// The request's tool specifications.
    #[builder(default)]
    pub tool_specs: Vec<ToolSpec>,
    /// Instructions replacing the default ones; `None` or an empty string keeps the defaults. The
    /// instruction not to act on the request or call tools is always sent.
    #[builder(into)]
    pub instructions: Option<String>,
    /// Cancels the call.
    pub cancel_signal: Option<CancelSignal>,
    /// Metadata of the invoking agent, as passed to [`Model::stream`].
    pub agent_metadata: Option<AgentMetadata>,
}

/// Input redaction a guardrail requested (TS `Redaction`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Redaction {
    /// The text to replace the last user message with.
    pub user_message: String,
}

impl Redaction {
    /// Creates a redaction.
    #[must_use]
    pub fn new(user_message: impl Into<String>) -> Self {
        Self {
            user_message: user_message.into(),
        }
    }
}

/// The outcome of [`Model::stream_aggregated`] (TS `StreamAggregatedResult`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct StreamAggregatedResult {
    /// The complete message from the model.
    pub message: Message,
    /// Why the model stopped.
    pub stop_reason: StopReason,
    /// The last metadata event (usage, metrics), if any.
    pub metadata: Option<ModelMetadataEventData>,
    /// Input redaction when a guardrail blocked input. Output redaction replaces the message.
    pub redaction: Option<Redaction>,
}

impl StreamAggregatedResult {
    /// Creates a result with no metadata or redaction.
    #[must_use]
    pub fn new(message: Message, stop_reason: StopReason) -> Self {
        Self {
            message,
            stop_reason,
            metadata: None,
            redaction: None,
        }
    }

    /// Sets the metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: ModelMetadataEventData) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Sets the redaction.
    #[must_use]
    pub fn with_redaction(mut self, redaction: Redaction) -> Self {
        self.redaction = Some(redaction);
        self
    }
}

/// An item of [`Model::stream_aggregated`]: each stream event as it arrives, each content block
/// as it completes, and finally the result (TS `streamAggregated` yields events and blocks and
/// returns the result).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum AggregatedEvent {
    /// A stream event, passed through.
    ModelStreamEvent(ModelStreamEvent),
    /// A completed content block.
    ContentBlock(ContentBlock),
    /// The aggregated message; always the last item of a successful stream.
    Done(Box<StreamAggregatedResult>),
}

/// A model provider (TS `Model`).
///
/// Implement [`stream`](Model::stream), [`get_config`](Model::get_config) and
/// [`update_config`](Model::update_config); everything else has a default. `stream` returns a
/// `'static` stream so model calls can be spawned: clone what the stream needs out of `&self`.
///
/// ```
/// use std::sync::Mutex;
///
/// use strands::models::model::{BaseModelConfig, BoxStream, Model, ModelRequest};
/// use strands::models::streaming::{
///     ModelContentBlockStopEventData, ModelMessageStopEventData, ModelStreamEvent, TextDelta,
/// };
/// use strands::types::messages::StopReason;
///
/// struct Echo {
///     config: Mutex<BaseModelConfig>,
/// }
///
/// impl Model for Echo {
///     fn stream(&self, _request: ModelRequest) -> BoxStream<'static, strands::Result<ModelStreamEvent>> {
///         let events = vec![
///             Ok(TextDelta::new("hi").into()),
///             Ok(ModelContentBlockStopEventData::new().into()),
///             Ok(ModelMessageStopEventData::new(StopReason::EndTurn).into()),
///         ];
///         Box::pin(futures::stream::iter(events))
///     }
///
///     fn get_config(&self) -> BaseModelConfig {
///         self.config.lock().unwrap().clone()
///     }
///
///     fn update_config(&self, config: BaseModelConfig) {
///         self.config.lock().unwrap().merge(config);
///     }
/// }
///
/// let model = Echo { config: Mutex::new(BaseModelConfig::builder().model_id("echo").build()) };
/// assert_eq!(model.model_id().as_deref(), Some("echo"));
/// assert!(!model.stateful());
/// ```
#[async_trait]
pub trait Model: Send + Sync + 'static {
    /// Streams the model's response to `request`.
    fn stream(&self, request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>>;

    /// Streams the response with content blocks aggregated (TS `streamAggregated`): yields every
    /// stream event, each content block when it completes, and finally
    /// [`AggregatedEvent::Done`].
    ///
    /// Every error is a model error: an error that isn't one becomes an [`Error::Model`] carrying
    /// it as its source. A response that stops at its token limit yields [`Error::MaxTokens`] with
    /// the partial message.
    ///
    /// [`Error::Model`]: crate::Error::Model
    /// [`Error::MaxTokens`]: crate::Error::MaxTokens
    fn stream_aggregated(&self, request: ModelRequest) -> BoxStream<'static, Result<AggregatedEvent>> {
        aggregate(self.stream(request))
    }

    /// The current configuration.
    fn get_config(&self) -> BaseModelConfig;

    /// Shallow-merges `config` into the current configuration.
    fn update_config(&self, config: BaseModelConfig);

    /// The configured model id.
    fn model_id(&self) -> Option<String> {
        self.get_config().model_id
    }

    /// Everything the SDK knows about the configured model, resolved on each call. Layers, later
    /// ones winning: the shared model info for [`Model::model_id`], temporary messages off (a provider
    /// that supports them turns them back on), the `context_window_limit` config value, and the
    /// `model_info` config override.
    ///
    /// A provider that knows better overrides this: put its facts over [`base_model_info`], then
    /// the user's `get_config().model_info` over those. Don't call [`Model::stateful`] from the
    /// override; it calls this.
    ///
    /// ```
    /// use std::sync::Mutex;
    ///
    /// use strands::models::model::{BaseModelConfig, BoxStream, Model, ModelRequest, base_model_info};
    /// use strands::models::model_info::{ModelCapabilities, ModelInfo};
    /// use strands::models::streaming::ModelStreamEvent;
    ///
    /// struct Provider {
    ///     config: Mutex<BaseModelConfig>,
    /// }
    ///
    /// impl Model for Provider {
    ///     fn stream(&self, _request: ModelRequest) -> BoxStream<'static, strands::Result<ModelStreamEvent>> {
    ///         Box::pin(futures::stream::empty())
    ///     }
    ///
    ///     fn get_config(&self) -> BaseModelConfig {
    ///         self.config.lock().unwrap().clone()
    ///     }
    ///
    ///     fn update_config(&self, config: BaseModelConfig) {
    ///         self.config.lock().unwrap().merge(config);
    ///     }
    ///
    ///     fn model_info(&self) -> ModelInfo {
    ///         let config = self.get_config();
    ///         let mut info = base_model_info(&config);
    ///         info.merge(ModelInfo {
    ///             capabilities: Some(ModelCapabilities { temporary_messages: Some(true), ..Default::default() }),
    ///             ..ModelInfo::default()
    ///         });
    ///         info.merge(config.model_info.unwrap_or_default());
    ///         info
    ///     }
    /// }
    ///
    /// let model = Provider { config: Mutex::new(BaseModelConfig::default()) };
    /// let capabilities = model.model_info().capabilities.unwrap_or_default();
    /// assert_eq!(capabilities.temporary_messages, Some(true));
    /// ```
    fn model_info(&self) -> ModelInfo {
        let mut config = self.get_config();
        config.model_id = self.model_id();
        base_model_info(&config)
    }

    /// Whether the model keeps conversation state server-side, so the agent sends only the latest
    /// message. Reads `model_info().capabilities.stateful`; `false` when unknown.
    fn stateful(&self) -> bool {
        self.model_info()
            .capabilities
            .and_then(|capabilities| capabilities.stateful)
            .unwrap_or(false)
    }

    /// Counts the input tokens of `messages` (TS `countTokens`). The default is a character
    /// heuristic (characters / 4 for text, / 2 for JSON) that counts only the latest user message's
    /// injected blocks. Providers with a native counting API override it.
    ///
    /// # Errors
    ///
    /// The default never fails; overrides fail when their counting call does.
    async fn count_tokens(&self, messages: &[Message], options: CountTokensOptions) -> Result<u64> {
        Ok(tokens::estimate_tokens_heuristic(messages, &options))
    }

    /// Summarizes `messages` client-side (TS `summarize`): sends them closed by an instruction not
    /// to act on them or call tools, followed by the instructions, and returns the reply's text as
    /// a summary block. A reply cut off at the output limit or a stop sequence still counts; one
    /// that ends for another reason, or holds no text, gives an empty result.
    ///
    /// # Errors
    ///
    /// Fails when the model call fails, unless the call was cancelled.
    async fn summarize(&self, messages: Vec<Message>, options: SummarizeOptions) -> Result<SummarizeResult> {
        summarize_with_stream(self, messages, options).await
    }

    /// The fraction of the context window `input_tokens` fills (above 1.0 means overflow). Falls
    /// back to the default context window, with a one-time warning, when the window is unknown.
    fn estimate_utilization(&self, input_tokens: u64) -> f64 {
        let limit = self
            .model_info()
            .limits
            .and_then(|limits| limits.context_window)
            .filter(|&limit| limit > 0);
        let limit = limit.unwrap_or_else(|| {
            let default = default_context_window_limit();
            warn_once(&format!(
                "contextWindowLimit is not set on the model, using default of {default} for utilization estimate | set contextWindowLimit in your model config for accurate results"
            ));
            default
        });
        input_tokens as f64 / limit as f64
    }
}

/// The default [`Model::model_info`] for `config`: the shared model info for its model id,
/// temporary messages off, its `context_window_limit`, then its `model_info` override. The default
/// [`Model::model_info`] sets that model id to [`Model::model_id`] first.
#[must_use]
pub fn base_model_info(config: &BaseModelConfig) -> ModelInfo {
    let layer = ModelInfo {
        capabilities: Some(ModelCapabilities {
            temporary_messages: Some(false),
            ..ModelCapabilities::default()
        }),
        limits: Some(ModelLimits {
            context_window: config.context_window_limit,
            ..ModelLimits::default()
        }),
    };
    merge_model_info([
        Some(&lookup_model_info(config.model_id.as_deref().unwrap_or_default())),
        Some(&layer),
        config.model_info.as_ref(),
    ])
}

/// Fills `context_window_limit` from the shared model info and the `model_info` override when it
/// isn't set (TS `resolveConfigMetadata`).
#[cfg_attr(not(test), expect(dead_code, reason = "used by the model providers"))]
pub(crate) fn resolve_config_metadata(config: &BaseModelConfig, model_id: &str) -> BaseModelConfig {
    let mut resolved = config.clone();
    if resolved.context_window_limit.is_none() {
        resolved.context_window_limit = configured_limits(config, model_id).context_window;
    }
    resolved
}

/// The `max_tokens` to send: the configured value, else the model's output ceiling from the shared
/// model info with the override applied (TS `resolveMaxTokens`).
#[cfg_attr(not(test), expect(dead_code, reason = "used by the model providers"))]
pub(crate) fn resolve_max_tokens(config: &BaseModelConfig, model_id: &str) -> Option<u64> {
    config
        .max_tokens
        .or_else(|| configured_limits(config, model_id).max_output_tokens)
}

fn configured_limits(config: &BaseModelConfig, model_id: &str) -> ModelLimits {
    merge_model_info([Some(&lookup_model_info(model_id)), config.model_info.as_ref()])
        .limits
        .unwrap_or_default()
}

/// The index of the latest user message (TS `latestUserIndex`, which returns -1 for none).
pub(crate) fn latest_user_index(messages: &[Message]) -> Option<usize> {
    messages.iter().rposition(|message| message.role == Role::User)
}

/// Where an agent gets its model (TS `AgentConfig.model`: a `Model` or a model id).
///
/// A string is a Bedrock model id.
///
/// ```
/// use strands::models::model::ModelSource;
///
/// assert!(matches!(ModelSource::from("global.anthropic.claude-sonnet-4-6"), ModelSource::Id(_)));
/// ```
#[non_exhaustive]
#[derive(Clone)]
pub enum ModelSource {
    /// A model instance.
    Model(Arc<dyn Model>),
    /// A Bedrock model id.
    Id(String),
}

impl fmt::Debug for ModelSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Model(model) => f.debug_tuple("Model").field(&model.model_id()).finish(),
            Self::Id(id) => f.debug_tuple("Id").field(id).finish(),
        }
    }
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
    fn from(id: &str) -> Self {
        Self::Id(id.to_owned())
    }
}

impl From<String> for ModelSource {
    fn from(id: String) -> Self {
        Self::Id(id)
    }
}

open_string_enum! {
    /// Time to live of a prompt cache entry (TS `CacheTTL`). Providers validate it server-side.
    CacheTTL, UnknownCacheTTL {
        FiveMinutes => "5m",
        OneHour => "1h",
    }
}

/// Whether caching skips models that don't support it (TS `CacheConfig.strategy`).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CacheStrategy {
    /// `"auto"`: cache only when the model is known to support it.
    #[default]
    Auto,
    /// `"anthropic"`: cache without that check, for model ids that can't be inspected (an
    /// application inference profile, for example).
    Anthropic,
}

/// One cache section's setting (TS `boolean | CacheTTL`): on or off, or on with its own TTL.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CacheSection {
    /// `true` caches the section with the shared TTL; `false` disables it.
    Enabled(bool),
    /// Caches the section with this TTL.
    Ttl(CacheTTL),
}

impl From<bool> for CacheSection {
    fn from(enabled: bool) -> Self {
        Self::Enabled(enabled)
    }
}

impl From<CacheTTL> for CacheSection {
    fn from(ttl: CacheTTL) -> Self {
        Self::Ttl(ttl)
    }
}

/// The prompt-cache routing key (TS `CacheConfig.cacheKey`, `string | false`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheKey {
    /// Use this key.
    Key(String),
    /// Opt out of routing (`false`).
    Disabled,
}

impl From<&str> for CacheKey {
    fn from(key: &str) -> Self {
        Self::Key(key.to_owned())
    }
}

impl From<String> for CacheKey {
    fn from(key: String) -> Self {
        Self::Key(key)
    }
}

impl Serialize for CacheKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Key(key) => serializer.serialize_str(key),
            Self::Disabled => serializer.serialize_bool(false),
        }
    }
}

impl<'de> Deserialize<'de> for CacheKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        match JsonValue::deserialize(deserializer)? {
            JsonValue::String(key) => Ok(Self::Key(key)),
            JsonValue::Bool(false) => Ok(Self::Disabled),
            _ => Err(serde::de::Error::custom("cacheKey must be a string or false")),
        }
    }
}

/// Prompt caching configuration (TS `CacheConfig`). Providers use the fields they support.
///
/// ```
/// use strands::models::model::{CacheConfig, CacheSection, CacheTTL};
///
/// let config: CacheConfig = serde_json::from_value(serde_json::json!({ "ttl": "1h", "messagesTTL": false })).unwrap();
/// assert_eq!(config.ttl, Some(CacheTTL::OneHour));
/// assert_eq!(config.messages_ttl, Some(CacheSection::Enabled(false)));
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, bon::Builder)]
#[serde(rename_all = "camelCase")]
pub struct CacheConfig {
    /// Whether to skip caching for models that don't support it; `auto` when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy: Option<CacheStrategy>,
    /// TTL for every cache point, overridden by a per-section TTL; the provider default when
    /// `None`. It also fills in for a hand-placed system prompt cache point without one.
    #[builder(into)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl: Option<CacheTTL>,
    /// Cache the tool definitions; on when `None`.
    #[builder(into)]
    #[serde(default, rename = "toolsTTL", skip_serializing_if = "Option::is_none")]
    pub tools_ttl: Option<CacheSection>,
    /// Cache the system prompt by injecting a cache point at its end; on when `None`.
    #[builder(into)]
    #[serde(default, rename = "systemPromptTTL", skip_serializing_if = "Option::is_none")]
    pub system_prompt_ttl: Option<CacheSection>,
    /// Cache the conversation prefix at the last user message; on when `None`.
    #[builder(into)]
    #[serde(default, rename = "messagesTTL", skip_serializing_if = "Option::is_none")]
    pub messages_ttl: Option<CacheSection>,
    /// Identity a prompt-cache-routing provider (`OpenAI`) uses as its cache key. When `None` it is
    /// derived per request as `strands-<sessionId>` if the agent has a session manager.
    #[builder(into)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_key: Option<CacheKey>,
}

/// A cache section resolved to what a provider emits (TS `ResolvedCacheSection`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedCacheSection {
    /// Whether the section carries a cache point.
    pub(crate) enabled: bool,
    /// The TTL to emit, if any applies.
    pub(crate) ttl: Option<CacheTTL>,
}

/// Resolves whether a cache section is enabled and its TTL: the section's own TTL, then the
/// fallbacks, most specific first; an empty TTL counts as unset (TS `resolveCacheSection`).
#[cfg_attr(not(test), expect(dead_code, reason = "used by the model providers"))]
pub(crate) fn resolve_cache_section(
    section: Option<&CacheSection>,
    ttl_fallbacks: &[Option<&CacheTTL>],
) -> ResolvedCacheSection {
    let section_ttl = match section {
        Some(CacheSection::Ttl(ttl)) => Some(ttl),
        _ => None,
    };
    let ttl = std::iter::once(section_ttl)
        .chain(ttl_fallbacks.iter().copied())
        .flatten()
        .find(|ttl| !ttl.as_str().is_empty())
        .cloned();
    ResolvedCacheSection {
        enabled: section != Some(&CacheSection::Enabled(false)),
        ttl,
    }
}

#[cfg(test)]
mod tests;
