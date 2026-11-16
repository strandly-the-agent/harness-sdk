//! The HTTP request tool (TS `vended-tools/http-request/`).

use std::collections::BTreeMap;

use futures::StreamExt;
use serde::{Deserialize, Serialize};

use crate::JsonValue;
use crate::retry::model_retry_strategy::{Sleeper, tokio_sleeper};
use crate::tools::function_tool::InputValidator;
use crate::tools::{FunctionTool, ToolContext, ToolError};

use super::shell::timeout_from_secs;
use super::{js_number, no_precheck, schema_tool};

const DESCRIPTION: &str = "Makes HTTP requests to external APIs. Supports GET, POST, PUT, DELETE, PATCH, HEAD, and OPTIONS methods. Returns response with status, headers, and body.";
const DEFAULT_TIMEOUT_SECS: f64 = 30.0;
/// The largest response body read; the body is read as a stream and stops at the cap.
const MAX_BODY_BYTES: usize = 10_485_760;

/// What the HTTP request tool returns (TS `HttpRequestOutput`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct HttpRequestOutput {
    /// The status code.
    pub status: u16,
    /// The status text (the canonical reason phrase).
    pub status_text: String,
    /// Response headers by lowercase name; repeated headers are joined with `, `.
    pub headers: BTreeMap<String, String>,
    /// The response body as text.
    pub body: String,
}

#[derive(Deserialize)]
struct HttpRequestInput {
    method: String,
    url: String,
    headers: Option<BTreeMap<String, String>>,
    body: Option<String>,
    timeout: Option<f64>,
}

fn http_request_input_schema() -> JsonValue {
    serde_json::json!({
        "type": "object",
        "properties": {
            "method": {
                "type": "string",
                "enum": ["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"],
                "description": "HTTP method to use for the request",
            },
            "url": { "type": "string", "format": "uri", "description": "URL to send the request to" },
            "headers": {
                "description": "Optional HTTP headers as key-value pairs",
                "type": "object",
                "propertyNames": { "type": "string" },
                "additionalProperties": { "type": "string" },
            },
            "body": { "description": "Optional request body as a string", "type": "string" },
            "timeout": {
                "description": "Optional timeout in seconds (default: 30)",
                "type": "number",
                "exclusiveMinimum": 0,
            },
        },
        "required": ["method", "url"],
        "additionalProperties": false,
    })
}

/// The HTTP request tool (TS `httpRequest`). A non-2xx status is an error; the request ends on
/// its timeout or when the invocation is cancelled.
pub fn http_request() -> FunctionTool {
    http_request_with(tokio_sleeper())
}

/// [`http_request`] timing out on `sleeper`.
pub(crate) fn http_request_with(sleeper: Sleeper) -> FunctionTool {
    let client = reqwest::Client::new();
    let schema = http_request_input_schema();
    schema_tool(
        "http_request".to_owned(),
        DESCRIPTION.to_owned(),
        schema.clone(),
        InputValidator::with_formats(&schema),
        no_precheck,
        move |input: HttpRequestInput, context: ToolContext| {
            let (client, sleeper) = (client.clone(), sleeper.clone());
            async move {
                let timeout_secs = input.timeout.unwrap_or(DEFAULT_TIMEOUT_SECS);
                let timeout = timeout_from_secs(timeout_secs)?;
                let label = format!("{} {}", input.method, input.url);
                let signal = context.cancel_signal().clone();
                tokio::select! {
                    result = send(&client, input) => result,
                    () = sleeper(timeout) => Err(ToolError::msg(format!(
                        "Request timed out after {} seconds: {label}",
                        js_number(timeout_secs)
                    ))),
                    () = signal.cancelled() => Err(ToolError::msg(format!("Request cancelled: {label}"))),
                }
            }
        },
    )
}

async fn send(client: &reqwest::Client, input: HttpRequestInput) -> Result<HttpRequestOutput, ToolError> {
    let method = reqwest::Method::from_bytes(input.method.as_bytes()).map_err(ToolError::from)?;
    let mut request = client.request(method, &input.url);
    for (name, value) in input.headers.iter().flatten() {
        request = request.header(name, value);
    }
    if let Some(body) = input.body {
        request = request.body(body);
    }
    let response = request.send().await?;
    let status = response.status();
    let mut headers = BTreeMap::<String, String>::new();
    for (name, value) in response.headers() {
        let value = String::from_utf8_lossy(value.as_bytes()).into_owned();
        let entry = headers.entry(name.as_str().to_owned());
        // fetch `Headers` iteration (TS) yields each `set-cookie` on its own, so the last one wins.
        if name == reqwest::header::SET_COOKIE {
            *entry.or_default() = value;
            continue;
        }
        entry
            .and_modify(|joined| {
                joined.push_str(", ");
                joined.push_str(&value);
            })
            .or_insert(value);
    }
    let body = read_capped(response, &input.method, &input.url).await?;
    let status_text = status.canonical_reason().unwrap_or_default().to_owned();
    if !status.is_success() {
        return Err(ToolError::msg(format!(
            "HTTP {} {status_text}: {} {}",
            status.as_u16(),
            input.method,
            input.url
        )));
    }
    Ok(HttpRequestOutput {
        status: status.as_u16(),
        status_text,
        headers,
        body,
    })
}

async fn read_capped(response: reqwest::Response, method: &str, url: &str) -> Result<String, ToolError> {
    let mut body = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk?;
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(ToolError::msg(format!(
                "Response body exceeds the {MAX_BODY_BYTES}-byte limit: {method} {url}"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

#[cfg(test)]
mod tests;
