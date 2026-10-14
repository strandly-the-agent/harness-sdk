//! `web_search`: search the web and return the top results as numbered sources
//! (`harness-ts/src/tools/web-search.ts`).
//!
//! The opt-in Exa tool, backed by Exa's hosted MCP server — a third party that receives the
//! queries. The keyless free tier covers getting started and `EXA_API_KEY` lifts the rate limit.
//!
//! TS calls the server through the SDK's `McpClient`; the Rust SDK has no MCP client yet, so
//! [`ExaBackend`] speaks the same JSON-RPC `tools/call` over streamable HTTP through an injectable
//! [`ExaTransport`] (default `reqwest`). The model-facing contract — tool name, description,
//! schema, result formatting and error text — is verbatim.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use strands_agents::tools::FunctionTool;

const EXA_MCP_URL: &str = "https://mcp.exa.ai/mcp";
const EXA_TOOL: &str = "web_search_exa";
const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESULTS: u32 = 10;
const DEFAULT_RESULTS: u32 = 5;
const SNIPPET_CHARS: usize = 500;

const WEB_SEARCH_DESCRIPTION: &str =
    "Search the web and return the top results as numbered sources with a short excerpt each.";

/// One search result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchResult {
    /// Page title.
    pub title: String,
    /// Page URL.
    pub url: String,
    /// Short excerpt.
    pub snippet: String,
}

/// A backend failure with a message the model can act on.
#[derive(Clone, Debug, thiserror::Error)]
#[error("{0}")]
pub struct WebSearchError(pub String);

impl WebSearchError {
    /// A failure with `message`.
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

/// The search seam behind the `web_search` tool.
#[async_trait]
pub trait SearchBackend: Send + Sync {
    /// Run `query`, returning at most `max_results` results.
    async fn search(&self, query: &str, max_results: u32) -> Result<Vec<SearchResult>, WebSearchError>;
}

/// The HTTP seam [`ExaBackend`] posts MCP JSON-RPC through.
#[async_trait]
pub trait ExaTransport: Send + Sync {
    /// POST `body` to Exa's MCP endpoint and return the decoded JSON-RPC response.
    async fn post(
        &self,
        body: serde_json::Value,
        api_key: Option<&str>,
    ) -> Result<serde_json::Value, WebSearchError>;
}

/// Split Exa's `---`-separated `Title:/URL:/.../Highlights:` blocks into results. Only a block that
/// opens with a `Title:` line directly followed by a `URL:` line counts, so those words inside a
/// page's highlighted text do not start a source.
pub(crate) fn parse_exa(text: &str) -> Vec<SearchResult> {
    let mut results = Vec::new();
    for block in split_separator(text) {
        let Some((title, url)) = block_head(&block) else {
            continue;
        };
        let highlights = match block.find("Highlights:") {
            Some(at) => &block[at + "Highlights:".len()..],
            None => "",
        };
        let collapsed = highlights.split_whitespace().collect::<Vec<_>>().join(" ");
        let snippet: String = collapsed.chars().take(SNIPPET_CHARS).collect();
        results.push(SearchResult { title, url, snippet });
    }
    results
}

/// `text.split(/^---\s*$/m)`.
fn split_separator(text: &str) -> Vec<String> {
    let mut blocks = vec![String::new()];
    for line in text.split('\n') {
        let trimmed = line.trim_end_matches('\r');
        if trimmed.starts_with("---") && trimmed[3..].trim().is_empty() {
            blocks.push(String::new());
        } else {
            let current = blocks.last_mut().expect("at least one block");
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(line);
        }
    }
    blocks
}

/// `/^\s*Title: (.*)\r?\nURL: (\S+)/`.
fn block_head(block: &str) -> Option<(String, String)> {
    let body = block.trim_start();
    let title_line = body.strip_prefix("Title: ")?;
    let (title, rest) = match title_line.split_once('\n') {
        Some((title, rest)) => (title.trim_end_matches('\r'), rest),
        None => return None,
    };
    let url = rest.strip_prefix("URL: ")?.split_whitespace().next()?;
    Some((title.trim().to_string(), url.to_string()))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct WebSearchInput {
    query: String,
    #[serde(default)]
    max_results: Option<i64>,
}

/// Build a `web_search` tool that formats `backend`'s results as a numbered `Sources:` list.
pub fn search_tool(backend: Arc<dyn SearchBackend>) -> FunctionTool {
    FunctionTool::builder("web_search")
        .description(WEB_SEARCH_DESCRIPTION)
        .input_schema(serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "What to search for. A descriptive sentence works better than bare keywords.",
                },
                "max_results": {
                    "type": "integer",
                    "description": "How many results to return, 1-10.",
                },
            },
            "required": ["query"],
            "additionalProperties": false,
        }))
        .typed_handler(move |input: WebSearchInput, _ctx| {
            let backend = Arc::clone(&backend);
            async move {
                let requested = input.max_results.unwrap_or(DEFAULT_RESULTS as i64);
                let max_results = requested.min(MAX_RESULTS as i64).max(1) as u32;
                let results = match backend.search(&input.query, max_results).await {
                    Ok(results) => results,
                    Err(err) => return Ok(format!("web_search failed: {err}")),
                };
                if results.is_empty() {
                    return Ok("No results.".to_string());
                }
                let one_line = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
                let mut lines = vec!["Sources:".to_string()];
                for (index, result) in results.iter().enumerate() {
                    lines.push(format!("{}. {} — {}", index + 1, one_line(&result.title), result.url));
                    if !result.snippet.is_empty() {
                        lines.push(format!("   {}", one_line(&result.snippet)));
                    }
                }
                Ok(lines.join("\n"))
            }
        })
}

