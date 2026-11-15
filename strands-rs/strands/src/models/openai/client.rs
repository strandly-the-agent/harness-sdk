//! The OpenAI API transport: what TS gets from the `openai` package (request headers, the
//! transport retry, the request timeout, SSE decoding and the API error shapes).

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use eventsource_stream::{Event, EventStreamError, Eventsource};
use futures::StreamExt;
use futures::future::BoxFuture;
use http::{HeaderMap, HeaderValue};

use crate::error::BoxError;
use crate::models::http_transport::{
    self, RetryPolicy, TransportError, is_truthy, merge_headers, parse_js_float, sensitive,
};
use crate::models::model::BoxStream;
use crate::retry::model_retry_strategy::Sleeper;
use crate::types::json::{JsonValue, from_json_str, to_js_string};

/// The base URL when neither the config nor `OPENAI_BASE_URL` sets one.
pub(crate) const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
/// The vendor SDK's retry count for connection errors and retryable statuses.
pub(crate) const DEFAULT_MAX_RETRIES: u32 = 2;
/// The vendor SDK's request timeout, which covers the wait for the response headers.
pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);
pub(crate) const RESPONSES_PATH: &str = "/responses";
pub(crate) const CHAT_COMPLETIONS_PATH: &str = "/chat/completions";
/// The largest delay one JS timer can represent; Node fires a longer (or negative) timer after 1 ms.
const MAX_TIMER_MS: f64 = 2_147_483_647.0;

/// An async function that returns the API key for the next request (TS `ApiKeySetter`).
pub type ApiKeySetter = Arc<dyn Fn() -> BoxFuture<'static, Result<String, BoxError>> + Send + Sync>;

/// What a failed client call was (the vendor SDK's `APIError` family, or another error).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApiErrorKind {
    /// The API answered with an error status or streamed an error (`APIError`).
    Api,
    /// The connection failed (`APIConnectionError`).
    Connection,
    /// The request timed out (`APIConnectionTimeoutError`).
    Timeout,
    /// Not an API error, such as an unparseable event or a failed `apiKey` function (`OpenAIError`).
    Other,
}

/// An error from the OpenAI client, with the vendor SDK's message text.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub(crate) struct ApiError {
    pub(crate) kind: ApiErrorKind,
    /// The HTTP status, when the API answered with one.
    pub(crate) status: Option<u16>,
    pub(crate) message: String,
    /// The error body's `code`, when it is a string.
    pub(crate) code: Option<String>,
}

impl ApiError {
    pub(crate) fn new(kind: ApiErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            status: None,
            message: message.into(),
            code: None,
        }
    }

    /// An error with a status and body, as `APIError.generate` builds it from a response.
    pub(crate) fn from_status(status: u16, text: &str) -> Self {
        let parsed = from_json_str::<JsonValue>(text).ok().filter(is_truthy);
        let error = parsed.as_ref().and_then(|body| body.get("error"));
        let fallback = if parsed.is_some() { None } else { Some(text) };
        let detail = make_message(error, fallback);
        let message = if detail.is_empty() {
            format!("{status} status code (no body)")
        } else {
            format!("{status} {detail}")
        };
        Self {
            kind: ApiErrorKind::Api,
            status: Some(status),
            message,
            code: error_code(error),
        }
    }

    /// An `error` carried in a streamed event (`new APIError(undefined, data.error, undefined)`).
    pub(crate) fn from_stream_error(error: &JsonValue) -> Self {
        let message = make_message(Some(error), None);
        Self {
            kind: ApiErrorKind::Api,
            status: None,
            message: if message.is_empty() {
                "(no status code or body)".to_owned()
            } else {
                message
            },
            code: error_code(Some(error)),
        }
    }

    /// The vendor SDK's `APIUserAbortError`.
    pub(crate) fn aborted() -> Self {
        Self::new(ApiErrorKind::Api, "Request was aborted.")
    }

    fn connection(error: &reqwest::Error) -> Self {
        if error.is_timeout() {
            return Self::new(ApiErrorKind::Timeout, "Request timed out.");
        }
        tracing::debug!("error=<{error}> | connection to the openai api failed");
        Self::new(ApiErrorKind::Connection, "Connection error.")
    }
}

