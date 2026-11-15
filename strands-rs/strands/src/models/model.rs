//! The model abstraction: the [`Model`] trait, its request, config and cache types (TS `models/model.ts`).

use std::pin::Pin;

use async_trait::async_trait;
use futures::{Stream, StreamExt};

use crate::agent::agent_metadata::AgentMetadata;
use crate::cancel::CancelSignal;
use crate::error::{Error, MaxTokensError, ModelError, Result};
use crate::hooks::events::Redaction;
use crate::logging::warn_once::warn_once;
use crate::models::model_info::{
    DEFAULT_CONTEXT_WINDOW_LIMIT, ModelCapabilities, ModelInfo, ModelLimits, lookup_model, lookup_model_info,
    merge_model_info,
};
use crate::models::streaming::{ContentBlockDelta, ContentBlockStart, ModelMetadataEvent, ModelStreamEvent};
use crate::models::summarize::{SummarizeOptions, SummarizeResult, summarize_with_stream};
use crate::state_store::StateStore;
use crate::types::citations::{Citation, CitationGeneratedContent, CitationsBlock};
use crate::types::json::{JsonValue, from_json_str, to_js_string};
use crate::types::messages::{
    ContentBlock, Message, MessageMetadata, ReasoningBlock, Role, StopReason, SystemContentBlock,
    SystemPrompt, TextBlock, ToolResultContent, ToolUseBlock,
};
use crate::types::tools::{ToolChoice, ToolSpec};

/// A boxed, `Send` stream.
pub type BoxStream<'a, T> = Pin<Box<dyn Stream<Item = T> + Send + 'a>>;

/// Configuration every model provider shares (TS `BaseModelConfig`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, bon::Builder)]
#[builder(on(String, into))]
pub struct BaseModelConfig {
    /// The provider's model id.
    pub model_id: Option<String>,
    /// The largest number of tokens to generate.
    pub max_tokens: Option<u64>,
    /// Sampling temperature.
    pub temperature: Option<f64>,
    /// Nucleus sampling probability.
    pub top_p: Option<f64>,
    /// The context window in tokens; overrides the model-info table.
    pub context_window_limit: Option<u64>,
    /// Model facts merged over the shared model-info table.
    pub model_info: Option<ModelInfo>,
}

impl BaseModelConfig {
    /// Shallow-merges `update` into `self`: every field `update` sets replaces the current value.
    ///
    /// A `None` field keeps the current value, so a merge can't clear a field (TS unsets it when
    /// the update holds `undefined`); replace the whole config to clear one.
    pub fn merge(&mut self, update: BaseModelConfig) {
        let BaseModelConfig {
            model_id,
            max_tokens,
            temperature,
            top_p,
            context_window_limit,
            model_info,
        } = update;
        replace_if_set(&mut self.model_id, model_id);
        replace_if_set(&mut self.max_tokens, max_tokens);
        replace_if_set(&mut self.temperature, temperature);
        replace_if_set(&mut self.top_p, top_p);
        replace_if_set(&mut self.context_window_limit, context_window_limit);
        replace_if_set(&mut self.model_info, model_info);
    }
}

fn replace_if_set<T>(slot: &mut Option<T>, value: Option<T>) {
    if value.is_some() {
        *slot = value;
    }
}

/// One model call: the conversation and the options for it (TS `stream(messages, StreamOptions)`).
///
/// ```
/// use strands::models::model::ModelRequest;
/// use strands::types::messages::{Message, Role, TextBlock};
///
/// let request = ModelRequest {
///     messages: vec![Message::new(Role::User, vec![TextBlock::new("Hello").into()])],
///     ..Default::default()
/// };
/// assert!(request.tool_specs.is_empty());
/// ```
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
pub struct ModelRequest {
    /// The conversation to send.
    #[builder(default)]
    pub messages: Vec<Message>,
    /// The system prompt.
    #[builder(into)]
    pub system_prompt: Option<SystemPrompt>,
    /// The tools the model may call.
    #[builder(default)]
    pub tool_specs: Vec<ToolSpec>,
    /// How the model must choose tools.
    pub tool_choice: Option<ToolChoice>,
    /// Provider state for this call (TS `modelState`). The agent passes a snapshot and keeps the
    /// provider's writes only when the call succeeds.
    #[builder(default)]
    pub model_state: StateStore,
    /// How many trailing blocks of the last message change between calls, so cache points stay
    /// ahead of them.
    pub dynamic_trailing_blocks: Option<usize>,
    /// Details of the calling agent.
    pub agent_metadata: Option<AgentMetadata>,
    /// JSON schema for native structured output, when the provider supports it.
    pub structured_output_schema: Option<JsonValue>,
    /// Cancels the call.
    pub cancel_signal: Option<CancelSignal>,
}

