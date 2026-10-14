//! Anthropic Messages API model provider (`strands-ts/src/models/anthropic.ts`).
//!
//! The TypeScript provider delegates HTTP/SSE to `@anthropic-ai/sdk`; this port speaks the wire
//! protocol directly over [`AnthropicTransport`] (default: `reqwest`), so tests can feed canned
//! SSE bytes offline.

mod format;
mod parse;

pub use format::{format_count_tokens_request, format_request};
pub use parse::{map_stop_reason, parse_sse_events, SseEvent};

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::{BoxStream, Stream};
use futures::StreamExt;
use serde::Deserialize;

use super::defaults::{
    default_max_tokens_warning_message, default_model_warning_message, get_context_window_limit, MODEL_DEFAULTS,
};
use super::model::{CountTokensOptions, Model, ModelEventStream, StreamOptions};
use super::streaming::ModelStreamEvent;
use crate::error::{Error, Result};
use crate::logging::warn_once;
use crate::types::{JsonValue, Message, Usage};

/// Maximum `pause_turn` continuations before giving up (`MAX_PAUSE_TURN_CONTINUATIONS` in TS).
pub const MAX_PAUSE_TURN_CONTINUATIONS: usize = 10;

/// Default Anthropic API base URL.
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

/// `anthropic-version` header value sent with every request.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Substrings that classify a provider error as a context-window overflow
/// (`CONTEXT_WINDOW_OVERFLOW_ERRORS` in TS).
pub const CONTEXT_WINDOW_OVERFLOW_ERRORS: &[&str] = &[
    "prompt is too long",
    "max_tokens exceeded",
    "input too long",
    "input is too long",
    "input length exceeds context window",
    "input and output tokens exceed your context limit",
];

/// Content block types produced by Anthropic server-side tools; these are logged, not emitted
/// (`SERVER_TOOL_BLOCK_TYPES` in TS).
pub const SERVER_TOOL_BLOCK_TYPES: &[&str] = &[
    "bash_code_execution_tool_result",
    "code_execution_tool_result",
    "container_upload",
    "mcp_tool_result",
    "mcp_tool_use",
    "server_tool_use",
    "text_editor_code_execution_tool_result",
    "tool_search_tool_result",
    "web_fetch_tool_result",
    "web_search_tool_result",
];

/// Prompt-cache strategy (`CacheStrategy` in TS).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AnthropicCacheStrategy {
    /// Cache when the model is known to support it.
    #[default]
    Auto,
    /// Force Anthropic-style cache points.
    Anthropic,
}

/// Per-section cache setting (`boolean | CacheTTL` in TS). Absent means "enabled, no TTL".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnthropicCacheSection {
    /// Enabled without its own TTL.
    Enabled,
    /// Disabled.
    Disabled,
    /// Enabled with this TTL (sent verbatim; the API validates it).
    Ttl(String),
}

/// Prompt-cache configuration (`CacheConfig` in TS).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AnthropicCacheConfig {
    /// Strategy; `None` means `auto`.
    pub strategy: Option<AnthropicCacheStrategy>,
    /// Default TTL for all sections.
    pub ttl: Option<String>,
    /// Tool-definition section.
    pub tools_ttl: Option<AnthropicCacheSection>,
    /// System-prompt section.
    pub system_prompt_ttl: Option<AnthropicCacheSection>,
    /// Conversation section.
    pub messages_ttl: Option<AnthropicCacheSection>,
}

/// Configuration for [`AnthropicModel`] (`AnthropicModelConfig` + `AnthropicModelOptions` in TS).
#[derive(Clone, Debug, Default)]
pub struct AnthropicModelConfig {
    /// Model id; defaults to `MODEL_DEFAULTS.anthropic.model_id`.
    pub model_id: Option<String>,
    /// Max output tokens; defaults to `MODEL_DEFAULTS.anthropic.max_tokens` (64_000).
    pub max_tokens: Option<u64>,
    /// Sampling temperature.
    pub temperature: Option<f64>,
    /// Nucleus sampling.
    pub top_p: Option<f64>,
    /// Stop sequences.
    pub stop_sequences: Option<Vec<String>>,
    /// Extra request-body keys, shallow-merged last (user params win).
    pub params: Option<serde_json::Map<String, JsonValue>>,
    /// Server-side tool definitions (`web_search`, `code_execution`, …).
    pub anthropic_tools: Option<Vec<JsonValue>>,
    /// Beta flags, joined into the `anthropic-beta` header.
    pub betas: Option<Vec<String>>,
    /// Use the provider's `count_tokens` endpoint instead of the character heuristic.
    pub use_native_token_count: bool,
    /// Prompt-cache configuration.
    pub cache_config: Option<AnthropicCacheConfig>,
    /// API key; falls back to `ANTHROPIC_API_KEY`.
    pub api_key: Option<String>,
    /// API base URL; defaults to [`DEFAULT_BASE_URL`].
    pub base_url: Option<String>,
}

/// A raw HTTP response body, as a stream of byte chunks.
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Vec<u8>>> + Send>>;

