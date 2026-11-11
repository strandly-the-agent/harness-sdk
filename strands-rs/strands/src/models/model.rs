//! The model abstraction (TS `models/model.ts`).

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};

use crate::agent::AgentMetadata;
use crate::cancel::CancelSignal;
use crate::error::Result;
use crate::logging::warn_once;
use crate::models::model_info::{
    ModelCapabilities, ModelInfo, ModelLimits, default_context_window_limit, lookup_model_info,
    merge_model_info,
};
use crate::models::streaming::{ModelMetadataEvent, ModelStreamEvent, Usage};
use crate::models::summarize::summarize_with_stream;
use crate::state_store::StateStore;
use crate::tools::{ToolChoice, ToolSpec};
use crate::types::json::JsonValue;
use crate::types::{ContentBlock, Message, Role, StopReason, SystemPrompt};

mod aggregate;
mod tokens;

/// A boxed, sendable stream.
pub type BoxStream<'a, T> = Pin<Box<dyn Stream<Item = T> + Send + 'a>>;

/// A model provider (TS `Model`).
///
/// Implement [`stream`](Model::stream), [`get_config`](Model::get_config) and
/// [`update_config`](Model::update_config); everything else has a default that follows TS.
/// Models are shared as `Arc<dyn Model>`, so configuration sits behind interior locking.
///
/// ```
/// use std::sync::{Mutex, PoisonError};
///
/// use futures::stream;
/// use strands::Result;
/// use strands::models::{
///     BaseModelConfig, BoxStream, Model, ModelContentBlockDeltaEvent, ModelContentBlockStartEvent,
///     ModelContentBlockStopEvent, ModelMessageStartEvent, ModelMessageStopEvent, ModelRequest,
///     ModelStreamEvent, TextDelta,
/// };
/// use strands::types::Role;
/// use strands::StopReason;
///
/// struct Echo {
///     config: Mutex<BaseModelConfig>,
/// }
///
/// impl Model for Echo {
///     fn stream(&self, _request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>> {
///         let events = vec![
///             ModelMessageStartEvent::new(Role::Assistant).into(),
///             ModelContentBlockStartEvent::new().into(),
///             ModelContentBlockDeltaEvent::new(TextDelta::new("hi")).into(),
///             ModelContentBlockStopEvent::new().into(),
///             ModelMessageStopEvent::new(StopReason::EndTurn).into(),
///         ];
///         Box::pin(stream::iter(events.into_iter().map(Ok)))
///     }
///
///     fn get_config(&self) -> BaseModelConfig {
///         self.config.lock().unwrap_or_else(PoisonError::into_inner).clone()
///     }
///
///     fn update_config(&self, config: BaseModelConfig) {
///         let mut current = self.config.lock().unwrap_or_else(PoisonError::into_inner);
///         *current = BaseModelConfig { model_id: config.model_id.or(current.model_id.take()), ..config };
///     }
/// }
/// ```
#[async_trait]
pub trait Model: Send + Sync + 'static {
    /// Streams the model's response to `request`.
    ///
    /// The stream is `'static` so it can be spawned: clone what it needs out of `&self`.
    fn stream(&self, request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>>;

    /// Streams the response with each completed content block and, last, the aggregated result
    /// (TS `streamAggregated`).
    ///
    /// Errors outside the `ModelError` family are wrapped in a `ModelError`. A `maxTokens` stop
    /// ends with [`Error::MaxTokens`](crate::Error::MaxTokens) carrying the partial message; tool
    /// input that isn't valid JSON and a stream that ends without a message stop are `ModelError`s.
    fn stream_aggregated(&self, request: ModelRequest) -> BoxStream<'static, Result<AggregatedEvent>> {
        aggregate::aggregate(self.stream(request))
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
    /// ones winning: the shared model info for the model id, temporary messages off, the
    /// `context_window_limit` config value, and the `model_info` config override.
    ///
    /// A provider that knows better puts its facts over this default and the user's
    /// `get_config().model_info` over those, so the override still wins. Don't call
    /// [`stateful`](Model::stateful) from an override; it reads this method.
    fn model_info(&self) -> ModelInfo {
        let config = self.get_config();
        let defaults = ModelInfo {
            capabilities: Some(ModelCapabilities {
                temporary_messages: Some(false),
                ..Default::default()
            }),
            limits: Some(ModelLimits {
                context_window: config.context_window_limit,
                ..Default::default()
            }),
        };
        merge_model_info([
            Some(&lookup_model_info(config.model_id.as_deref().unwrap_or_default())),
            Some(&defaults),
            config.model_info.as_ref(),
        ])
    }

    /// Whether the provider keeps conversation state server-side, so the agent sends only the
    /// latest message. Reads `model_info().capabilities.stateful`; `false` when unknown.
    fn stateful(&self) -> bool {
        self.model_info().capabilities().stateful.unwrap_or(false)
    }

    /// Counts the input tokens of a request before sending it.
    ///
    /// The default estimates from characters (four per token for text, two for JSON) and counts
    /// injected blocks only in the latest user message. Providers with a native counting API
    /// override it and fall back to this estimate on failure.
    ///
    /// # Errors
    ///
    /// The default never fails; a provider's native count may.
    async fn count_tokens(&self, messages: &[Message], options: CountTokensOptions) -> Result<u64> {
        Ok(tokens::estimate_tokens_heuristic(messages, &options))
    }

    /// Summarizes `messages`, a request as the caller would send it to [`stream`](Model::stream).
    ///
    /// The default sends the request closed by an instruction not to act on it or call tools,
    /// then the instructions (or default ones), so a caller replaying its own conversation shares
    /// its prompt cache. The summary is the reply's text, including a reply cut off at the output
    /// limit or a stop sequence; a reply that ends for another reason or holds no text gives an
    /// empty result. The summary is one summary block with no `provider`. Providers that summarize
    /// server-side override this and set `provider`.
    ///
    /// # Errors
    ///
    /// Model errors other than a `maxTokens` stop, unless the call was cancelled.
    async fn summarize(&self, messages: Vec<Message>, options: SummarizeOptions) -> Result<SummarizeResult> {
        summarize_with_stream(self, messages, options).await
    }

    /// The share of the context window `input_tokens` fills. Without a known context window it
    /// assumes the default (200,000) and warns once.
    fn estimate_utilization(&self, input_tokens: u64) -> f64 {
        let context_window = self.model_info().context_window().filter(|limit| *limit > 0).unwrap_or_else(|| {
            let default = default_context_window_limit();
            warn_once(&format!(
                "contextWindowLimit is not set on the model, using default of {default} for utilization estimate | set contextWindowLimit in your model config for accurate results"
            ));
            default
        });
        input_tokens as f64 / context_window as f64
    }
}