/// `APIError.makeMessage` without the status: `error.message`, else the error as JSON, else `fallback`.
fn make_message(error: Option<&JsonValue>, fallback: Option<&str>) -> String {
    match error {
        Some(error) => match error.get("message") {
            Some(JsonValue::String(message)) if !message.is_empty() => message.clone(),
            Some(message) if is_truthy(message) => to_js_string(message).unwrap_or_default(),
            _ if is_truthy(error) => to_js_string(error).unwrap_or_default(),
            _ => fallback.unwrap_or_default().to_owned(),
        },
        None => fallback.unwrap_or_default().to_owned(),
    }
}

fn error_code(error: Option<&JsonValue>) -> Option<String> {
    error
        .and_then(|error| error.get("code"))
        .and_then(JsonValue::as_str)
        .map(str::to_owned)
}

/// One streaming call: the API path and the JSON body.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ApiRequest {
    pub(crate) path: &'static str,
    pub(crate) body: JsonValue,
}

/// The parsed events of a streamed response.
pub(crate) type EventStream = BoxStream<'static, Result<JsonValue, ApiError>>;

/// What the provider needs from an OpenAI client: the transport seam unit tests replace.
pub(crate) trait OpenAIClient: Send + Sync + 'static {
    /// Streams a request (`responses.create` or `chat.completions.create` with `stream: true`).
    fn stream(&self, request: ApiRequest) -> EventStream;
}

/// Where the bearer token of each request comes from.
#[derive(Clone)]
pub(crate) enum KeySource {
    Static(String),
    Setter(ApiKeySetter),
}

impl fmt::Debug for KeySource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Static(_) => f.write_str("Static(<redacted>)"),
            Self::Setter(_) => f.write_str("Setter(..)"),
        }
    }
}

impl KeySource {
    /// The key for one attempt; a setter runs before every attempt (the vendor `_callApiKey`).
    async fn resolve(&self) -> Result<String, ApiError> {
        let setter = match self {
            Self::Static(key) => return Ok(key.clone()),
            Self::Setter(setter) => setter,
        };
        match setter().await {
            Ok(token) if !token.is_empty() => Ok(token),
            Ok(token) => Err(ApiError::new(
                ApiErrorKind::Other,
                format!("Expected 'apiKey' function argument to return a string but it returned {token}"),
            )),
            Err(error) => Err(ApiError::new(
                ApiErrorKind::Other,
                format!("Failed to get token from 'apiKey' function: {error}"),
            )),
        }
    }
}

/// How to build an [`HttpClient`]; tests record these instead of building one.
#[derive(Clone)]
pub(crate) struct HttpClientParts {
    pub(crate) http: reqwest::Client,
    pub(crate) base_url: String,
    pub(crate) key: KeySource,
    /// Headers on every request, after the defaults and the `Authorization` header.
    pub(crate) headers: HeaderMap,
    pub(crate) max_retries: u32,
    pub(crate) timeout: Duration,
    pub(crate) sleeper: Sleeper,
    pub(crate) random: Arc<dyn Fn() -> f64 + Send + Sync>,
}

impl fmt::Debug for HttpClientParts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpClientParts")
            .field("base_url", &self.base_url)
            .field("key", &self.key)
            .field("max_retries", &self.max_retries)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

/// The default [`OpenAIClient`]: the API over `reqwest`, with the vendor SDK's retry.
#[derive(Clone)]
pub(crate) struct HttpClient {
    inner: Arc<HttpInner>,
}

struct HttpInner {
    parts: HttpClientParts,
}

impl fmt::Debug for HttpClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpClient")
            .field("parts", &self.inner.parts)
            .finish_non_exhaustive()
    }
}

impl HttpClient {
    pub(crate) fn new(parts: HttpClientParts) -> Self {
        Self {
            inner: Arc::new(HttpInner { parts }),
        }
    }
}

impl HttpInner {
    fn url(&self, path: &str) -> String {
        let base = &self.parts.base_url;
        match base.strip_suffix('/') {
            Some(trimmed) if path.starts_with('/') => format!("{trimmed}{path}"),
            _ => format!("{base}{path}"),
        }
    }