/// An HTTP response from [`AnthropicTransport`].
pub struct TransportResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response body chunks.
    pub body: ByteStream,
}

/// The HTTP seam under [`AnthropicModel`]; implement it to test without network access.
#[async_trait]
pub trait AnthropicTransport: Send + Sync {
    /// POST `body` as JSON to `url` with `headers`, returning the status and the raw body stream.
    async fn post(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
        body: JsonValue,
    ) -> Result<TransportResponse>;
}

/// `reqwest`-backed [`AnthropicTransport`].
#[derive(Clone, Debug)]
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl Default for ReqwestTransport {
    fn default() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl AnthropicTransport for ReqwestTransport {
    async fn post(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
        body: JsonValue,
    ) -> Result<TransportResponse> {
        let mut req = self.client.post(url).json(&body);
        for (name, value) in headers {
            req = req.header(name, value);
        }
        let response = req.send().await.map_err(|e| Error::model(e.to_string()))?;
        let status = response.status().as_u16();
        let body = response
            .bytes_stream()
            .map(|chunk| chunk.map(|b| b.to_vec()).map_err(|e| Error::model(e.to_string())))
            .boxed();
        Ok(TransportResponse { status, body })
    }
}

/// Anthropic Messages API provider.
pub struct AnthropicModel {
    config: AnthropicModelConfig,
    model_id: String,
    max_tokens: u64,
    api_key: String,
    base_url: String,
    transport: Arc<dyn AnthropicTransport>,
}

impl std::fmt::Debug for AnthropicModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicModel")
            .field("model_id", &self.model_id)
            .field("max_tokens", &self.max_tokens)
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl AnthropicModel {
    /// Build a model from `config`, reading `ANTHROPIC_API_KEY` when no key is configured.
    pub fn new(config: AnthropicModelConfig) -> Result<Self> {
        Self::with_transport(config, Arc::new(ReqwestTransport::default()))
    }

    /// Like [`AnthropicModel::new`] but with a caller-supplied transport.
    pub fn with_transport(config: AnthropicModelConfig, transport: Arc<dyn AnthropicTransport>) -> Result<Self> {
        let defaults = MODEL_DEFAULTS.anthropic;
        let model_id = match &config.model_id {
            Some(id) => id.clone(),
            None => {
                warn_once(&default_model_warning_message(defaults.model_id));
                defaults.model_id.to_owned()
            }
        };
        let max_tokens = match config.max_tokens {
            Some(n) => n,
            None => {
                let default = defaults.max_tokens.unwrap_or(64_000);
                warn_once(&default_max_tokens_warning_message(default));
                default
            }
        };
        validate_anthropic_tools(config.anthropic_tools.as_deref())?;
        let api_key = config
            .api_key
            .clone()
            .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
            .filter(|k| !k.is_empty())
            .ok_or_else(|| {
                Error::model(
                    "Anthropic API key is required. Provide it via the 'apiKey' option or set the \
                     ANTHROPIC_API_KEY environment variable."
                        .to_owned(),
                )
            })?;
        let base_url = config
            .base_url
            .clone()
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned())
            .trim_end_matches('/')
            .to_owned();
        Ok(Self {
            config,
            model_id,
            max_tokens,
            api_key,
            base_url,
            transport,
        })
    }

    /// The resolved configuration.
    pub fn config(&self) -> &AnthropicModelConfig {
        &self.config
    }

    /// Resolved max output tokens.
    pub fn max_tokens(&self) -> u64 {
        self.max_tokens
    }

    fn headers(&self) -> Vec<(String, String)> {
        let mut headers = vec![
            ("x-api-key".to_owned(), self.api_key.clone()),
            ("anthropic-version".to_owned(), ANTHROPIC_VERSION.to_owned()),
            ("content-type".to_owned(), "application/json".to_owned()),
        ];
        if let Some(betas) = self.config.betas.as_ref().filter(|b| !b.is_empty()) {
            headers.push(("anthropic-beta".to_owned(), betas.join(",")));
        }
        headers
    }

    /// Format the streaming request body for `messages` (exposed for tests and snapshots).
    pub fn format_request(&self, messages: &[Message], options: &StreamOptions) -> Result<JsonValue> {
        format_request(&self.config, &self.model_id, self.max_tokens, messages, options)
    }
}

fn validate_anthropic_tools(tools: Option<&[JsonValue]>) -> Result<()> {
    for tool in tools.unwrap_or(&[]) {
        if tool.get("input_schema").is_some() {
            return Err(Error::model(
                "anthropicTools should not contain function tool definitions. Pass function tools \
                 through the agent's tool registry instead."
                    .to_owned(),
            ));
        }
    }
    Ok(())
}

/// Map a provider error body/status onto the SDK error taxonomy.
pub fn map_error(status: u16, message: &str) -> Error {
    let lowered = message.to_lowercase();
    if CONTEXT_WINDOW_OVERFLOW_ERRORS.iter().any(|m| lowered.contains(m)) {
        return Error::ContextWindowOverflow(message.to_owned());
    }
    if status == 429 {
        return Error::throttled(message.to_owned());
    }
    Error::model(message.to_owned())
}