/// Configuration every model provider understands (TS `BaseModelConfig`).
///
/// Providers extend it with their own settings; [`Model::get_config`] and
/// [`Model::update_config`] read and shallow-merge this common part.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, bon::Builder)]
#[serde(rename_all = "camelCase")]
pub struct BaseModelConfig {
    /// The model identifier in the provider's catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(into)]
    pub model_id: Option<String>,
    /// Maximum tokens to generate per response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    /// Sampling temperature; see the provider for the valid range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// Nucleus sampling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    /// Context window size in tokens. Shorthand for `model_info.limits.context_window`, which
    /// wins when both are set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window_limit: Option<u64>,
    /// Overrides for what the SDK knows about the model; they win over the shared model info and
    /// the provider. Updating it replaces the previous override entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_info: Option<ModelInfo>,
}

impl BaseModelConfig {
    /// Overwrites each field `update` sets, keeping the rest (TS `{ ...config, ...update }`).
    pub(crate) fn merge(&mut self, update: BaseModelConfig) {
        let BaseModelConfig {
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

/// Fills `context_window_limit` from the model info when it's unset (TS `resolveConfigMetadata`).
#[cfg_attr(not(test), expect(dead_code, reason = "used by model providers"))]
pub(crate) fn resolve_config_metadata(mut config: BaseModelConfig, model_id: &str) -> BaseModelConfig {
    if config.context_window_limit.is_none() {
        config.context_window_limit =
            merge_model_info([Some(&lookup_model_info(model_id)), config.model_info.as_ref()])
                .context_window();
    }
    config
}

/// `max_tokens` from the config, else the model's known output limit (TS `resolveMaxTokens`).
#[cfg_attr(not(test), expect(dead_code, reason = "used by model providers"))]
pub(crate) fn resolve_max_tokens(config: &BaseModelConfig, model_id: &str) -> Option<u64> {
    config.max_tokens.or_else(|| {
        merge_model_info([Some(&lookup_model_info(model_id)), config.model_info.as_ref()]).max_output_tokens()
    })
}

/// A prompt-cache time to live, such as `5m` or `1h` (TS `CacheTTL`). Providers accept others.
pub type CacheTtl = String;

/// Whether to cache models that may not support it (TS `CacheConfig.strategy`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum CacheStrategy {
    /// Cache only when the model is known to support it.
    #[default]
    Auto,
    /// Cache without checking, for model ids the SDK can't inspect (an application inference
    /// profile, for example).
    Anthropic,
}

/// One cacheable section of a request: on, off, or on with its own TTL (TS `boolean | CacheTTL`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum CacheSection {
    /// Caching on (`true`, using the shared TTL) or off (`false`).
    Enabled(bool),
    /// Caching on with this TTL.
    Ttl(CacheTtl),
}

impl From<bool> for CacheSection {
    fn from(enabled: bool) -> Self {
        Self::Enabled(enabled)
    }
}

impl From<&str> for CacheSection {
    fn from(ttl: &str) -> Self {
        Self::Ttl(ttl.to_owned())
    }
}

impl From<String> for CacheSection {
    fn from(ttl: String) -> Self {
        Self::Ttl(ttl)
    }
}

/// The prompt-cache routing key sent to providers that route by key (TS `cacheKey: string | false`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CacheKey {
    /// Use this key.
    Key(String),
    /// Send no key (TS `false`).
    Off,
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

/// Prompt caching for a provider (TS `CacheConfig`).
///
/// ```
/// use strands::models::{CacheConfig, CacheSection};
///
/// let cache = CacheConfig {
///     ttl: Some("1h".into()),
///     messages_ttl: Some(CacheSection::from(false)),
///     ..Default::default()
/// };
/// assert_eq!(cache.tools_ttl, None);
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, bon::Builder)]
pub struct CacheConfig {
    /// Whether to skip caching for models that don't support it. Defaults to
    /// [`CacheStrategy::Auto`].
    pub strategy: Option<CacheStrategy>,
    /// TTL for every cache point, overridden by a section's own TTL; the provider default when
    /// unset. It also fills in for a cache point placed by hand in the system prompt without a
    /// TTL, so one value keeps Bedrock's non-increasing TTL rule satisfied.
    #[builder(into)]
    pub ttl: Option<CacheTtl>,
    /// Cache the tool definitions. Defaults to on.
    #[builder(into)]
    pub tools_ttl: Option<CacheSection>,
    /// Cache the system prompt by adding a cache point at its end. Defaults to on.
    #[builder(into)]
    pub system_prompt_ttl: Option<CacheSection>,
    /// Cache the conversation prefix at the last user message. Defaults to on.
    #[builder(into)]
    pub messages_ttl: Option<CacheSection>,
    /// Routing key for providers that route caches by key (OpenAI). Unset, it is derived per
    /// request as `strands-<sessionId>` when the agent has a session manager; [`CacheKey::Off`]
    /// opts out.
    #[builder(into)]
    pub cache_key: Option<CacheKey>,
}

