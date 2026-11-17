//! `web_search`: search the web and return the top results as numbered sources (TS
//! `tools/web-search.ts`).
//!
//! The opt-in Exa tool, backed by Exa's hosted MCP server: a third party that receives the
//! queries. The keyless free tier covers getting started and `EXA_API_KEY` lifts the rate limit.
//! The tool and backend are separate so the model-facing contract stays fixed while the search
//! provider behind it can change.
#![cfg_attr(
    all(not(feature = "mcp"), not(test)),
    expect(
        dead_code,
        reason = "the Exa tool is built over the SDK's MCP client, behind the `mcp` feature"
    )
)]

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use async_trait::async_trait;
use futures::future::BoxFuture;
use regex::Regex;
use serde::Deserialize;
use strands::JsonValue;
use strands::tools::function_tool::InputValidator;
use strands::tools::{FunctionTool, ToolContext};

use super::web_fetch::cap_chars;
use crate::env::EnvLookup;
use crate::text::{js_is_whitespace, js_trim};

const EXA_MCP_URL: &str = "https://mcp.exa.ai/mcp";
const EXA_TOOL: &str = "web_search_exa";
const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESULTS: i64 = 10;
const SNIPPET_CHARS: usize = 500;
const HIGHLIGHTS: &str = "Highlights:";

const DESCRIPTION: &str =
    "Search the web and return the top results as numbered sources with a short excerpt each.";

/// One search hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchResult {
    pub(crate) title: String,
    pub(crate) url: String,
    pub(crate) snippet: String,
}

/// A backend failure with a message the model can act on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub(crate) struct WebSearchError(pub(crate) String);

/// Runs a query for at most the given number of results.
pub(crate) type SearchBackend =
    Arc<dyn Fn(String, i64) -> BoxFuture<'static, Result<Vec<SearchResult>, WebSearchError>> + Send + Sync>;

// The line terminators JS `.` does not match.
static HEAD: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^\s*Title: ([^\n\r\u{2028}\u{2029}]*)\r?\nURL: (\S+)").ok());
static SEPARATOR: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"(?mR)^---\s*$").ok());