#[derive(Debug, Deserialize)]
struct ErrorEnvelope {
    error: Option<ErrorBody>,
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    message: Option<String>,
}

fn error_message(status: u16, body: &str) -> String {
    serde_json::from_str::<ErrorEnvelope>(body)
        .ok()
        .and_then(|e| e.error.and_then(|b| b.message))
        .unwrap_or_else(|| {
            if body.trim().is_empty() {
                format!("anthropic request failed with status {status}")
            } else {
                body.trim().to_owned()
            }
        })
}

async fn drain(mut body: ByteStream) -> String {
    let mut out = Vec::new();
    while let Some(chunk) = body.next().await {
        match chunk {
            Ok(bytes) => out.extend_from_slice(&bytes),
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[async_trait]
impl Model for AnthropicModel {
    fn model_id(&self) -> Option<String> {
        Some(self.model_id.clone())
    }

    fn context_window_limit(&self) -> Option<u64> {
        get_context_window_limit(&self.model_id)
    }

    fn stream<'a>(&'a self, messages: &'a [Message], options: &'a StreamOptions) -> ModelEventStream<'a> {
        Box::pin(async_stream::try_stream! {
            let mut request = self.format_request(messages, options)?;
            let url = format!("{}/v1/messages", self.base_url);
            let mut continuations = 0usize;
            let mut usage = Usage::default();

            'outer: loop {
                if let Some(cancel) = &options.cancel {
                    if cancel.is_cancelled() {
                        Err(Error::Cancelled)?;
                    }
                }
                let TransportResponse { status, mut body } =
                    self.transport.post(&url, self.headers(), request.clone()).await?;
                if !(200..300).contains(&status) {
                    let text = drain(body).await;
                    Err(map_error(status, &error_message(status, &text)))?;
                    return;
                }

                let mut state = parse::StreamState::new(continuations == 0);
                let mut buffer = Vec::<u8>::new();
                while let Some(chunk) = body.next().await {
                    if let Some(cancel) = &options.cancel {
                        if cancel.is_cancelled() {
                            Err(Error::Cancelled)?;
                        }
                    }
                    buffer.extend_from_slice(&chunk?);
                    for sse in parse::drain_sse(&mut buffer) {
                        for event in state.handle(&sse)? {
                            yield event;
                        }
                    }
                }
                for sse in parse::flush_sse(&mut buffer) {
                    for event in state.handle(&sse)? {
                        yield event;
                    }
                }

                if !state.message_stopped {
                    return;
                }
                parse::accumulate(&mut usage, &state.usage);
                if state.stop_reason_is_pause_turn() {
                    continuations += 1;
                    if continuations > MAX_PAUSE_TURN_CONTINUATIONS {
                        Err(Error::model(format!(
                            "server-side tool turn did not complete after {continuations} continuations"
                        )))?;
                    }
                    let assistant = state.final_assistant_message();
                    if let Some(list) = request.get_mut("messages").and_then(|m| m.as_array_mut()) {
                        list.push(assistant);
                    }
                    continue 'outer;
                }
                usage.total_tokens = usage.input_tokens
                    + usage.output_tokens
                    + usage.cache_read_input_tokens.unwrap_or(0)
                    + usage.cache_write_input_tokens.unwrap_or(0);
                yield ModelStreamEvent::Metadata {
                    usage: Some(usage.clone()),
                    metrics: None,
                    trace: None,
                };
                yield ModelStreamEvent::MessageStop {
                    stop_reason: state.stop_reason.clone(),
                    additional_model_response_fields: None,
                };
                return;
            }
        })
    }

    async fn count_tokens(&self, messages: &[Message], options: &CountTokensOptions) -> Result<u64> {
        if !self.config.use_native_token_count {
            return Ok(super::model::estimate_tokens_heuristic(messages, options));
        }
        match self.native_count_tokens(messages, options).await {
            Ok(n) => Ok(n),
            Err(e) => {
                tracing::debug!("error=<{e}> | native token counting failed, falling back to estimation");
                Ok(super::model::estimate_tokens_heuristic(messages, options))
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct CountTokensResponse {
    input_tokens: u64,
}

impl AnthropicModel {
    async fn native_count_tokens(&self, messages: &[Message], options: &CountTokensOptions) -> Result<u64> {
        let body = format_count_tokens_request(&self.config, &self.model_id, messages, options)?;
        let url = format!("{}/v1/messages/count_tokens", self.base_url);
        let response = self.transport.post(&url, self.headers(), body).await?;
        let status = response.status;
        let text = drain(response.body).await;
        if !(200..300).contains(&status) {
            return Err(map_error(status, &error_message(status, &text)));
        }
        let parsed: CountTokensResponse =
            serde_json::from_str(&text).map_err(|e| Error::model(e.to_string()))?;
        Ok(parsed.input_tokens)
    }
}

/// A [`BoxStream`] of canned SSE chunks, for tests.
pub type CannedStream = BoxStream<'static, Result<Vec<u8>>>;

#[cfg(test)]
mod tests;