/// Whether a request section carries a cache point, and with which TTL (TS `ResolvedCacheSection`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedCacheSection {
    pub(crate) enabled: bool,
    pub(crate) ttl: Option<CacheTtl>,
}

/// Resolves a section: on unless `false`; its TTL is its own, else the first non-empty fallback
/// (TS `resolveCacheSection`).
#[cfg_attr(not(test), expect(dead_code, reason = "used by model providers"))]
pub(crate) fn resolve_cache_section(
    section: Option<&CacheSection>,
    ttl_fallbacks: &[Option<&str>],
) -> ResolvedCacheSection {
    let enabled = !matches!(section, Some(CacheSection::Enabled(false)));
    let own = match section {
        Some(CacheSection::Ttl(ttl)) => Some(ttl.as_str()),
        _ => None,
    };
    let ttl = std::iter::once(own)
        .chain(ttl_fallbacks.iter().copied())
        .flatten()
        .find(|ttl| !ttl.is_empty())
        .map(str::to_owned);
    ResolvedCacheSection { enabled, ttl }
}

/// One call to [`Model::stream`] (TS `stream(messages, options)`).
///
/// ```
/// use strands::models::ModelRequest;
/// use strands::{ContentBlock, Message};
/// use strands::types::Role;
///
/// let request = ModelRequest {
///     messages: vec![Message::new(Role::User, vec![ContentBlock::from("Hi")])],
///     system_prompt: Some("Be brief.".into()),
///     ..Default::default()
/// };
/// assert!(request.tool_specs.is_empty());
/// ```
#[derive(Debug, Clone, Default, bon::Builder)]
pub struct ModelRequest {
    /// The conversation.
    #[builder(default)]
    pub messages: Vec<Message>,
    /// The system prompt.
    #[builder(into)]
    pub system_prompt: Option<SystemPrompt>,
    /// Tools the model may call.
    #[builder(default)]
    pub tool_specs: Vec<ToolSpec>,
    /// How the model chooses tools.
    pub tool_choice: Option<ToolChoice>,
    /// State for providers that keep conversation state server-side (for example a response id).
    /// The agent passes a snapshot and keeps the provider's writes only when the call succeeds.
    #[builder(default)]
    pub model_state: StateStore,
    /// How many trailing blocks of the last user message are rebuilt on every call.
    pub dynamic_trailing_blocks: Option<usize>,
    /// The invoking agent, supplied per request.
    pub agent_metadata: Option<AgentMetadata>,
    /// JSON schema this call must produce, set on the structured output retry so the provider can
    /// choose how to enforce it.
    pub structured_output_schema: Option<JsonValue>,
    /// Aborts the in-flight request; support depends on the provider.
    pub cancel_signal: Option<CancelSignal>,
}