/// Options for [`Model::count_tokens`] (TS `CountTokensOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, bon::Builder)]
pub struct CountTokensOptions {
    /// The system prompt to count.
    #[builder(into)]
    pub system_prompt: Option<SystemPrompt>,
    /// The tool specs to count.
    #[builder(default)]
    pub tool_specs: Vec<ToolSpec>,
}

open_enum! {
    /// A prompt cache time-to-live (TS `CacheTTL`); providers accept other values too.
    pub enum CacheTtl / UnknownCacheTtl {
        /// Five minutes (`5m`).
        FiveMinutes => "5m",
        /// One hour (`1h`).
        OneHour => "1h",
    }
}

/// How a provider places cache points (TS `CacheConfig.strategy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CacheStrategy {
    /// Cache where the model is known to support it (`auto`).
    Auto,
    /// Anthropic-style cache points regardless of the model id (`anthropic`).
    Anthropic,
}

/// Whether one request section is cached, and for how long (TS `boolean | CacheTTL`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CacheSection {
    /// Cache the section (`true`) or not (`false`), with the config's TTL.
    Enabled(bool),
    /// Cache the section with this TTL.
    Ttl(CacheTtl),
}

/// The prompt cache key (TS `string | false`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CacheKey {
    /// Send this key.
    Key(String),
    /// Send no key (`false`).
    Disabled,
}

/// Prompt caching configuration shared by providers (TS `CacheConfig`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, Eq, bon::Builder)]
pub struct CacheConfig {
    /// How cache points are placed.
    pub strategy: Option<CacheStrategy>,
    /// The default TTL for every section.
    pub ttl: Option<CacheTtl>,
    /// Caching of the tool specs (TS `toolsTTL`).
    pub tools_ttl: Option<CacheSection>,
    /// Caching of the system prompt (TS `systemPromptTTL`).
    pub system_prompt_ttl: Option<CacheSection>,
    /// Caching of the messages (TS `messagesTTL`).
    pub messages_ttl: Option<CacheSection>,
    /// The prompt cache key, for providers that take one.
    pub cache_key: Option<CacheKey>,
}

/// A cache section resolved against its fallbacks (TS `ResolvedCacheSection`).
#[allow(dead_code, reason = "used by model providers")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedCacheSection {
    pub(crate) enabled: bool,
    pub(crate) ttl: Option<CacheTtl>,
}

/// Resolves a section setting: enabled unless `false`; the first TTL of the section's own and the
/// fallbacks (TS `resolveCacheSection`).
#[allow(dead_code, reason = "used by model providers")]
pub(crate) fn resolve_cache_section(
    section: Option<&CacheSection>,
    ttl_fallbacks: &[Option<&CacheTtl>],
) -> ResolvedCacheSection {
    let enabled = section != Some(&CacheSection::Enabled(false));
    let own = match section {
        Some(CacheSection::Ttl(ttl)) => Some(ttl),
        _ => None,
    };
    let ttl = std::iter::once(own)
        .chain(ttl_fallbacks.iter().copied())
        .flatten()
        .find(|ttl| !ttl.as_str().is_empty())
        .cloned();
    ResolvedCacheSection { enabled, ttl }
}

/// The context window for `model_id` when the config doesn't set one (TS `resolveConfigMetadata`).
#[allow(dead_code, reason = "used by model providers")]
pub(crate) fn resolve_config_metadata(config: &mut BaseModelConfig, model_id: &str) {
    if config.context_window_limit.is_some() {
        return;
    }
    let looked_up = lookup_model_info(model_id);
    config.context_window_limit =
        merge_model_info([Some(&looked_up), config.model_info.as_ref()]).context_window();
}