/// Search backend over Exa's hosted MCP server; `api_key` defaults to `EXA_API_KEY` (read per call).
pub struct ExaBackend {
    api_key: Option<String>,
    transport: Arc<dyn ExaTransport>,
}

impl ExaBackend {
    /// Backend over the default `reqwest` transport.
    #[cfg(feature = "http")]
    pub fn new(api_key: Option<String>) -> Self {
        Self {
            api_key,
            transport: Arc::new(ReqwestExaTransport),
        }
    }

    /// Backend over an injected transport (tests).
    pub fn with_transport(api_key: Option<String>, transport: Arc<dyn ExaTransport>) -> Self {
        Self { api_key, transport }
    }

    async fn call(
        &self,
        query: &str,
        max_results: u32,
        key: Option<&str>,
    ) -> Result<Vec<SearchResult>, WebSearchError> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": EXA_TOOL, "arguments": { "query": query, "numResults": max_results } },
        });
        let response = self.transport.post(body, key).await?;
        if let Some(error) = response.get("error") {
            let message = error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Exa reported an error");
            return Err(WebSearchError::new(message));
        }
        let result = response.get("result").unwrap_or(&serde_json::Value::Null);
        let text = result
            .get("content")
            .and_then(|c| c.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        if result.get("isError").and_then(|e| e.as_bool()).unwrap_or(false) {
            return Err(WebSearchError::new(if text.is_empty() {
                "Exa reported an error".to_string()
            } else {
                text
            }));
        }
        Ok(parse_exa(&text))
    }
}

#[async_trait]
impl SearchBackend for ExaBackend {
    async fn search(&self, query: &str, max_results: u32) -> Result<Vec<SearchResult>, WebSearchError> {
        let key = self
            .api_key
            .clone()
            .or_else(|| std::env::var("EXA_API_KEY").ok().filter(|k| !k.is_empty()));
        match self.call(query, max_results, key.as_deref()).await {
            Ok(results) => Ok(results),
            Err(err) if key.is_some() => Err(err),
            // The keyless tier is rate limited and the status is not distinguishable here.
            Err(err) => Err(WebSearchError::new(format!(
                "{err} (Exa's keyless tier is rate limited; set EXA_API_KEY to lift it.)"
            ))),
        }
    }
}

/// The default Exa transport: a `reqwest` POST to Exa's hosted MCP endpoint.
#[cfg(feature = "http")]
#[derive(Debug, Default)]
pub struct ReqwestExaTransport;