/// Options for [`Model::count_tokens`] (TS `CountTokensOptions`).
#[derive(Debug, Clone, Default, PartialEq, bon::Builder)]
pub struct CountTokensOptions {
    /// The system prompt to count.
    #[builder(into)]
    pub system_prompt: Option<SystemPrompt>,
    /// Tool specs to count.
    #[builder(default)]
    pub tool_specs: Vec<ToolSpec>,
}

/// Options for [`Model::summarize`] (TS `SummarizeOptions`).
#[derive(Debug, Clone, Default, bon::Builder)]
pub struct SummarizeOptions {
    /// The request's system prompt.
    #[builder(into)]
    pub system_prompt: Option<SystemPrompt>,
    /// The request's tool specs.
    #[builder(default)]
    pub tool_specs: Vec<ToolSpec>,
    /// Instructions replacing the default ones; `None` or empty keeps them. The instruction not
    /// to act on the request or call tools is always sent.
    #[builder(into)]
    pub instructions: Option<String>,
    /// Aborts the call.
    pub cancel_signal: Option<CancelSignal>,
    /// The invoking agent, as passed to [`Model::stream`].
    pub agent_metadata: Option<AgentMetadata>,
}

/// The result of [`Model::summarize`] (TS `SummarizeResult`).
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct SummarizeResult {
    /// The summary as content blocks: one summary block, or none when the reply held no summary.
    pub content: Vec<ContentBlock>,
    /// Token usage of the call.
    pub usage: Option<Usage>,
}

impl SummarizeResult {
    /// A result holding `content`.
    pub fn new(content: Vec<ContentBlock>) -> Self {
        Self { content, usage: None }
    }

    /// Sets the usage.
    #[must_use]
    pub fn with_usage(mut self, usage: Usage) -> Self {
        self.usage = Some(usage);
        self
    }
}

/// Text that replaces the last user message after a guardrail redacted it (TS `Redaction`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Redaction {
    /// The replacement text.
    pub user_message: String,
}

impl Redaction {
    /// A redaction replacing the user message with `user_message`.
    pub fn new(user_message: impl Into<String>) -> Self {
        Self {
            user_message: user_message.into(),
        }
    }
}

/// The outcome of [`Model::stream_aggregated`] (TS `StreamAggregatedResult`).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct StreamAggregatedResult {
    /// The complete message.
    pub message: Message,
    /// Why the model stopped.
    pub stop_reason: StopReason,
    /// Usage, metrics and trace of the call.
    pub metadata: Option<ModelMetadataEvent>,
    /// Set when a guardrail redacted the input. Output redaction replaces `message` instead.
    pub redaction: Option<Redaction>,
}

impl StreamAggregatedResult {
    /// A result for `message` and `stop_reason`.
    pub fn new(message: Message, stop_reason: impl Into<StopReason>) -> Self {
        Self {
            message,
            stop_reason: stop_reason.into(),
            metadata: None,
            redaction: None,
        }
    }

    /// Sets the metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: ModelMetadataEvent) -> Self {
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

/// An item of [`Model::stream_aggregated`]: every stream event, each completed content block,
/// and finally the result (TS `streamAggregated` yields events and blocks and returns the result).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum AggregatedEvent {
    /// A raw stream event, passed through as it arrives.
    ModelStreamEvent(ModelStreamEvent),
    /// A content block, once complete.
    ContentBlock(ContentBlock),
    /// The aggregated result; always the last item of a successful stream.
    Done(Box<StreamAggregatedResult>),
}

/// Index of the latest user message (TS `latestUserIndex`, which returns `-1` for none).
pub(crate) fn latest_user_index(messages: &[Message]) -> Option<usize> {
    messages.iter().rposition(|message| message.role == Role::User)
}

#[cfg(test)]
mod tests;