/// The configured max tokens, else the model's output limit from the shared table (TS `resolveMaxTokens`).
#[allow(dead_code, reason = "used by model providers")]
pub(crate) fn resolve_max_tokens(config: &BaseModelConfig, model_id: &str) -> Option<u64> {
    if config.max_tokens.is_some() {
        return config.max_tokens;
    }
    let looked_up = lookup_model_info(model_id);
    merge_model_info([Some(&looked_up), config.model_info.as_ref()]).max_output_tokens()
}

/// The index of the last user message (TS `latestUserIndex`, which returns `-1` for none).
pub(crate) fn latest_user_index(messages: &[Message]) -> Option<usize> {
    messages.iter().rposition(|message| message.role() == Role::User)
}

/// The default [`Model::model_info`] (TS `super.modelInfo`), in layers where later ones win: the
/// shared model-info entry for [`Model::model_id`], temporary messages off, the config's
/// `context_window_limit`, and the config's `model_info` override.
///
/// A provider that knows more overrides `model_info` and layers its facts over this, then the
/// config's `model_info` again so the user's override still wins:
/// `merge_model_info([Some(&base_model_info(self)), Some(&facts), config.model_info.as_ref()])`.
/// Don't call [`Model::stateful`] from the override; it reads `model_info`.
pub fn base_model_info<M: Model + ?Sized>(model: &M) -> ModelInfo {
    let config = model.get_config();
    let known = ModelInfo {
        capabilities: Some(ModelCapabilities {
            temporary_messages: Some(false),
            ..Default::default()
        }),
        limits: Some(ModelLimits {
            context_window: config.context_window_limit,
            max_output_tokens: None,
        }),
    };
    let shared = lookup_model(model.model_id().as_deref().unwrap_or_default());
    merge_model_info([shared, Some(&known), config.model_info.as_ref()])
}

/// An item of [`Model::stream_aggregated`] (TS `streamAggregated` yields events and blocks, then
/// returns the result).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum AggregatedEvent {
    /// A provider event, passed through as it arrives.
    ModelStreamEvent(ModelStreamEvent),
    /// A content block, complete at its stop event.
    ContentBlock(ContentBlock),
    /// The aggregated result; always the last item of a successful stream.
    Done(Box<StreamAggregatedResult>),
}

/// The aggregated outcome of a model call (TS `StreamAggregatedResult`).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct StreamAggregatedResult {
    message: Message,
    stop_reason: StopReason,
    metadata: Option<ModelMetadataEvent>,
    redaction: Option<Redaction>,
}

impl StreamAggregatedResult {
    /// A result with this message and stop reason.
    pub fn new(message: Message, stop_reason: impl Into<StopReason>) -> Self {
        Self {
            message,
            stop_reason: stop_reason.into(),
            metadata: None,
            redaction: None,
        }
    }

    /// Sets the last metadata event of the stream.
    #[must_use]
    pub fn with_metadata(mut self, metadata: ModelMetadataEvent) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Sets the input redaction a guardrail requested.
    #[must_use]
    pub fn with_redaction(mut self, redaction: Redaction) -> Self {
        self.redaction = Some(redaction);
        self
    }

    /// The assistant message.
    pub fn message(&self) -> &Message {
        &self.message
    }

    /// Takes the assistant message.
    pub fn into_message(self) -> Message {
        self.message
    }

    /// Why the model stopped.
    pub fn stop_reason(&self) -> &StopReason {
        &self.stop_reason
    }

    /// The last metadata event of the stream.
    pub fn metadata(&self) -> Option<&ModelMetadataEvent> {
        self.metadata.as_ref()
    }

    /// The input redaction a guardrail requested.
    pub fn redaction(&self) -> Option<&Redaction> {
        self.redaction.as_ref()
    }
}

