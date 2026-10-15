//! The `http_request` tool (`strands-ts/src/vended-tools/http-request/http-request.ts`).
//!
//! Issues one HTTP request through an injectable [`reqwest::Client`] and returns the status,
//! status text, headers and body. A non-2xx response is an error, as in TS. The body is read in
//! full and never truncated, so a hostile or very large response is a memory risk — put such
//! endpoints behind a client with its own limits.
//!
//! Divergences from TS: `fetch` reports the status text the server sent, while this port derives
//! it from the code's canonical reason (empty for an unknown code); zod's `.url()` check becomes
//! URL parsing, reported as [`Error::ToolValidation`]; and a cancelled call returns
//! [`Error::Cancelled`] — the variant the executor already understands — rather than TS's
//! `Request cancelled: …` message. `method` is accepted case-insensitively (`get` works) where
//! TS's `z.enum` requires upper case. The timeout message matches TS verbatim.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::tools::tool::{create_error_result, Tool, ToolContext};
use crate::tools::wrap_in_tool_result;
use crate::types::{ToolResultBlock, ToolSpec};

/// Name of the HTTP request tool.
pub const HTTP_REQUEST_TOOL_NAME: &str = "http_request";

/// Description of the HTTP request tool (verbatim from TS).
pub const DEFAULT_HTTP_REQUEST_DESCRIPTION: &str = "Makes HTTP requests to external APIs. Supports GET, POST, PUT, DELETE, PATCH, HEAD, and OPTIONS methods. Returns response with status, headers, and body.";

/// Timeout applied when the input omits one, in seconds (TS default: 30).
pub const DEFAULT_TIMEOUT_SECONDS: f64 = 30.0;

/// The methods the tool accepts, in TS schema order.
const METHODS: [&str; 7] = ["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"];

/// Configuration of an [`HttpRequestTool`].
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct HttpRequestConfig {
    /// Tool name reported to the model.
    pub name: String,
    /// Tool description reported to the model.
    pub description: String,
    /// Timeout used when the input omits `timeout`.
    pub timeout: Duration,
    /// Client the requests go through; swap it out to add proxies, TLS roots or redirect policy.
    pub client: reqwest::Client,
}

impl Default for HttpRequestConfig {
    fn default() -> Self {
        Self {
            name: HTTP_REQUEST_TOOL_NAME.to_owned(),
            description: DEFAULT_HTTP_REQUEST_DESCRIPTION.to_owned(),
            timeout: Duration::from_secs_f64(DEFAULT_TIMEOUT_SECONDS),
            client: reqwest::Client::new(),
        }
    }
}

/// Input of the HTTP request tool (`HttpRequestInput` in TS).
#[derive(Clone, Debug, Deserialize)]
pub struct HttpRequestInput {
    /// HTTP method to use for the request.
    pub method: String,
    /// URL to send the request to.
    pub url: String,
    /// Optional HTTP headers as key-value pairs.
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
    /// Optional request body as a string.
    #[serde(default)]
    pub body: Option<String>,
    /// Optional timeout in seconds (default: 30).
    #[serde(default)]
    pub timeout: Option<f64>,
}

/// Output of the HTTP request tool (`HttpRequestOutput` in TS).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HttpRequestOutput {
    /// HTTP status code.
    pub status: u16,
    /// HTTP status text.
    #[serde(rename = "statusText")]
    pub status_text: String,
    /// Response headers as key-value pairs.
    pub headers: HashMap<String, String>,
    /// Response body as text.
    pub body: String,
}

/// The `http_request` tool: one HTTP request per call, cancellation-aware.
#[derive(Clone, Debug)]
pub struct HttpRequestTool {
    config: HttpRequestConfig,
    spec: ToolSpec,
}

impl Default for HttpRequestTool {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpRequestTool {
    /// HTTP request tool with the default configuration (30 second timeout, a fresh client).
    ///
    /// # Panics
    ///
    /// Panics if the default [`reqwest::Client`] cannot be built; pass your own client through
    /// [`HttpRequestTool::with_config`] to control construction.
    pub fn new() -> Self {
        Self::with_config(HttpRequestConfig::default())
    }

