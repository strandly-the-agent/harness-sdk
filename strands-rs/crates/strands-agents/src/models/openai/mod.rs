//! OpenAI model provider (`strands-ts/src/models/openai/*`): Responses API and Chat Completions.

mod chat;
mod mantle;
mod responses;

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use serde_json::{Map, Value};

use super::defaults::{default_model_warning_message, get_context_window_limit, MODEL_DEFAULTS};
use super::model::{Model, ModelEventStream, StreamOptions};
use super::sse::SseParser;
use crate::error::{Error, Result};
use crate::logging::warn_once;
use crate::types::{JsonValue, Message};

pub use mantle::{resolve_mantle_base_path, resolve_mantle_base_url, BedrockMantleConfig};

/// Which OpenAI API the provider talks to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OpenAIApi {
    /// `POST /responses` (default).
    #[default]
    Responses,
    /// `POST /chat/completions`.
    Chat,
}

/// Configuration for [`OpenAIModel`].
#[derive(Clone, Debug, Default)]
pub struct OpenAIModelConfig {
    /// Which API to use; defaults to [`OpenAIApi::Responses`].
    pub api: OpenAIApi,
    /// API key; falls back to `OPENAI_API_KEY`.
    pub api_key: Option<String>,
    /// Base URL, defaults to `https://api.openai.com/v1` (or the Mantle-derived URL).
    pub base_url: Option<String>,
    /// Model id; defaults to `gpt-5.4`.
    pub model_id: Option<String>,
    /// Sampling temperature.
    pub temperature: Option<f64>,
    /// Output token cap (`max_output_tokens` / `max_completion_tokens`).
    pub max_tokens: Option<u64>,
    /// Nucleus sampling.
    pub top_p: Option<f64>,
    /// Chat-only frequency penalty.
    pub frequency_penalty: Option<f64>,
    /// Chat-only presence penalty.
    pub presence_penalty: Option<f64>,
    /// Reasoning effort (`minimal` | `low` | `medium` | `high`).
    pub reasoning_effort: Option<String>,
    /// Responses-only: emit `strict` on function tools instead of deferring to the server default.
    pub strict_tools: Option<bool>,
    /// Responses-only: keep conversation state server-side (`store`).
    pub stateful: bool,
    /// Extra request params; managed fields always win over these.
    pub params: Map<String, Value>,
    /// Bedrock Mantle (OpenAI-compatible Bedrock endpoint) configuration.
    pub mantle: Option<BedrockMantleConfig>,
}

/// Managed request fields that a user's `params` may not override (Responses API).
const RESPONSES_MANAGED: &[&str] = &["model", "input", "stream", "store"];
/// Managed request fields that a user's `params` may not override (Chat Completions).
const CHAT_MANAGED: &[&str] = &["model", "messages", "stream", "stream_options"];

/// An HTTP request the provider wants executed.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenAIRequest {
    /// Fully-qualified URL.
    pub url: String,
    /// JSON request body.
    pub body: JsonValue,
    /// Authorization bearer token, if any.
    pub bearer_token: Option<String>,
}

/// Transport that executes an OpenAI request and yields raw SSE body chunks.
#[async_trait]
pub trait OpenAITransport: Send + Sync + std::fmt::Debug {
    /// Execute the request, returning the response body as a chunk stream.
    async fn send(&self, request: OpenAIRequest) -> Result<BoxStream<'static, Result<String>>>;
}

/// `reqwest`-backed transport used when no transport is injected.
#[derive(Debug, Default)]
pub struct ReqwestTransport {
    client: reqwest::Client,
}

#[async_trait]
impl OpenAITransport for ReqwestTransport {
    async fn send(&self, request: OpenAIRequest) -> Result<BoxStream<'static, Result<String>>> {
        let mut builder = self.client.post(&request.url).json(&request.body);
        if let Some(token) = &request.bearer_token {
            builder = builder.bearer_auth(token);
        }
        let response = builder
            .send()
            .await
            .map_err(|e| Error::model(format!("openai request failed: {e}")))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(classify_openai_error(status.as_u16(), None, &body));
        }
        let stream = response.bytes_stream().map(|chunk| {
            chunk
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .map_err(|e| Error::model(format!("openai stream failed: {e}")))
        });
        Ok(stream.boxed())
    }
}

/// OpenAI model provider.
#[derive(Clone, Debug)]
pub struct OpenAIModel {
    config: OpenAIModelConfig,
    model_id: String,
    base_url: String,
    api_key: Option<String>,
    transport: Arc<dyn OpenAITransport>,
}

impl OpenAIModel {
    /// Build a provider, resolving the model id, base URL and credentials.
    pub fn new(config: OpenAIModelConfig) -> Result<Self> {
        Self::with_transport(config, Arc::new(ReqwestTransport::default()))
    }