/// A model provider (TS `Model`).
///
/// Implement [`stream`](Model::stream), [`get_config`](Model::get_config) and
/// [`update_config`](Model::update_config); everything else has a default. `stream` returns a
/// `'static` stream, so clone what it needs (a config snapshot, a client `Arc`) out of `&self`.
///
/// ```
/// use std::sync::Mutex;
///
/// use futures::stream;
/// use strands::models::model::{BaseModelConfig, BoxStream, CountTokensOptions, Model, ModelRequest};
/// use strands::models::streaming::{
///     ModelContentBlockDeltaEvent, ModelContentBlockStartEvent, ModelContentBlockStopEvent,
///     ModelMessageStartEvent, ModelMessageStopEvent, ModelStreamEvent, TextDelta,
/// };
/// use strands::types::messages::Role;
/// use strands::{Message, Result, StopReason, async_trait};
///
/// struct Echo {
///     config: Mutex<BaseModelConfig>,
/// }
///
/// // The async methods (`count_tokens`, `summarize`) need `#[async_trait]` on the impl.
/// #[async_trait]
/// impl Model for Echo {
///     fn stream(&self, _request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>> {
///         let events: Vec<ModelStreamEvent> = vec![
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
///         self.config.lock().unwrap_or_else(|e| e.into_inner()).clone()
///     }
///
///     fn update_config(&self, config: BaseModelConfig) {
///         self.config.lock().unwrap_or_else(|e| e.into_inner()).merge(config);
///     }
///
///     async fn count_tokens(&self, messages: &[Message], _options: CountTokensOptions) -> Result<u64> {
///         Ok(messages.len() as u64)
///     }
/// }
/// ```
#[async_trait]
pub trait Model: Send + Sync + 'static {
    /// Streams the provider's events for one call (TS `stream`).
    fn stream(&self, request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>>;

    /// Streams the call's events, each content block once complete, then the aggregated result
    /// (TS `streamAggregated`).
    ///
    /// The stream ends with an error for a `maxTokens` stop ([`MaxTokensError`] with the partial
    /// message), for unparseable tool input JSON, and when the provider stream ends without a
    /// message stop. Errors outside the model error family are wrapped in [`ModelError`].
    fn stream_aggregated(&self, request: ModelRequest) -> BoxStream<'static, Result<AggregatedEvent>> {
        let mut events = self.stream(request);
        Box::pin(async_stream::stream! {
            let mut aggregator = Aggregator::default();
            while let Some(event) = events.next().await {
                let event = match event {
                    Ok(event) => event,
                    Err(error) => {
                        yield Err(into_model_error(error));
                        return;
                    }
                };
                let block = aggregator.on_event(&event);
                yield Ok(AggregatedEvent::ModelStreamEvent(event));
                if let Some(block) = block {
                    yield Ok(AggregatedEvent::ContentBlock(block));
                }
            }
            yield aggregator.finish().map(|result| AggregatedEvent::Done(Box::new(result)));
        })
    }

    /// The provider's current config (TS `getConfig`).
    fn get_config(&self) -> BaseModelConfig;

    /// Shallow-merges `config` into the provider's config (TS `updateConfig`), as
    /// [`BaseModelConfig::merge`] does: a `None` field keeps the current value.
    fn update_config(&self, config: BaseModelConfig);

    /// The configured model id (TS `modelId`).
    fn model_id(&self) -> Option<String> {
        self.get_config().model_id
    }

    /// What the SDK knows about this model (TS `modelInfo`), resolved on each call.
    ///
    /// The default is [`base_model_info`]: the shared model-info entry for
    /// [`model_id`](Model::model_id), then the config's context window and `model_info`. A
    /// provider with facts of its own overrides this and layers them with [`merge_model_info`].
    fn model_info(&self) -> ModelInfo {
        base_model_info(self)
    }

    /// Whether the provider keeps the conversation server-side (TS `stateful`); `false` when unknown.
    fn stateful(&self) -> bool {
        self.model_info()
            .capabilities
            .and_then(|capabilities| capabilities.stateful)
            .unwrap_or(false)
    }

    /// Estimates the tokens of `messages`, the system prompt and the tool specs (TS `countTokens`).
    ///
    /// The default is a heuristic: text at four characters per token, JSON at two. Only the
    /// latest user message's injected blocks count, since earlier ones aren't sent again.
    ///
    /// # Errors
    ///
    /// Providers that call a token counting API return its errors; the default never fails.
    async fn count_tokens(&self, messages: &[Message], options: CountTokensOptions) -> Result<u64> {
        Ok(estimate_tokens_heuristic(messages, &options))
    }

    /// Summarizes `messages` for a fresh context window (TS `summarize`).
    ///
    /// The default asks this model for a handoff summary, closing the request with the summary
    /// instructions. A reply that didn't finish (cancelled, a guardrail, tool use) yields no content.
    ///
    /// # Errors
    ///
    /// Returns the model call's error, except a `maxTokens` stop (the partial summary is kept) and
    /// errors after a cancel.
    async fn summarize(&self, messages: Vec<Message>, options: SummarizeOptions) -> Result<SummarizeResult> {
        summarize_with_stream(self, messages, options).await
    }

    /// The share of the context window `input_tokens` fills (TS `estimateUtilization`); above 1.0
    /// when it overflows. Without a known context window it uses the default of 200,000 and warns once.
    fn estimate_utilization(&self, input_tokens: u64) -> f64 {
        let limit = match self.model_info().context_window() {
            Some(limit) if limit > 0 => limit,
            _ => {
                warn_once(&format!(
                    "contextWindowLimit is not set on the model, using default of {DEFAULT_CONTEXT_WINDOW_LIMIT} for utilization estimate | set contextWindowLimit in your model config for accurate results"
                ));
                DEFAULT_CONTEXT_WINDOW_LIMIT
            }
        };
        #[expect(clippy::cast_precision_loss, reason = "a ratio estimate")]
        let ratio = input_tokens as f64 / limit as f64;
        ratio
    }
}