    /// HTTP request tool with an explicit configuration.
    pub fn with_config(config: HttpRequestConfig) -> Self {
        let spec = http_request_tool_spec(&config);
        Self { config, spec }
    }

    /// Send the request, returning the response once it has been read to the end.
    ///
    /// Errors on an invalid method, URL or timeout, on a transport failure, on a timeout, and —
    /// as in TS — on any non-2xx status.
    pub async fn execute(&self, input: &HttpRequestInput) -> Result<HttpRequestOutput> {
        let method = parse_method(&input.method)?;
        let (timeout, timeout_secs) = self.timeout(input.timeout)?;
        let url = reqwest::Url::parse(&input.url).map_err(|err| {
            Error::ToolValidation(format!("url must be a valid URL, got '{}': {err}", input.url))
        })?;

        let mut request = self.config.client.request(method.clone(), url);
        for (name, value) in input.headers.iter().flatten() {
            request = request.header(name, value);
        }
        if let Some(body) = &input.body {
            request = request.body(body.clone());
        }

        tokio::select! {
            biased;
            () = tokio::time::sleep(timeout) => Err(Error::other(format!(
                "Request timed out after {timeout_secs} seconds: {method} {}",
                input.url
            ))),
            response = send(request, &method, &input.url) => response,
        }
    }

    /// The timeout to apply, as a duration and as the seconds TS puts in its message.
    fn timeout(&self, requested: Option<f64>) -> Result<(Duration, f64)> {
        let Some(secs) = requested else {
            return Ok((self.config.timeout, self.config.timeout.as_secs_f64()));
        };
        if !(secs.is_finite() && secs > 0.0) {
            return Err(Error::ToolValidation(format!(
                "timeout must be a positive, finite number of seconds, got {secs}"
            )));
        }
        Ok((Duration::from_secs_f64(secs), secs))
    }
}

/// Send `request` and read the whole response, failing on a non-2xx status as TS does.
async fn send(
    request: reqwest::RequestBuilder,
    method: &reqwest::Method,
    url: &str,
) -> Result<HttpRequestOutput> {
    let response = request
        .send()
        .await
        .map_err(|err| Error::other(format!("Request failed: {method} {url}: {err}")))?;

    let status = response.status();
    let status_text = status.canonical_reason().unwrap_or_default().to_owned();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                value.to_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    let body = response
        .text()
        .await
        .map_err(|err| Error::other(format!("Request failed: {method} {url}: {err}")))?;

    if !status.is_success() {
        return Err(Error::other(format!(
            "HTTP {} {status_text}: {method} {url}",
            status.as_u16()
        )));
    }

    Ok(HttpRequestOutput {
        status: status.as_u16(),
        status_text,
        headers,
        body,
    })
}

/// Uppercase `method` and check it against the schema's enum.
fn parse_method(method: &str) -> Result<reqwest::Method> {
    let upper = method.to_ascii_uppercase();
    if !METHODS.contains(&upper.as_str()) {
        return Err(Error::ToolValidation(format!(
            "method must be one of {}, got '{method}'",
            METHODS.join(", ")
        )));
    }
    reqwest::Method::from_bytes(upper.as_bytes())
        .map_err(|err| Error::ToolValidation(format!("invalid method '{method}': {err}")))
}

/// Spec of the HTTP request tool, mirroring the TS zod schema.
fn http_request_tool_spec(config: &HttpRequestConfig) -> ToolSpec {
    ToolSpec {
        name: config.name.clone(),
        description: config.description.clone(),
        input_schema: Some(serde_json::json!({
            "type": "object",
            "properties": {
                "method": {
                    "type": "string",
                    "enum": METHODS,
                    "description": "HTTP method to use for the request",
                },
                "url": {
                    "type": "string",
                    "format": "uri",
                    "description": "URL to send the request to",
                },
                "headers": {
                    "type": "object",
                    "additionalProperties": { "type": "string" },
                    "description": "Optional HTTP headers as key-value pairs",
                },
                "body": {
                    "type": "string",
                    "description": "Optional request body as a string",
                },
                "timeout": {
                    "type": "number",
                    "exclusiveMinimum": 0,
                    "description": "Optional timeout in seconds (default: 30)",
                },
            },
            "required": ["method", "url"],
            "additionalProperties": false,
        })),
        output_schema: None,
        annotations: None,
    }
}

#[async_trait]
impl Tool for HttpRequestTool {
    fn name(&self) -> &str {
        &self.spec.name
    }