    /// Build a provider with an injected transport (offline tests, custom HTTP stacks).
    pub fn with_transport(config: OpenAIModelConfig, transport: Arc<dyn OpenAITransport>) -> Result<Self> {
        let model_id = match &config.model_id {
            Some(id) => id.clone(),
            None => {
                let default_id = MODEL_DEFAULTS.openai.model_id.to_string();
                warn_once(&default_model_warning_message("openai", &default_id));
                default_id
            }
        };
        let base_url = match (&config.base_url, &config.mantle) {
            (Some(url), _) => url.trim_end_matches('/').to_string(),
            (None, Some(mantle)) => resolve_mantle_base_url(mantle, &model_id)?,
            (None, None) => "https://api.openai.com/v1".to_string(),
        };
        let api_key = match (&config.api_key, &config.mantle) {
            (Some(key), _) => Some(key.clone()),
            (None, Some(_)) => mantle::mint_bearer_token()?,
            (None, None) => Some(std::env::var("OPENAI_API_KEY").map_err(|_| {
                Error::model(
                    "OpenAI API key is required. Provide it via the 'apiKey' option (string or function) or set the OPENAI_API_KEY environment variable.",
                )
            })?),
        };
        Self::warn_managed_params(&config);
        Ok(Self {
            config,
            model_id,
            base_url,
            api_key,
            transport,
        })
    }

    fn warn_managed_params(config: &OpenAIModelConfig) {
        let managed: BTreeSet<&str> = match config.api {
            OpenAIApi::Responses => RESPONSES_MANAGED.iter().copied().collect(),
            OpenAIApi::Chat => CHAT_MANAGED.iter().copied().collect(),
        };
        for key in config.params.keys() {
            if managed.contains(key.as_str()) {
                warn_once(&format!(
                    "param={key} | params contains a managed openai request field, ignoring"
                ));
            }
        }
    }

    /// The request this provider would send for `messages` (also used by tests).
    pub fn format_request(&self, messages: &[Message], options: &StreamOptions) -> Result<OpenAIRequest> {
        if messages.is_empty() {
            return Err(Error::model("At least one message is required"));
        }
        let (path, body) = match self.config.api {
            OpenAIApi::Responses => (
                "/responses",
                responses::format_request(&self.config, &self.model_id, messages, options)?,
            ),
            OpenAIApi::Chat => (
                "/chat/completions",
                chat::format_request(&self.config, &self.model_id, messages, options)?,
            ),
        };
        Ok(OpenAIRequest {
            url: format!("{}{path}", self.base_url),
            body,
            bearer_token: self.api_key.clone(),
        })
    }
}

#[async_trait]
impl Model for OpenAIModel {
    fn model_id(&self) -> Option<String> {
        Some(self.model_id.clone())
    }

    fn context_window_limit(&self) -> Option<u64> {
        get_context_window_limit(&self.model_id)
    }

    fn stateful(&self) -> bool {
        matches!(self.config.api, OpenAIApi::Responses) && self.config.stateful
    }

    fn stream<'a>(&'a self, messages: &'a [Message], options: &'a StreamOptions) -> ModelEventStream<'a> {
        let api = self.config.api;
        let request = self.format_request(messages, options);
        let transport = self.transport.clone();
        let cancel = options.cancel.clone();
        futures::stream::once(async move { request })
            .then(move |request| {
                let transport = transport.clone();
                async move {
                    let request = request?;
                    transport.send(request).await
                }
            })
            .flat_map(move |body| match body {
                Err(e) => futures::stream::once(async move { Err(e) }).boxed(),
                Ok(chunks) => decode(api, chunks).boxed(),
            })
            .take_while(move |_| {
                let cancelled = cancel.as_ref().is_some_and(|c| c.is_cancelled());
                async move { !cancelled }
            })
            .boxed()
    }
}

/// Decode an SSE body into `ModelStreamEvent`s with the adapter's state machine.
fn decode(api: OpenAIApi, chunks: BoxStream<'static, Result<String>>) -> impl futures::Stream<Item = Result<super::streaming::ModelStreamEvent>> {
    async_stream::try_stream! {
        let mut parser = SseParser::new();
        let mut responses_state = responses::ResponsesStream::default();
        let mut chat_state = chat::ChatStream::default();
        let mut chunks = chunks;
        while let Some(chunk) = chunks.next().await {
            for sse in parser.push(&chunk?) {
                let events = match api {
                    OpenAIApi::Responses => responses_state.handle(sse.event.as_deref(), &sse.data)?,
                    OpenAIApi::Chat => chat_state.handle(&sse.data)?,
                };
                for event in events {
                    yield event;
                }
            }
        }
        let tail = match api {
            OpenAIApi::Responses => responses_state.finalize(),
            OpenAIApi::Chat => chat_state.finalize(),
        };
        for event in tail {
            yield event;
        }
    }
}

/// Map an HTTP status / error code / message to the SDK error type.
///
/// Throttling is checked before context overflow (matching the TS provider).
pub fn classify_openai_error(status: u16, code: Option<&str>, message: &str) -> Error {
    let lower = message.to_lowercase();
    let code = code.unwrap_or("");
    let throttled = status == 429
        || code == "rate_limit_exceeded"
        || ["rate_limit_exceeded", "rate limit", "too many requests"]
            .iter()
            .any(|needle| lower.contains(needle));
    if throttled {
        return Error::throttled(message.to_string());
    }
    const OVERFLOW: &[&str] = &[
        "maximum context length",
        "context_length_exceeded",
        "too many tokens",
        "context length",
        "input is too long for requested model",
        "input length and `max_tokens` exceed context limit",
        "too many total text bytes",
        "exceed customer model maximum",
    ];
    if code == "context_length_exceeded" || OVERFLOW.iter().any(|needle| lower.contains(needle)) {
        return Error::ContextWindowOverflow(message.to_string());
    }
    Error::model(message.to_string())
}

#[cfg(test)]
mod tests;