fn into_model_error(error: Error) -> Error {
    if error.is_model_error() {
        return error;
    }
    ModelError::new(error.to_string()).with_source(error).into()
}

#[derive(Debug, Default)]
struct ReasoningParts {
    text: Option<String>,
    signature: Option<String>,
    redacted_content: Option<Vec<u8>>,
}

impl ReasoningParts {
    fn is_empty(&self) -> bool {
        self.text.is_none() && self.signature.is_none() && self.redacted_content.is_none()
    }

    fn into_block(self) -> ReasoningBlock {
        let mut block = ReasoningBlock::new();
        if let Some(text) = self.text {
            block = block.with_text(text);
        }
        if let Some(signature) = self.signature {
            block = block.with_signature(signature);
        }
        if let Some(redacted) = self.redacted_content {
            block = block.with_redacted_content(redacted);
        }
        block
    }
}

/// The state of TS `streamAggregated`.
#[derive(Debug, Default)]
struct Aggregator {
    role: Option<Role>,
    blocks: Vec<ContentBlock>,
    text: String,
    tool_input: String,
    tool_name: String,
    tool_use_id: String,
    tool_reasoning_signature: String,
    reasoning: ReasoningParts,
    citations: Vec<Citation>,
    citation_content: Vec<CitationGeneratedContent>,
    stopped_message: Option<Message>,
    stop_reason: Option<StopReason>,
    metadata: Option<ModelMetadataEvent>,
    redaction: Option<String>,
    tool_input_error: Option<serde_json::Error>,
}

impl Aggregator {
    /// Folds one event in; returns the block a stop event completed.
    fn on_event(&mut self, event: &ModelStreamEvent) -> Option<ContentBlock> {
        match event {
            ModelStreamEvent::MessageStart(event) => {
                self.role = Some(event.role());
                self.blocks.clear();
            }
            ModelStreamEvent::BlockStart(event) => self.start_block(event.start()),
            ModelStreamEvent::BlockDelta(event) => self.add_delta(event.delta()),
            ModelStreamEvent::BlockStop(_) => return self.stop_block(),
            ModelStreamEvent::MessageStop(event) => {
                // Providers can end an empty turn with only a stop event, so the role defaults to the assistant's.
                let role = self.role.unwrap_or(Role::Assistant);
                let content = self
                    .blocks
                    .iter()
                    .filter(|block| !is_blank_text(block))
                    .cloned()
                    .collect();
                self.stopped_message = Some(Message::new(role, content));
                self.stop_reason = Some(event.stop_reason().clone());
            }
            ModelStreamEvent::Metadata(event) => self.metadata = Some(event.clone()),
            ModelStreamEvent::Redaction(event) => {
                if let Some(input) = event.input_redaction() {
                    self.redaction = Some(input.replace_content().to_owned());
                }
                if let Some(output) = event.output_redaction() {
                    let text = TextBlock::new(output.replace_content());
                    self.stopped_message = Some(Message::new(Role::Assistant, vec![text.into()]));
                }
            }
        }
        None
    }