    fn description(&self) -> &str {
        &self.spec.description
    }

    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn invoke(&self, ctx: ToolContext) -> Result<ToolResultBlock> {
        let input: HttpRequestInput = ctx.input()?;
        if ctx.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }

        let result = tokio::select! {
            biased;
            () = ctx.cancel.cancelled() => return Err(Error::Cancelled),
            result = self.execute(&input) => result,
        };

        match result {
            Ok(output) => {
                let value =
                    serde_json::to_value(output).map_err(|err| Error::JsonValidation(err.to_string()))?;
                Ok(wrap_in_tool_result(value, &ctx.tool_use.tool_use_id))
            }
            Err(err) => Ok(create_error_result(err, &ctx.tool_use.tool_use_id)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::tool::test_context;
    use crate::types::ToolResultStatus;
    use parking_lot::Mutex;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A single-response HTTP server on a loopback port, recording the requests it received.
    struct TestServer {
        base_url: String,
        requests: Arc<Mutex<Vec<String>>>,
    }

    /// Serve `response` (a raw HTTP response) to every connection, recording each request head.
    async fn serve(response: &'static str) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);

        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let recorded = Arc::clone(&recorded);
                tokio::spawn(async move {
                    let mut raw = Vec::new();
                    let mut chunk = [0u8; 1024];
                    while !is_complete(&raw) {
                        match stream.read(&mut chunk).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => raw.extend_from_slice(&chunk[..n]),
                        }
                    }
                    recorded.lock().push(String::from_utf8_lossy(&raw).into_owned());
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.flush().await;
                });
            }
        });

