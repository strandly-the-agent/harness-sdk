//! The model abstraction: request, config and cache types (TS `models/model.ts`).

use crate::agent::agent_metadata::AgentMetadata;
use crate::cancel::CancelSignal;
use crate::models::model_info::ModelInfo;
use crate::state_store::StateStore;
use crate::types::json::JsonValue;
use crate::types::messages::{Message, SystemPrompt};
use crate::types::tools::{ToolChoice, ToolSpec};

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

/// Options for counting tokens (TS `CountTokensOptions`).
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

#[cfg(test)]
mod tests;
