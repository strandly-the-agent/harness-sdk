//! The Messages API transport: what TS gets from `@anthropic-ai/sdk` (request headers, the
//! transport retry, the request timeout, SSE decoding and the API error shapes).

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use eventsource_stream::{Event, EventStreamError, Eventsource};
use futures::StreamExt;
use http::{HeaderMap, HeaderValue};

use super::stream::MessageSnapshot;
use crate::models::http_transport::{
    self, RetryPolicy, TransportError, is_truthy, merge_headers, parse_js_float, sensitive,
};
use crate::models::model::BoxStream;
use crate::retry::model_retry_strategy::Sleeper;
use crate::types::json::{JsonValue, from_json_str, to_js_string};

/// The base URL when neither the config nor `ANTHROPIC_BASE_URL` sets one.
pub(crate) const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
/// The `anthropic-version` the vendor SDK (1.x) sends unless a header pins one.
pub(crate) const DEFAULT_API_VERSION: &str = "2026-10-21";
/// The vendor SDK's retry count for connection errors and retryable statuses.
pub(crate) const DEFAULT_MAX_RETRIES: u32 = 2;
/// The vendor SDK's request timeout, which covers the wait for the response headers.
pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

const MESSAGES_PATH: &str = "/v1/messages";
const BETA_MESSAGES_PATH: &str = "/v1/messages?beta=true";
const COUNT_TOKENS_PATH: &str = "/v1/messages/count_tokens";
const API_KEY_HEADER: &str = "x-api-key";
const BETA_HEADER: &str = "anthropic-beta";
const VERSION_HEADER: &str = "anthropic-version";
/// The largest delay one JS timer can represent; longer `retry-after` values use the default backoff.
const MAX_RETRY_AFTER_MS: f64 = 2_147_483_647.0;

/// What a failed client call was (the vendor SDK's `APIError` family, or another error).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApiErrorKind {
    /// The API answered with an error status or streamed an `error` event (`APIError`).
    Api,
    /// The connection failed (`APIConnectionError`).
    Connection,
    /// The request timed out (`APIConnectionTimeoutError`).
    Timeout,
    /// Not an API error, such as an unparseable event.
    Other,
}

/// An error from the Messages API client, with the vendor SDK's message text.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub(crate) struct ApiError {
    pub(crate) kind: ApiErrorKind,
    /// The HTTP status, when the API answered with one.
    pub(crate) status: Option<u16>,
    pub(crate) message: String,
    /// The parsed error body, when there is one.
    pub(crate) body: Option<JsonValue>,
}

impl ApiError {
    pub(crate) fn new(kind: ApiErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            status: None,
            message: message.into(),
            body: None,
        }
    }

    pub(crate) fn status(&self) -> Option<u16> {
        self.status
    }

    /// Whether the vendor SDK would throw an `APIError` (any kind but [`ApiErrorKind::Other`]).
    pub(crate) fn is_api_error(&self) -> bool {
        self.kind != ApiErrorKind::Other
    }

    /// The body's own `error.message`, else the error message (TS `apiErrorMessage`).
    pub(crate) fn api_message(&self) -> &str {
        self.body
            .as_ref()
            .and_then(|body| body.pointer("/error/message"))
            .and_then(JsonValue::as_str)
            .unwrap_or(&self.message)
    }

    /// An error with a status and body, worded as the vendor SDK's `APIError.makeMessage`.
    pub(crate) fn from_status(status: u16, text: &str) -> Self {
        let body = from_json_str::<JsonValue>(text).ok();
        let detail = match &body {
            Some(body) => body_message(body),
            None => text.to_owned(),
        };
        let message = if detail.is_empty() {
            format!("{status} status code (no body)")
        } else {
            format!("{status} {detail}")
        };
        Self {
            kind: ApiErrorKind::Api,
            status: Some(status),
            message,
            body,
        }
    }

    /// An SSE `error` event, worded as the vendor SDK words a status-less `APIError`.
    fn from_event(data: &str) -> Self {
        let body = from_json_str::<JsonValue>(data).ok();
        let message = body.as_ref().map_or_else(|| data.to_owned(), body_message);
        Self {
            kind: ApiErrorKind::Api,
            status: None,
            message: if message.is_empty() {
                "(no status code or body)".to_owned()
            } else {
                message
            },
            body,
        }
    }

    fn connection(error: &reqwest::Error) -> Self {
        if error.is_timeout() {
            return Self::new(ApiErrorKind::Timeout, "Request timed out.");
        }
        tracing::debug!("error=<{error}> | connection to the anthropic api failed");
        Self::new(ApiErrorKind::Connection, "Connection error.")
    }
}