    fn start_block(&mut self, start: Option<&ContentBlockStart>) {
        if let Some(ContentBlockStart::ToolUse(start)) = start {
            self.tool_name = start.name().to_owned();
            self.tool_use_id = start.tool_use_id().to_owned();
            self.tool_reasoning_signature = start.reasoning_signature().unwrap_or_default().to_owned();
        }
        self.tool_input.clear();
        self.text.clear();
        self.reasoning = ReasoningParts::default();
        self.citations.clear();
        self.citation_content.clear();
    }

    fn add_delta(&mut self, delta: &ContentBlockDelta) {
        match delta {
            ContentBlockDelta::Text(delta) => self.text.push_str(delta.text()),
            ContentBlockDelta::ToolUseInput(delta) => self.tool_input.push_str(delta.input()),
            ContentBlockDelta::ReasoningContent(delta) => {
                if let Some(text) = delta.text().filter(|text| !text.is_empty()) {
                    self.reasoning.text.get_or_insert_default().push_str(text);
                }
                if let Some(signature) = delta.signature().filter(|signature| !signature.is_empty()) {
                    self.reasoning.signature = Some(signature.to_owned());
                }
                if let Some(redacted) = delta.redacted_content() {
                    self.reasoning.redacted_content = Some(redacted.to_vec());
                }
            }
            ContentBlockDelta::Citations(delta) => {
                self.citations.extend_from_slice(delta.citations());
                self.citation_content.extend_from_slice(delta.content());
            }
        }
    }

    fn stop_block(&mut self) -> Option<ContentBlock> {
        let block = if self.tool_use_id.is_empty() {
            self.finish_content_block()
        } else {
            match self.finish_tool_use() {
                Ok(block) => block,
                Err(error) => {
                    tracing::error!(error = %error, "unable to parse tool input JSON");
                    self.tool_input_error = Some(error);
                    return None;
                }
            }
        };
        self.blocks.push(block.clone());
        Some(block)
    }

    fn finish_tool_use(&mut self) -> serde_json::Result<ContentBlock> {
        // Cleared before parsing so a failed block doesn't leak into the next one (TS keeps them).
        let name = std::mem::take(&mut self.tool_name);
        let tool_use_id = std::mem::take(&mut self.tool_use_id);
        let signature = std::mem::take(&mut self.tool_reasoning_signature);
        let input = if self.tool_input.is_empty() {
            JsonValue::Object(serde_json::Map::new())
        } else {
            from_json_str(&self.tool_input)?
        };
        let mut block = ToolUseBlock::new(name, tool_use_id, input);
        if !signature.is_empty() {
            block = block.with_reasoning_signature(signature);
        }
        Ok(block.into())
    }

    fn finish_content_block(&mut self) -> ContentBlock {
        if !self.reasoning.is_empty() {
            return std::mem::take(&mut self.reasoning).into_block().into();
        }
        let text = std::mem::take(&mut self.text);
        if self.citations.is_empty() {
            return TextBlock::new(text).into();
        }
        // Citations deltas that ground already-streamed text carry no content of their own.
        let mut content = std::mem::take(&mut self.citation_content);
        if content.is_empty() && !text.is_empty() {
            content.push(CitationGeneratedContent::new(text));
        }
        CitationsBlock::new(std::mem::take(&mut self.citations), content).into()
    }

    fn finish(self) -> Result<StreamAggregatedResult> {
        let (Some(mut message), Some(stop_reason)) = (self.stopped_message, self.stop_reason) else {
            let error = ModelError::new("Stream ended without completing a message");
            return Err(match self.tool_input_error {
                Some(cause) => error.with_source(cause),
                None => error,
            }
            .into());
        };
        let usage = self.metadata.as_ref().and_then(ModelMetadataEvent::usage);
        let metrics = self.metadata.as_ref().and_then(ModelMetadataEvent::metrics);
        if usage.is_some() || metrics.is_some() {
            let mut metadata = MessageMetadata::new();
            if let Some(usage) = usage {
                metadata = metadata.with_usage(*usage);
            }
            if let Some(metrics) = metrics {
                metadata = metadata.with_metrics(*metrics);
            }
            message.set_metadata(Some(metadata));
        }
        if stop_reason == StopReason::MaxTokens {
            return Err(MaxTokensError::new(
                "Model reached maximum token limit. This is an unrecoverable state that requires intervention.",
                message,
            )
            .into());
        }
        if let Some(cause) = self.tool_input_error {
            return Err(ModelError::new("unable to parse tool input JSON")
                .with_source(cause)
                .into());
        }
        let mut result = StreamAggregatedResult::new(message, stop_reason);
        if let Some(metadata) = self.metadata {
            result = result.with_metadata(metadata);
        }
        if let Some(user_message) = self.redaction {
            result = result.with_redaction(Redaction::new(user_message));
        }
        Ok(result)
    }
}