    async fn build_request(&self, request: &ApiRequest) -> Result<reqwest::Request, ApiError> {
        let other = |error: &dyn std::fmt::Display| ApiError::new(ApiErrorKind::Other, error.to_string());
        let body = to_js_string(&request.body).map_err(|error| other(&error))?;
        let key = self.parts.key.resolve().await?;
        let mut headers = HeaderMap::new();
        headers.insert(http::header::ACCEPT, HeaderValue::from_static("application/json"));
        if let Ok(value) = HeaderValue::from_str(&format!("Bearer {key}")) {
            headers.insert(http::header::AUTHORIZATION, sensitive(value));
        }
        let mut headers = merge_headers(headers, &self.parts.headers);
        headers.insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        self.parts
            .http
            .post(self.url(request.path))
            .headers(headers)
            .body(body)
            .build()
            .map_err(|error| other(&error))
    }

    /// Sends `request` with the vendor SDK's retry.
    async fn send(&self, request: &ApiRequest) -> Result<reqwest::Response, ApiError> {
        let policy = RetryPolicy {
            max_retries: self.parts.max_retries,
            timeout: self.parts.timeout,
            sleeper: &self.parts.sleeper,
            random: self.parts.random.as_ref(),
            retry_after,
            label: "openai",
        };
        http_transport::send(&self.parts.http, &policy, || self.build_request(request)).await
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

/// The delay `retry-after-ms` or `retry-after` asks for (the vendor `retryRequest`); `None`
/// means the default backoff. Node runs a timer that is negative, `NaN` or too long after 1 ms.
pub(crate) fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
    let mut millis = header("retry-after-ms").and_then(parse_js_float);
    if millis.is_none_or(|millis| millis == 0.0)
        && let Some(value) = header("retry-after")
    {
        millis = Some(retry_after_millis(value));
    }
    let millis = millis?;
    if (1.0..=MAX_TIMER_MS).contains(&millis) {
        Some(Duration::from_secs_f64(millis / 1000.0))
    } else {
        Some(Duration::from_millis(u64::from(millis.is_nan() || millis != 0.0)))
    }
}

/// `retry-after` as seconds, else as an HTTP date relative to now (`NaN` when neither parses).
fn retry_after_millis(value: &str) -> f64 {
    if let Some(seconds) = parse_js_float(value) {
        return seconds * 1000.0;
    }
    let Ok(date) = jiff::fmt::rfc2822::parse(value) else {
        return f64::NAN;
    };
    let delta = date
        .timestamp()
        .as_millisecond()
        .saturating_sub(jiff::Timestamp::now().as_millisecond());
    #[expect(clippy::cast_precision_loss, reason = "a delay in milliseconds")]
    let delta = delta as f64;
    delta
}

/// What one SSE message becomes: skipped, the end of the stream, a parsed event, or an error.
enum Parsed {
    Skip,
    Done,
    Event(JsonValue),
    Failed(ApiError),
}

/// The vendor `Stream.fromSSEResponse`: `[DONE]` ends the stream and a data `error` fails it.
fn parse_event(event: &Event) -> Parsed {
    if event.data.starts_with("[DONE]") {
        return Parsed::Done;
    }
    if event.data.is_empty() {
        return Parsed::Skip;
    }
    match from_json_str::<JsonValue>(&event.data) {
        Ok(data) => match data.get("error") {
            Some(error) if is_truthy(error) => Parsed::Failed(ApiError::from_stream_error(error)),
            _ => Parsed::Event(data),
        },
        Err(error) => Parsed::Failed(ApiError::new(ApiErrorKind::Other, error.to_string())),
    }
}

impl OpenAIClient for HttpClient {
    fn stream(&self, request: ApiRequest) -> EventStream {
        let inner = Arc::clone(&self.inner);
        Box::pin(async_stream::stream! {
            let response = match inner.send(&request).await {
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
                    Err(EventStreamError::Transport(error)) => Parsed::Failed(ApiError::connection(&error)),
                    Err(error) => Parsed::Failed(ApiError::new(ApiErrorKind::Other, error.to_string())),
                };
                match parsed {
                    Parsed::Skip => {}
                    Parsed::Done => return,
                    Parsed::Event(event) => yield Ok(event),
                    Parsed::Failed(error) => {
                        yield Err(error);
                        return;
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests;