/// `error.message` when the body has a string one, else the body as JSON (`APIError.makeMessage`).
fn body_message(body: &JsonValue) -> String {
    match body.get("message") {
        Some(JsonValue::String(message)) if !message.is_empty() => message.clone(),
        Some(message) if is_truthy(message) => to_js_string(message).unwrap_or_default(),
        _ => to_js_string(body).unwrap_or_default(),
    }
}

/// One Messages API call: the JSON body and the per-request `anthropic-beta` header.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ApiRequest {
    pub(crate) body: JsonValue,
    pub(crate) beta: Option<String>,
}

/// The parsed events of a streamed Messages response.
pub(crate) type EventStream = BoxStream<'static, Result<JsonValue, ApiError>>;

/// What the provider needs from a Messages API client (TS `AnthropicMessagesClient`): the
/// transport seam unit tests replace.
#[async_trait]
pub(crate) trait MessagesClient: Send + Sync + 'static {
    /// Streams a Messages request (`messages.stream`).
    fn stream(&self, request: ApiRequest) -> EventStream;

    /// Whether the client has the token counting API (Bedrock clients don't).
    fn has_count_tokens(&self) -> bool {
        true
    }

    /// Counts a request's input tokens (`messages.countTokens`); the response JSON.
    async fn count_tokens(&self, request: ApiRequest) -> Result<JsonValue, ApiError>;

    /// Whether the client has the beta Messages API.
    fn has_beta(&self) -> bool {
        true
    }

    /// Streams a beta Messages request to its final message (`beta.messages.stream(..).finalMessage()`).
    async fn beta_final_message(&self, request: ApiRequest) -> Result<JsonValue, ApiError>;
}

/// The default [`MessagesClient`]: the Messages API over `reqwest`, with the vendor SDK's retry.
#[derive(Clone)]
pub(crate) struct HttpClient {
    inner: Arc<HttpInner>,
}

struct HttpInner {
    http: reqwest::Client,
    base_url: String,
    /// Every request's headers: the defaults, the API key, then the configured headers.
    headers: HeaderMap,
    max_retries: u32,
    timeout: Duration,
    sleeper: Sleeper,
    random: Arc<dyn Fn() -> f64 + Send + Sync>,
}

impl fmt::Debug for HttpClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpClient")
            .field("base_url", &self.inner.base_url)
            .field("max_retries", &self.inner.max_retries)
            .field("timeout", &self.inner.timeout)
            .finish_non_exhaustive()
    }
}

/// How to build an [`HttpClient`].
pub(crate) struct HttpClientParts {
    pub(crate) base_url: String,
    pub(crate) api_key: String,
    /// Headers on every request, after the defaults; they may pin `anthropic-version`.
    pub(crate) headers: HeaderMap,
    pub(crate) max_retries: u32,
    pub(crate) timeout: Duration,
    pub(crate) sleeper: Sleeper,
    pub(crate) random: Arc<dyn Fn() -> f64 + Send + Sync>,
}

impl HttpClient {
    /// Builds the client.
    ///
    /// # Errors
    ///
    /// Fails when the TLS backend can't be initialized.
    pub(crate) fn new(parts: HttpClientParts) -> Result<Self, reqwest::Error> {
        let HttpClientParts {
            base_url,
            api_key,
            headers: extra,
            max_retries,
            timeout,
            sleeper,
            random,
        } = parts;
        let mut defaults = HeaderMap::new();
        defaults.insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        defaults.insert(VERSION_HEADER, HeaderValue::from_static(DEFAULT_API_VERSION));
        if let Ok(key) = HeaderValue::from_str(&api_key) {
            defaults.insert(API_KEY_HEADER, sensitive(key));
        }
        let headers = merge_headers(defaults, &extra);
        let http = reqwest::Client::builder().build()?;
        Ok(Self {
            inner: Arc::new(HttpInner {
                http,
                base_url: base_url.trim_end_matches('/').to_owned(),
                headers,
                max_retries,
                timeout,
                sleeper,
                random,
            }),
        })
    }
}

impl HttpInner {
    fn build_request(&self, path: &str, request: &ApiRequest) -> Result<reqwest::Request, ApiError> {
        let body = to_js_string(&request.body)
            .map_err(|error| ApiError::new(ApiErrorKind::Other, error.to_string()))?;
        let mut headers = self.headers.clone();
        // A request header replaces the client's header of the same name.
        if let Some(beta) = &request.beta {
            let value = HeaderValue::from_str(beta)
                .map_err(|error| ApiError::new(ApiErrorKind::Other, error.to_string()))?;
            headers.insert(BETA_HEADER, value);
        }
        self.http
            .post(format!("{}{path}", self.base_url))
            .headers(headers)
            .body(body)
            .build()
            .map_err(|error| ApiError::new(ApiErrorKind::Other, error.to_string()))
    }