        TestServer { base_url, requests }
    }

    /// Accept connections and never answer, so the caller's timeout or cancellation wins.
    async fn serve_forever() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });
        base_url
    }

    /// A loopback address with nothing listening on it.
    async fn dead_url() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        format!("http://{addr}")
    }

    /// Whether `raw` holds a full request: head plus as many body bytes as `content-length` says.
    fn is_complete(raw: &[u8]) -> bool {
        let text = String::from_utf8_lossy(raw);
        let Some(head_end) = text.find("\r\n\r\n") else {
            return false;
        };
        let length = text[..head_end]
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())?
            })
            .unwrap_or(0);
        raw.len() >= head_end + 4 + length
    }

    fn response(status: u16, status_text: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status} {status_text}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn input(method: &str, url: &str) -> HttpRequestInput {
        HttpRequestInput {
            method: method.to_owned(),
            url: url.to_owned(),
            headers: None,
            body: None,
            timeout: Some(5.0),
        }
    }

    async fn error_message(tool: &HttpRequestTool, input: &HttpRequestInput) -> String {
        tool.execute(input)
            .await
            .expect_err("expected failure")
            .to_string()
    }

    #[tokio::test]
    async fn returns_successful_response_for_every_method() {
        // TS: `describe.each([...])('$method request') > returns successful response`.
        let cases = [
            ("GET", 200, "OK"),
            ("POST", 201, "Created"),
            ("PUT", 200, "OK"),
            ("DELETE", 204, "No Content"),
            ("PATCH", 200, "OK"),
            ("HEAD", 200, "OK"),
            ("OPTIONS", 200, "OK"),
        ];

        for (method, status, status_text) in cases {
            let body = if status == 204 { "" } else { "{\"success\":true}" };
            let raw: &'static str = Box::leak(response(status, status_text, body).into_boxed_str());
            let server = serve(raw).await;
            let tool = HttpRequestTool::new();

            let output = tool
                .execute(&input(method, &server.base_url))
                .await
                .unwrap_or_else(|err| panic!("{method} failed: {err}"));

            assert_eq!(output.status, status, "{method}");
            assert_eq!(output.status_text, status_text, "{method}");
            assert_eq!(output.headers["content-type"], "application/json");
            // HEAD responses carry no body, as the server must not send one.
            if method != "HEAD" {
                assert_eq!(output.body, body, "{method}");
            }
        }
    }

    #[tokio::test]
    async fn sends_request_with_custom_headers_and_body() {
        let raw: &'static str = Box::leak(response(200, "OK", "ok").into_boxed_str());
        let server = serve(raw).await;
        let tool = HttpRequestTool::new();

        let mut request = input("POST", &server.base_url);
        request.headers = Some(HashMap::from([
            ("content-type".to_owned(), "application/json".to_owned()),
            ("x-api-key".to_owned(), "secret".to_owned()),
        ]));
        request.body = Some("{\"name\":\"test\"}".to_owned());

        tool.execute(&request).await.unwrap();

        let received = server.requests.lock()[0].clone();
        assert!(received.starts_with("POST / HTTP/1.1"), "{received}");
        assert!(received.to_lowercase().contains("content-type: application/json"));
        assert!(received.to_lowercase().contains("x-api-key: secret"));
        assert!(received.ends_with("{\"name\":\"test\"}"), "{received}");
    }

    #[tokio::test]
    async fn omits_headers_and_body_when_not_provided() {
        let raw: &'static str = Box::leak(response(200, "OK", "ok").into_boxed_str());
        let server = serve(raw).await;
        let tool = HttpRequestTool::new();

        tool.execute(&input("GET", &server.base_url)).await.unwrap();

        let received = server.requests.lock()[0].clone();
        assert!(!received.to_lowercase().contains("content-length"), "{received}");
        assert!(received.ends_with("\r\n\r\n"), "{received}");
    }

    #[tokio::test]
    async fn defaults_the_timeout_to_thirty_seconds() {
        let tool = HttpRequestTool::new();
        let (duration, secs) = tool.timeout(None).unwrap();
        assert_eq!(duration, Duration::from_secs(30));
        assert_eq!(secs, DEFAULT_TIMEOUT_SECONDS);
    }

    #[tokio::test]
    async fn rejects_a_non_positive_timeout() {
        let tool = HttpRequestTool::new();
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let mut request = input("GET", "http://127.0.0.1:1");
            request.timeout = Some(bad);
            assert!(
                error_message(&tool, &request)
                    .await
                    .contains("timeout must be a positive"),
                "{bad}"
            );
        }
    }

    #[tokio::test]
    async fn throws_error_for_3xx_redirect_status_codes() {
        let raw: &'static str = Box::leak(
            "HTTP/1.1 301 Moved Permanently\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                .to_owned()
                .into_boxed_str(),
        );
        let server = serve(raw).await;
        let tool = HttpRequestTool::new();

        // No `location`, so the redirect policy cannot follow it and the status surfaces.
        let message = error_message(&tool, &input("GET", &server.base_url)).await;
        assert!(message.contains("HTTP 301"), "{message}");
    }

    #[tokio::test]
    async fn throws_error_for_4xx_client_error_codes() {
        let raw: &'static str = Box::leak(response(404, "Not Found", "missing").into_boxed_str());
        let server = serve(raw).await;
        let tool = HttpRequestTool::new();

        let message = error_message(&tool, &input("GET", &server.base_url)).await;
        assert!(message.starts_with("HTTP 404 Not Found: GET "), "{message}");
    }

    #[tokio::test]
    async fn throws_error_for_5xx_server_error_codes() {
        let raw: &'static str = Box::leak(response(500, "Internal Server Error", "boom").into_boxed_str());
        let server = serve(raw).await;
        let tool = HttpRequestTool::new();

        let message = error_message(&tool, &input("GET", &server.base_url)).await;
        assert!(message.contains("HTTP 500"), "{message}");
    }

    #[tokio::test]
    async fn throws_timeout_error_when_request_exceeds_timeout() {
        let url = serve_forever().await;
        let tool = HttpRequestTool::new();

        let mut request = input("GET", &url);
        request.timeout = Some(0.05);

        let message = error_message(&tool, &request).await;
        assert_eq!(
            message,
            format!("Request timed out after 0.05 seconds: GET {url}")
        );
    }

    #[tokio::test]
    async fn throws_error_for_network_failures() {
        let url = dead_url().await;
        let tool = HttpRequestTool::new();

        let message = error_message(&tool, &input("GET", &url)).await;
        assert!(
            message.starts_with(&format!("Request failed: GET {url}")),
            "{message}"
        );
    }

    #[tokio::test]
    async fn rejects_an_unsupported_method() {
        let tool = HttpRequestTool::new();
        let message = error_message(&tool, &input("TRACE", "http://127.0.0.1:1")).await;
        assert!(message.contains("method must be one of"), "{message}");
    }

    #[tokio::test]
    async fn rejects_a_url_that_is_not_a_url() {
        let tool = HttpRequestTool::new();
        let message = error_message(&tool, &input("GET", "not-a-url")).await;
        assert!(message.contains("url must be a valid URL"), "{message}");
    }

    #[tokio::test]
    async fn invoke_returns_the_response_as_a_tool_result() {
        let raw: &'static str = Box::leak(response(200, "OK", "{\"success\":true}").into_boxed_str());
        let server = serve(raw).await;
        let tool = HttpRequestTool::new();

        let result = tool
            .invoke(test_context(
                HTTP_REQUEST_TOOL_NAME,
                "test-id",
                serde_json::json!({ "method": "GET", "url": server.base_url, "timeout": 5 }),
            ))
            .await
            .unwrap();

        assert_eq!(result.status, ToolResultStatus::Success);
        let serialized = serde_json::to_value(&result).unwrap();
        let output = &serialized["content"][0]["json"];
        assert_eq!(output["status"], 200);
        assert_eq!(output["statusText"], "OK");
        assert_eq!(output["body"], "{\"success\":true}");
    }

    #[tokio::test]
    async fn invoke_reports_a_failed_request_as_an_error_result() {
        let raw: &'static str = Box::leak(response(404, "Not Found", "missing").into_boxed_str());
        let server = serve(raw).await;
        let tool = HttpRequestTool::new();

        let result = tool
            .invoke(test_context(
                HTTP_REQUEST_TOOL_NAME,
                "test-id",
                serde_json::json!({ "method": "GET", "url": server.base_url, "timeout": 5 }),
            ))
            .await
            .unwrap();

        assert_eq!(result.status, ToolResultStatus::Error);
        assert!(result.text().contains("HTTP 404 Not Found"), "{}", result.text());
    }

    #[tokio::test]
    async fn uses_the_cancellation_token_mid_request() {
        // TS: `uses context.cancelSignal when it differs from the agent signal`.
        let url = serve_forever().await;
        let tool = HttpRequestTool::new();
        let context = test_context(
            HTTP_REQUEST_TOOL_NAME,
            "task-http",
            serde_json::json!({ "method": "GET", "url": url }),
        );
        let cancel = context.cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            cancel.cancel();
        });

        let err = tool.invoke(context).await.expect_err("expected cancellation");
        assert!(matches!(err, Error::Cancelled), "{err}");
    }

    #[tokio::test]
    async fn rejects_immediately_when_the_token_is_already_cancelled() {
        let tool = HttpRequestTool::new();
        let context = test_context(
            HTTP_REQUEST_TOOL_NAME,
            "task-http",
            serde_json::json!({ "method": "GET", "url": "http://127.0.0.1:1" }),
        );
        context.cancel.cancel();

        let err = tool.invoke(context).await.expect_err("expected cancellation");
        assert!(matches!(err, Error::Cancelled), "{err}");
    }

    #[test]
    fn spec_matches_the_ts_schema() {
        let tool = HttpRequestTool::new();
        assert_eq!(tool.name(), "http_request");
        assert_eq!(tool.description(), DEFAULT_HTTP_REQUEST_DESCRIPTION);

        let schema = tool.spec().input_schema.clone().unwrap();
        assert_eq!(schema["required"], serde_json::json!(["method", "url"]));
        assert_eq!(
            schema["properties"]["method"]["enum"],
            serde_json::json!(["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"])
        );
        assert_eq!(
            schema["properties"]["timeout"]["description"],
            "Optional timeout in seconds (default: 30)"
        );
    }
}