#[cfg(feature = "http")]
#[async_trait]
impl ExaTransport for ReqwestExaTransport {
    async fn post(
        &self,
        body: serde_json::Value,
        api_key: Option<&str>,
    ) -> Result<serde_json::Value, WebSearchError> {
        let client = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .build()
            .map_err(|e| WebSearchError::new(e.to_string()))?;
        let mut request = client
            .post(EXA_MCP_URL)
            .header("accept", "application/json, text/event-stream")
            .json(&body);
        if let Some(key) = api_key {
            request = request.header("x-api-key", key);
        }
        let response = request.send().await.map_err(|e| {
            WebSearchError::new(format!("could not reach Exa's MCP server at {EXA_MCP_URL} ({e})"))
        })?;
        let text = response
            .text()
            .await
            .map_err(|e| WebSearchError::new(e.to_string()))?;
        // Streamable HTTP may answer with SSE; take the first `data:` payload.
        let payload = text
            .lines()
            .find_map(|line| line.strip_prefix("data:"))
            .unwrap_or(&text)
            .trim();
        serde_json::from_str(payload).map_err(|e| {
            WebSearchError::new(format!("could not reach Exa's MCP server at {EXA_MCP_URL} ({e})"))
        })
    }
}

/// Build a `web_search` tool over Exa's hosted search (`EXA_API_KEY` lifts the keyless rate limit).
#[cfg(feature = "http")]
pub fn make_exa_web_search() -> FunctionTool {
    search_tool(Arc::new(ExaBackend::new(None)))
}