    /// Sends `request` with the vendor SDK's retry.
    async fn send(&self, path: &str, request: &ApiRequest) -> Result<reqwest::Response, ApiError> {
        let policy = RetryPolicy {
            max_retries: self.max_retries,
            timeout: self.timeout,
            sleeper: &self.sleeper,
            random: self.random.as_ref(),
            retry_after,
            label: "anthropic",
        };
        http_transport::send(&self.http, &policy, || {
            std::future::ready(self.build_request(path, request))
        })
        .await
    }
}

impl TransportError for ApiError {
    fn connection(error: &reqwest::Error) -> Self {
        Self::connection(error)
    }

    fn timed_out() -> Self {
        Self::new(ApiErrorKind::Timeout, "Request timed out.")
    }

    fn from_status(status: u16, text: &str) -> Self {
        Self::from_status(status, text)
    }
}

/// The delay `retry-after-ms` or `retry-after` asks for, when it is a positive delay one timer can hold.
pub(crate) fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
    let millis = header("retry-after-ms")
        .and_then(parse_js_float)
        .filter(|millis| *millis != 0.0)
        .or_else(|| header("retry-after").and_then(retry_after_millis))?;
    (millis > 0.0 && millis <= MAX_RETRY_AFTER_MS).then(|| Duration::from_secs_f64(millis / 1000.0))
}

fn retry_after_millis(value: &str) -> Option<f64> {
    if let Some(seconds) = parse_js_float(value) {
        return Some(seconds * 1000.0);
    }
    let date = jiff::fmt::rfc2822::parse(value).ok()?;
    let delta = date
        .timestamp()
        .as_millisecond()
        .saturating_sub(jiff::Timestamp::now().as_millisecond());
    #[expect(clippy::cast_precision_loss, reason = "a delay in milliseconds")]
    Some(delta as f64)
}

/// What one SSE event becomes: skipped, a parsed event, or the stream's error.
fn parse_event(event: &Event) -> Option<Result<JsonValue, ApiError>> {
    match event.event.as_str() {
        // eventsource-stream names an unnamed event `message`; the vendor SDK skips unnamed events.
        "ping" | "message" => None,
        "error" => Some(Err(ApiError::from_event(&event.data))),
        _ if event.data.is_empty() => None,
        _ => Some(
            from_json_str::<JsonValue>(&event.data)
                .map_err(|error| ApiError::new(ApiErrorKind::Other, error.to_string())),
        ),
    }
}

impl HttpClient {
    fn events(&self, path: &'static str, request: ApiRequest) -> EventStream {
        let inner = Arc::clone(&self.inner);
        Box::pin(async_stream::stream! {
            let response = match inner.send(path, &request).await {
                Ok(response) => response,
                Err(error) => {
                    yield Err(error);
                    return;
                }
            };
            let mut events = response.bytes_stream().eventsource();
            while let Some(item) = events.next().await {
                let parsed = match item {
                    Ok(event) => parse_event(&event),
                    Err(EventStreamError::Transport(error)) => Some(Err(ApiError::connection(&error))),
                    Err(error) => Some(Err(ApiError::new(ApiErrorKind::Other, error.to_string()))),
                };
                match parsed {
                    None => {}
                    Some(Ok(event)) => yield Ok(event),
                    Some(Err(error)) => {
                        yield Err(error);
                        return;
                    }
                }
            }
        })
    }
}

#[async_trait]
impl MessagesClient for HttpClient {
    fn stream(&self, request: ApiRequest) -> EventStream {
        self.events(MESSAGES_PATH, request)
    }

    async fn count_tokens(&self, request: ApiRequest) -> Result<JsonValue, ApiError> {
        let response = self.inner.send(COUNT_TOKENS_PATH, &request).await?;
        let text = response
            .text()
            .await
            .map_err(|error| ApiError::connection(&error))?;
        from_json_str(&text).map_err(|error| ApiError::new(ApiErrorKind::Other, error.to_string()))
    }

    async fn beta_final_message(&self, request: ApiRequest) -> Result<JsonValue, ApiError> {
        let mut events = self.events(BETA_MESSAGES_PATH, request);
        let mut snapshot = MessageSnapshot::default();
        while let Some(event) = events.next().await {
            snapshot.apply(&event?);
        }
        snapshot.final_message().ok_or_else(|| {
            ApiError::new(
                ApiErrorKind::Other,
                "stream ended without producing a Message with role=assistant",
            )
        })
    }
}

/// The string values of `name` in `headers`, in order.
pub(crate) fn header_values<'a>(headers: &'a HeaderMap, name: &str) -> impl Iterator<Item = &'a str> {
    headers
        .get_all(name)
        .into_iter()
        .filter_map(|value| value.to_str().ok())
}

#[cfg(test)]
mod tests;
