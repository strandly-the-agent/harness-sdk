//! What the Anthropic and OpenAI transports share: both vendor SDKs (`@anthropic-ai/sdk`,
//! `openai`) retry, back off, time out and merge headers the same way. Error wording and the
//! `retry-after` rules stay with each provider, since the two SDKs differ there.

use std::fmt;
use std::future::Future;
use std::time::Duration;

use futures::future::{Either, select};
use http::{HeaderMap, HeaderName, HeaderValue};

use crate::retry::model_retry_strategy::Sleeper;
use crate::types::json::JsonValue;

/// The errors a transport attempt can end with, in the provider's own error type.
pub(crate) trait TransportError: fmt::Display {
    /// A failure before any response arrived.
    fn connection(error: &reqwest::Error) -> Self;
    /// No response headers within the timeout.
    fn timed_out() -> Self;
    /// A non-success response.
    fn from_status(status: u16, text: &str) -> Self;
}

/// How one provider retries (the vendor SDK's `maxRetries`, `timeout` and backoff).
pub(crate) struct RetryPolicy<'a> {
    pub(crate) max_retries: u32,
    pub(crate) timeout: Duration,
    pub(crate) sleeper: &'a Sleeper,
    pub(crate) random: &'a (dyn Fn() -> f64 + Send + Sync),
    /// The delay a retryable response asks for; `None` means the default backoff.
    pub(crate) retry_after: fn(&HeaderMap) -> Option<Duration>,
    /// The provider name in the retry log line.
    pub(crate) label: &'static str,
}

/// One attempt, bounded by the timeout up to the response headers.
async fn attempt<E: TransportError>(
    http: &reqwest::Client,
    request: reqwest::Request,
    policy: &RetryPolicy<'_>,
) -> Result<reqwest::Response, E> {
    let send = http.execute(request);
    let timer = (policy.sleeper)(policy.timeout);
    match select(Box::pin(send), timer).await {
        Either::Left((Ok(response), _)) => Ok(response),
        Either::Left((Err(error), _)) => Err(E::connection(&error)),
        Either::Right(((), _)) => Err(E::timed_out()),
    }
}

/// Sends the request `build` makes, retrying as the vendor SDKs do: connection errors and
/// timeouts, and the statuses [`should_retry`] accepts, up to `max_retries` times. `build` runs
/// before every attempt.
pub(crate) async fn send<E, B, F>(
    http: &reqwest::Client,
    policy: &RetryPolicy<'_>,
    mut build: B,
) -> Result<reqwest::Response, E>
where
    E: TransportError,
    B: FnMut() -> F,
    F: Future<Output = Result<reqwest::Request, E>>,
{
    let mut retries_remaining = policy.max_retries;
    loop {
        let request = build().await?;
        let retry_after = match attempt::<E>(http, request, policy).await {
            Ok(response) if response.status().is_success() => return Ok(response),
            Ok(response)
                if retries_remaining > 0 && should_retry(response.status().as_u16(), response.headers()) =>
            {
                (policy.retry_after)(response.headers())
            }
            Ok(response) => {
                let status = response.status().as_u16();
                let text = response.text().await.unwrap_or_default();
                return Err(E::from_status(status, &text));
            }
            Err(error) if retries_remaining > 0 => {
                tracing::debug!(
                    "retries_remaining=<{retries_remaining}>, error=<{error}> | retrying {} request",
                    policy.label
                );
                None
            }
            Err(error) => return Err(error),
        };
        let delay = retry_after
            .unwrap_or_else(|| default_retry_delay(retries_remaining, policy.max_retries, (policy.random)()));
        (policy.sleeper)(delay).await;
        retries_remaining = retries_remaining.saturating_sub(1);
    }
}

/// The vendor SDKs' `shouldRetry`: `x-should-retry` wins, else 408, 409, 429 and 5xx.
pub(crate) fn should_retry(status: u16, headers: &HeaderMap) -> bool {
    match headers
        .get("x-should-retry")
        .and_then(|value| value.to_str().ok())
    {
        Some("true") => return true,
        Some("false") => return false,
        _ => {}
    }
    matches!(status, 408 | 409 | 429) || status >= 500
}

/// The vendor SDKs' backoff: 0.5 s doubling per retry up to 8 s, less up to 25 % jitter.
pub(crate) fn default_retry_delay(retries_remaining: u32, max_retries: u32, random: f64) -> Duration {
    let retries = max_retries.saturating_sub(retries_remaining);
    let exponent = i32::try_from(retries).unwrap_or(i32::MAX);
    let seconds = (0.5 * 2f64.powi(exponent)).min(8.0);
    let jitter = 1.0 - random.clamp(0.0, 1.0) * 0.25;
    Duration::from_secs_f64(seconds * jitter)
}

/// JS `parseFloat`: the longest leading decimal number, ignoring leading whitespace.
pub(crate) fn parse_js_float(value: &str) -> Option<f64> {
    let trimmed = value.trim_start();
    let end = trimmed
        .char_indices()
        .take_while(|(index, c)| {
            c.is_ascii_digit()
                || *c == '.'
                || ((*c == '-' || *c == '+') && *index == 0)
                || *c == 'e'
                || *c == 'E'
        })
        .map(|(index, c)| index + c.len_utf8())
        .last()?;
    (1..=end)
        .rev()
        .filter_map(|len| trimmed.get(..len))
        .find_map(|prefix| prefix.parse::<f64>().ok())
        .filter(|parsed| parsed.is_finite())
}

/// JS truthiness of a JSON value.
pub(crate) fn is_truthy(value: &JsonValue) -> bool {
    match value {
        JsonValue::Null => false,
        JsonValue::Bool(flag) => *flag,
        JsonValue::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        JsonValue::String(text) => !text.is_empty(),
        JsonValue::Array(_) | JsonValue::Object(_) => true,
    }
}

/// `value`, marked so `Debug` output hides it.
pub(crate) fn sensitive(mut value: HeaderValue) -> HeaderValue {
    value.set_sensitive(true);
    value
}

/// Parses a `*_CUSTOM_HEADERS` variable (`env_name`): one `name: value` per line, as the vendor
/// SDKs do.
pub(crate) fn parse_custom_headers(value: &str, env_name: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for line in value.split('\n') {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.trim().as_bytes()),
            HeaderValue::from_str(value.trim()),
        ) else {
            tracing::debug!("line=<{line}> | skipping an invalid {env_name} entry");
            continue;
        };
        headers.insert(name, value);
    }
    headers
}

/// `base` with every header of `overrides` replacing the same-named headers of `base`.
pub(crate) fn merge_headers(base: HeaderMap, overrides: &HeaderMap) -> HeaderMap {
    let mut merged = base;
    for name in overrides.keys() {
        merged.remove(name);
    }
    for (name, value) in overrides {
        merged.append(name, value.clone());
    }
    merged
}

#[cfg(test)]
mod tests;