/// JS `text.replace(/\s+/g, ' ').trim()`.
fn one_line(text: &str) -> String {
    text.split(js_is_whitespace)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Splits Exa's `---`-separated `Title:/URL:/.../Highlights:` blocks into results. Only a block
/// that opens with a `Title:` line directly followed by a `URL:` line counts, so those words
/// inside a page's highlighted text do not start a source.
fn parse_exa(text: &str) -> Vec<SearchResult> {
    let (Some(separator), Some(head)) = (SEPARATOR.as_ref(), HEAD.as_ref()) else {
        return Vec::new();
    };
    separator
        .split(text)
        .filter_map(|block| {
            let head = head.captures(block)?;
            let highlights = block.split_once(HIGHLIGHTS).map_or("", |(_, rest)| rest);
            Some(SearchResult {
                title: js_trim(&head[1]).to_owned(),
                url: head[2].to_owned(),
                snippet: cap_chars(one_line(highlights), SNIPPET_CHARS),
            })
        })
        .collect()
}

/// How to reach Exa's MCP server (the TS `McpClient` config).
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ExaConnection {
    pub(crate) url: &'static str,
    pub(crate) allowed_tools: Vec<&'static str>,
    /// Sent as the `x-api-key` header.
    pub(crate) api_key: Option<String>,
}

impl std::fmt::Debug for ExaConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExaConnection")
            .field("url", &self.url)
            .field("allowed_tools", &self.allowed_tools)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// The MCP client operations a search needs; failures carry the error's message.
#[async_trait]
pub(crate) trait ExaClient: Send + Sync {
    /// The names of the tools the server offers.
    async fn list_tools(&self) -> Result<Vec<String>, String>;
    /// Calls the tool `name` and returns the MCP result (`content`, `isError`).
    async fn call_tool(&self, name: &str, args: JsonValue, timeout: Duration) -> Result<JsonValue, String>;
    async fn disconnect(&self) -> Result<(), String>;
}

/// Makes the MCP client for one search; tests substitute a fake.
pub(crate) type ExaConnector = Arc<dyn Fn(ExaConnection) -> Result<Box<dyn ExaClient>, String> + Send + Sync>;

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct ExaResult {
    content: Option<Vec<JsonValue>>,
    is_error: Option<bool>,
}

async fn exa_request(
    client: &dyn ExaClient,
    query: &str,
    max_results: i64,
) -> Result<JsonValue, WebSearchError> {
    let tools = client.list_tools().await.map_err(|reason| {
        WebSearchError(format!(
            "could not reach Exa's MCP server at {EXA_MCP_URL} ({reason})"
        ))
    })?;
    if !tools.iter().any(|name| name == EXA_TOOL) {
        return Err(WebSearchError(format!(
            "Exa's MCP server does not offer {EXA_TOOL}"
        )));
    }
    let args = serde_json::json!({ "query": query, "numResults": max_results });
    client
        .call_tool(EXA_TOOL, args, TIMEOUT)
        .await
        .map_err(WebSearchError)
}

/// One `web_search_exa` call over an MCP client; the session lives only for this call.
async fn exa_call(
    connector: &ExaConnector,
    query: &str,
    max_results: i64,
    api_key: Option<String>,
) -> Result<Vec<SearchResult>, WebSearchError> {
    let client = connector(ExaConnection {
        url: EXA_MCP_URL,
        allowed_tools: vec![EXA_TOOL],
        api_key,
    })
    .map_err(WebSearchError)?;
    let result = exa_request(client.as_ref(), query, max_results).await;
    // A teardown error must not discard a result already in hand.
    let _ = client.disconnect().await;
    let result: ExaResult = serde_json::from_value(result?).unwrap_or_default();
    let text = result
        .content
        .unwrap_or_default()
        .iter()
        .filter_map(|item| item.get("text")?.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if result.is_error.unwrap_or(false) {
        return Err(WebSearchError(if text.is_empty() {
            "Exa reported an error".to_owned()
        } else {
            text
        }));
    }
    Ok(parse_exa(&text))
}

/// The search backend over Exa's hosted MCP server; `api_key` defaults to `EXA_API_KEY`, read
/// from `env` on every call.
pub(crate) fn exa_backend(api_key: Option<String>, env: EnvLookup, connector: ExaConnector) -> SearchBackend {
    Arc::new(move |query, max_results| {
        let key = api_key
            .clone()
            .or_else(|| env("EXA_API_KEY"))
            .filter(|key| !key.is_empty());
        let connector = Arc::clone(&connector);
        Box::pin(async move {
            let keyless = key.is_none();
            exa_call(&connector, &query, max_results, key)
                .await
                .map_err(|error| {
                    if keyless {
                        // The MCP client hides the HTTP status, so a keyless 429 can't be told apart.
                        WebSearchError(format!(
                            "{error} (Exa's keyless tier is rate limited; set EXA_API_KEY to lift it.)"
                        ))
                    } else {
                        error
                    }
                })
        })
    })
}

#[derive(Deserialize)]
struct SearchInput {
    query: String,
    max_results: Option<i64>,
}

/// Builds a `web_search` tool that formats `backend`'s results as a numbered `Sources:` list.
pub(crate) fn search_tool(backend: SearchBackend) -> FunctionTool {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "What to search for. A descriptive sentence works better than bare keywords.",
            },
            "max_results": {
                "description": "How many results to return, 1-10.",
                "type": "integer",
                "minimum": -9_007_199_254_740_991_i64,
                "maximum": 9_007_199_254_740_991_i64,
            },
        },
        "required": ["query"],
        "additionalProperties": false,
    });
    let validator = InputValidator::new(&schema);
    FunctionTool::new(
        "web_search",
        DESCRIPTION,
        move |input: JsonValue, _context: ToolContext| {
            let parsed = validator.parse::<SearchInput>(input);
            let backend = Arc::clone(&backend);
            async move {
                let input = parsed?;
                let max_results = input.max_results.unwrap_or(5).clamp(1, MAX_RESULTS);
                let results = match backend(input.query, max_results).await {
                    Ok(results) => results,
                    Err(WebSearchError(message)) => {
                        // TS `err.message || err.name`.
                        let reason = if message.is_empty() { "Error" } else { &message };
                        return Ok(format!("web_search failed: {reason}"));
                    }
                };
                if results.is_empty() {
                    return Ok("No results.".to_owned());
                }
                let mut lines = vec!["Sources:".to_owned()];
                for (index, result) in results.iter().enumerate() {
                    lines.push(format!(
                        "{}. {} — {}",
                        index + 1,
                        one_line(&result.title),
                        result.url
                    ));
                    if !result.snippet.is_empty() {
                        lines.push(format!("   {}", one_line(&result.snippet)));
                    }
                }
                Ok(lines.join("\n"))
            }
        },
    )
    .with_input_schema(schema)
}