fn is_blank_text(block: &ContentBlock) -> bool {
    matches!(block, ContentBlock::Text(text) if js_trim(text.text()).is_empty())
}

/// Trims like JS `String.prototype.trim`, which strips U+FEFF and keeps U+0085, unlike `str::trim`.
pub(crate) fn js_trim(text: &str) -> &str {
    text.trim_matches(|c: char| (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}')
}

fn heuristic_text(text: &str) -> u64 {
    // TS counts UTF-16 code units (`text.length`); counting them keeps estimates identical.
    (text.encode_utf16().count() as u64).div_ceil(4)
}

fn heuristic_json<T: serde::Serialize + ?Sized>(value: &T) -> u64 {
    match to_js_string(value) {
        Ok(json) => (json.encode_utf16().count() as u64).div_ceil(2),
        Err(_) => {
            tracing::debug!("unable to serialize object for token estimation, skipping");
            0
        }
    }
}

fn estimate_content_block_tokens(block: &ContentBlock) -> u64 {
    match block {
        ContentBlock::Text(block) => heuristic_text(block.text()),
        ContentBlock::ToolUse(block) => {
            heuristic_text(block.name()).saturating_add(heuristic_json(block.input()))
        }
        ContentBlock::ToolResult(block) => block
            .content()
            .iter()
            .map(|item| match item {
                ToolResultContent::Text(text) => heuristic_text(text.text()),
                ToolResultContent::Json(json) => heuristic_json(json.json()),
                _ => 0,
            })
            .fold(0, u64::saturating_add),
        ContentBlock::Reasoning(block) => block.text().map_or(0, heuristic_text),
        ContentBlock::GuardContent(block) => block.text().map_or(0, |text| heuristic_text(text.text())),
        ContentBlock::Citations(block) => block
            .content()
            .iter()
            .map(|item| heuristic_text(item.text()))
            .fold(0, u64::saturating_add),
        ContentBlock::Injected(block) => heuristic_text(block.text()),
        ContentBlock::Summary(block) => heuristic_text(block.text()),
        _ => 0,
    }
}

fn estimate_system_prompt_tokens(system_prompt: &SystemPrompt) -> u64 {
    match system_prompt {
        SystemPrompt::Text(text) => heuristic_text(text),
        SystemPrompt::Blocks(blocks) => blocks
            .iter()
            .map(|block| match block {
                SystemContentBlock::Text(text) => heuristic_text(text.text()),
                SystemContentBlock::GuardContent(guard) => {
                    guard.text().map_or(0, |text| heuristic_text(text.text()))
                }
                _ => 0,
            })
            .fold(0, u64::saturating_add),
    }
}

fn estimate_tokens_heuristic(messages: &[Message], options: &CountTokensOptions) -> u64 {
    let latest_user = latest_user_index(messages);
    let message_tokens = messages
        .iter()
        .enumerate()
        .flat_map(|(index, message)| {
            let keep_injected = Some(index) == latest_user;
            message
                .content()
                .iter()
                .filter(move |block| keep_injected || !matches!(block, ContentBlock::Injected(_)))
        })
        .map(estimate_content_block_tokens)
        .fold(0, u64::saturating_add);
    let system_tokens = options
        .system_prompt
        .as_ref()
        .map_or(0, estimate_system_prompt_tokens);
    let tool_tokens = options
        .tool_specs
        .iter()
        .map(heuristic_json)
        .fold(0, u64::saturating_add);
    system_tokens
        .saturating_add(message_tokens)
        .saturating_add(tool_tokens)
}

#[cfg(test)]
mod tests;