/// The `web_search` tool served by Exa (`EXA_API_KEY` is read per call).
#[cfg(feature = "http")]
pub fn exa_web_search() -> FunctionTool {
    make_exa_web_search()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_support::{invoke, text_of};
    use parking_lot::Mutex;

    type Calls = Arc<Mutex<Vec<(String, u32)>>>;
    type Posts = Arc<Mutex<Vec<(serde_json::Value, Option<String>)>>>;

    struct StubBackend {
        results: Vec<SearchResult>,
        calls: Calls,
    }

    #[async_trait]
    impl SearchBackend for StubBackend {
        async fn search(&self, query: &str, max_results: u32) -> Result<Vec<SearchResult>, WebSearchError> {
            self.calls.lock().push((query.to_string(), max_results));
            Ok(self.results.clone())
        }
    }

    struct FailingBackend;

    #[async_trait]
    impl SearchBackend for FailingBackend {
        async fn search(&self, _query: &str, _max: u32) -> Result<Vec<SearchResult>, WebSearchError> {
            Err(WebSearchError::new("backend exploded"))
        }
    }

    fn result(title: &str, url: &str, snippet: &str) -> SearchResult {
        SearchResult {
            title: title.into(),
            url: url.into(),
            snippet: snippet.into(),
        }
    }

    fn stub(results: Vec<SearchResult>) -> (FunctionTool, Calls) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let backend = StubBackend {
            results,
            calls: Arc::clone(&calls),
        };
        (search_tool(Arc::new(backend)), calls)
    }

    #[tokio::test]
    async fn formats_results_as_numbered_sources() {
        let (tool, _) = stub(vec![
            result("First  page", "https://a.example", "a  snippet\nhere"),
            result("Second", "https://b.example", ""),
        ]);
        let out = invoke(&tool, serde_json::json!({ "query": "q" })).await;
        assert_eq!(
            text_of(&out),
            "Sources:\n1. First page — https://a.example\n   a snippet here\n2. Second — https://b.example"
        );
    }

    #[tokio::test]
    async fn reports_no_results() {
        let (tool, _) = stub(vec![]);
        let out = invoke(&tool, serde_json::json!({ "query": "q" })).await;
        assert_eq!(text_of(&out), "No results.");
    }

    #[tokio::test]
    async fn reports_a_backend_failure() {
        let tool = search_tool(Arc::new(FailingBackend));
        let out = invoke(&tool, serde_json::json!({ "query": "q" })).await;
        assert_eq!(text_of(&out), "web_search failed: backend exploded");
    }

    #[tokio::test]
    async fn max_results_defaults_to_five() {
        let (tool, calls) = stub(vec![]);
        invoke(&tool, serde_json::json!({ "query": "q" })).await;
        assert_eq!(calls.lock()[0].1, 5);
    }

    #[tokio::test]
    async fn max_results_is_clamped_to_one_through_ten() {
        let (tool, calls) = stub(vec![]);
        invoke(&tool, serde_json::json!({ "query": "q", "max_results": 99 })).await;
        invoke(&tool, serde_json::json!({ "query": "q", "max_results": 0 })).await;
        invoke(&tool, serde_json::json!({ "query": "q", "max_results": 7 })).await;
        let calls = calls.lock();
        assert_eq!(
            calls.iter().map(|c| c.1).collect::<Vec<_>>(),
            vec![MAX_RESULTS, 1, 7]
        );
    }

    #[test]
    fn parses_exa_blocks() {
        let text = "Title: One\nURL: https://one.example\nText: x\nHighlights:\n  first  highlight\n---\nTitle: Two\nURL: https://two.example\nHighlights: second\n";
        let results = parse_exa(text);
        assert_eq!(
            results,
            vec![
                result("One", "https://one.example", "first highlight"),
                result("Two", "https://two.example", "second"),
            ]
        );
    }

    #[test]
    fn ignores_a_block_without_a_title_url_head() {
        assert_eq!(parse_exa("Text: Title: fake\nURL: https://x.example\n"), vec![]);
    }

    #[test]
    fn truncates_a_long_snippet() {
        let text = format!(
            "Title: T\nURL: https://x.example\nHighlights: {}\n",
            "a".repeat(600)
        );
        assert_eq!(parse_exa(&text)[0].snippet.chars().count(), SNIPPET_CHARS);
    }

    struct StubExaTransport {
        response: serde_json::Value,
        seen: Posts,
    }

    #[async_trait]
    impl ExaTransport for StubExaTransport {
        async fn post(
            &self,
            body: serde_json::Value,
            api_key: Option<&str>,
        ) -> Result<serde_json::Value, WebSearchError> {
            self.seen.lock().push((body, api_key.map(str::to_string)));
            Ok(self.response.clone())
        }
    }

    fn exa_response(text: &str, is_error: bool) -> serde_json::Value {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "content": [{ "type": "text", "text": text }], "isError": is_error },
        })
    }

    #[tokio::test]
    async fn exa_backend_sends_the_mcp_tool_call_and_parses_the_reply() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let backend = ExaBackend::with_transport(
            Some("key-123".into()),
            Arc::new(StubExaTransport {
                response: exa_response("Title: One\nURL: https://one.example\nHighlights: hi", false),
                seen: Arc::clone(&seen),
            }),
        );
        let results = backend.search("rust agents", 3).await.unwrap();
        assert_eq!(results, vec![result("One", "https://one.example", "hi")]);
        let (body, key) = seen.lock()[0].clone();
        assert_eq!(body["method"], "tools/call");
        assert_eq!(body["params"]["name"], EXA_TOOL);
        assert_eq!(body["params"]["arguments"]["query"], "rust agents");
        assert_eq!(body["params"]["arguments"]["numResults"], 3);
        assert_eq!(key.as_deref(), Some("key-123"));
    }

    #[tokio::test]
    async fn exa_backend_surfaces_a_tool_error() {
        let backend = ExaBackend::with_transport(
            Some("key".into()),
            Arc::new(StubExaTransport {
                response: exa_response("rate limited", true),
                seen: Arc::new(Mutex::new(Vec::new())),
            }),
        );
        let err = backend.search("q", 1).await.unwrap_err();
        assert_eq!(err.to_string(), "rate limited");
    }

    struct BrokenTransport;

    #[async_trait]
    impl ExaTransport for BrokenTransport {
        async fn post(
            &self,
            _body: serde_json::Value,
            _api_key: Option<&str>,
        ) -> Result<serde_json::Value, WebSearchError> {
            Err(WebSearchError::new("connection refused"))
        }
    }

    #[tokio::test]
    async fn a_keyless_failure_mentions_the_rate_limit() {
        let backend = ExaBackend::with_transport(None, Arc::new(BrokenTransport));
        // Only meaningful without an ambient key; skip when the environment provides one.
        if std::env::var("EXA_API_KEY").is_ok() {
            return;
        }
        let err = backend.search("q", 1).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "connection refused (Exa's keyless tier is rate limited; set EXA_API_KEY to lift it.)"
        );
    }

    #[test]
    fn spec_matches_typescript() {
        use strands_agents::tools::Tool;
        let (tool, _) = stub(vec![]);
        assert_eq!(tool.name(), "web_search");
        assert_eq!(tool.description(), WEB_SEARCH_DESCRIPTION);
        assert_eq!(
            tool.spec().input_schema.clone().unwrap()["required"],
            serde_json::json!(["query"])
        );
    }
}