#[cfg(feature = "mcp")]
mod exa_mcp {
    //! The Exa client over the SDK's `McpClient`.

    use std::sync::{Mutex, PoisonError};

    use indexmap::IndexMap;
    use strands::mcp::{
        McpCallToolOptions, McpClient, McpClientConfig, McpClientOptions, McpTool, McpToolFilters,
    };
    use strands::tools::Tool;

    use super::{Arc, Duration, ExaClient, ExaConnection, ExaConnector, JsonValue, async_trait};

    pub(super) fn config(connection: ExaConnection) -> McpClientConfig {
        McpClientConfig {
            url: Some(connection.url.to_owned()),
            headers: connection
                .api_key
                .map(|key| IndexMap::from([("x-api-key".to_owned(), key)])),
            options: McpClientOptions {
                tool_filters: Some(McpToolFilters {
                    allowed: Some(connection.allowed_tools.into_iter().map(Into::into).collect()),
                    rejected: None,
                }),
                ..McpClientOptions::default()
            },
            ..McpClientConfig::default()
        }
    }

    struct McpExaClient {
        client: McpClient,
        tools: Mutex<Vec<McpTool>>,
    }

    #[async_trait]
    impl ExaClient for McpExaClient {
        async fn list_tools(&self) -> Result<Vec<String>, String> {
            let tools = self
                .client
                .list_tools()
                .await
                .map_err(|error| error.to_string())?;
            let names = tools.iter().map(|tool| tool.name().to_owned()).collect();
            *self.tools.lock().unwrap_or_else(PoisonError::into_inner) = tools;
            Ok(names)
        }

        async fn call_tool(
            &self,
            name: &str,
            args: JsonValue,
            timeout: Duration,
        ) -> Result<JsonValue, String> {
            let tool = self
                .tools
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .find(|tool| tool.name() == name)
                .cloned()
                .ok_or_else(|| format!("Exa's MCP server does not offer {name}"))?;
            let options = McpCallToolOptions {
                timeout: Some(timeout),
                ..McpCallToolOptions::default()
            };
            self.client
                .call_tool_with(&tool, args, options)
                .await
                .map_err(|error| error.to_string())
        }

        async fn disconnect(&self) -> Result<(), String> {
            self.client.disconnect().await.map_err(|error| error.to_string())
        }
    }

    /// Connects with the SDK's MCP client.
    pub(super) fn connector() -> ExaConnector {
        Arc::new(|connection| {
            let client = McpClient::new(config(connection)).map_err(|error| error.to_string())?;
            Ok(Box::new(McpExaClient {
                client,
                tools: Mutex::default(),
            }))
        })
    }
}

/// Builds a `web_search` tool over Exa's hosted search (TS `makeExaWebSearch`). `EXA_API_KEY`,
/// read on every call, lifts the keyless rate limit.
#[cfg(feature = "mcp")]
pub fn make_exa_web_search() -> FunctionTool {
    search_tool(exa_backend(None, crate::env::process_env(), exa_mcp::connector()))
}

/// The `web_search` tool served by Exa (TS `exaWebSearch`).
#[cfg(feature = "mcp")]
pub fn exa_web_search() -> FunctionTool {
    make_exa_web_search()
}

#[cfg(test)]
mod tests;
