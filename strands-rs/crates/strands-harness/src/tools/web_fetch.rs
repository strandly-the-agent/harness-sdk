//! `web_fetch`: fetch a URL and answer a prompt about its content
//! (`harness-ts/src/tools/web-fetch.ts`).
//!
//! Fetches the URL, reduces it to text, and asks a small fast model to answer the prompt over that
//! content, returning the answer rather than the raw page so large payloads never reach the main
//! agent's context.
//!
//! The TS tool runs the request as `curl` inside `context.agent.sandbox` by default
//! (`transport: 'direct'` opts into a process-local fetch). The Rust SDK has no sandbox seam, so
//! the transport is a [`WebFetchTransport`] trait and the default implementation is the `reqwest`
//! equivalent of TS's `'direct'`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::Deserialize;
use strands_agents::models::Model;
use strands_agents::tools::FunctionTool;
use strands_agents::{Agent, Error, Result};

const USER_AGENT: &str = "strands-harness/1.0";
const TIMEOUT_SECONDS: u64 = 30;
const MAX_BYTES: usize = 5 * 1024 * 1024;
const MAX_CHARS: usize = 50_000;
const CACHE_TTL: Duration = Duration::from_secs(15 * 60);

/// The characters RFC 3986 allows anywhere in a URL; anything else (whitespace, quotes, control
/// characters, non-ASCII) is rejected before the URL reaches a socket.
fn is_url_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "-._~:/?#[]@!$&'()*+,;=%".contains(c)
}

const SUMMARIZER_PROMPT: &str = concat!(
    "You answer a request about a single fetched web page. Use only the provided content; if it ",
    "does not contain the answer, say so plainly. Be concise and factual, and preserve concrete ",
    "details (names, numbers, quotes, links) relevant to the request."
);

const WEB_FETCH_DESCRIPTION: &str = concat!(
    "Fetch a URL and answer a prompt about its content. Performs an HTTP GET, reduces the ",
    "response to text, and asks a small fast model to answer the prompt using only that content, ",
    "returning the answer rather than the raw page so large pages do not flood the conversation. ",
    "Fetched content is cached for 15 minutes, so repeated fetches of the same URL are fast while ",
    "each prompt is answered fresh."
);

/// A fetched response, before text reduction.
#[derive(Clone, Debug, Default)]
pub struct FetchedPage {
    /// The final hop's URL (empty falls back to the requested URL).
    pub resolved_url: String,
    /// The `content-type` header, or empty when absent.
    pub content_type: String,
    /// At most 5 MiB of the body.
    pub body: Vec<u8>,
}

/// The HTTP seam `web_fetch` fetches through. The URL is already validated.
#[async_trait]
pub trait WebFetchTransport: Send + Sync {
    /// GET `url`, returning at most 5 MiB of body.
    async fn fetch(&self, url: &str) -> Result<FetchedPage>;
}

/// The default transport: a plain `reqwest` GET from the harness process.
#[derive(Debug, Default)]
pub struct ReqwestTransport;

#[async_trait]
impl WebFetchTransport for ReqwestTransport {
    async fn fetch(&self, url: &str) -> Result<FetchedPage> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(TIMEOUT_SECONDS))
            .build()
            .map_err(|e| Error::other(e.to_string()))?;
        let response = client.get(url).send().await.map_err(|e| {
            if e.is_timeout() {
                Error::other(format!("web_fetch gave up after {TIMEOUT_SECONDS}s."))
            } else {
                Error::other(e.to_string())
            }
        })?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::other(format!(
                "HTTP {} {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("")
            )));
        }
        let resolved_url = response.url().to_string();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let mut body = response
            .bytes()
            .await
            .map_err(|e| Error::other(e.to_string()))?
            .to_vec();
        body.truncate(MAX_BYTES);
        Ok(FetchedPage {
            resolved_url,
            content_type,
            body,
        })
    }
}

/// Strip `<script>`/`<style>` bodies and all tags, then collapse whitespace (the TS `htmlToText`).
pub(crate) fn html_to_text(html: &str) -> String {
    let stripped = strip_elements(html, &["script", "style"]);
    let no_tags = strip_tags(&stripped);
    collapse_blank_lines(&collapse_spaces(&no_tags))
        .trim()
        .to_string()
}

fn strip_elements(input: &str, tags: &[&str]) -> String {
    let lower = input.to_lowercase();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;
    while i < input.len() {
        let Some(open) = lower[i..].find('<').map(|p| i + p) else {
            out.push_str(&input[i..]);
            break;
        };
        let matched = tags.iter().find_map(|tag| {
            let rest = &lower[open + 1..];
            if !rest.starts_with(tag) {
                return None;
            }
            let after = rest[tag.len()..].chars().next();
            // `<script>` / `<script src=...>`, but not `<scripted>`
            if !matches!(after, Some(c) if c == '>' || c.is_whitespace() || c == '/') {
                return None;
            }
            let close = format!("</{tag}");
            let body_start = lower[open..].find('>').map(|p| open + p + 1)?;
            let close_at = lower[body_start..].find(&close).map(|p| body_start + p)?;
            let end = lower[close_at..].find('>').map(|p| close_at + p + 1)?;
            Some(end)
        });
        out.push_str(&input[i..open]);
        match matched {
            Some(end) => {
                out.push(' ');
                i = end;
            }
            None => {
                out.push('<');
                i = open + 1;
            }
        }
    }
    out
}

fn strip_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(open) = rest.find('<') {
        match rest[open..].find('>') {
            // `<[^>]+>`: needs at least one character between the brackets
            Some(close) if close > 1 => {
                out.push_str(&rest[..open]);
                out.push(' ');
                rest = &rest[open + close + 1..];
            }
            _ => {
                out.push_str(&rest[..=open]);
                rest = &rest[open + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn collapse_spaces(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_run = false;
    for c in input.chars() {
        if c == ' ' || c == '\t' {
            if !in_run {
                out.push(' ');
            }
            in_run = true;
        } else {
            in_run = false;
            out.push(c);
        }
    }
    out
}

/// `\n\s*\n\s*\n+` → `\n\n`: a whitespace run holding three or more newlines becomes one blank line.
fn collapse_blank_lines(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if !chars[i].is_whitespace() {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        let run = &chars[start..i];
        let newlines = run.iter().filter(|c| **c == '\n').count();
        if newlines < 3 {
            out.extend(run.iter());
            continue;
        }
        let first = run.iter().position(|c| *c == '\n').unwrap();
        let last = run.iter().rposition(|c| *c == '\n').unwrap();
        out.extend(run[..first].iter());
        out.push_str("\n\n");
        out.extend(run[last + 1..].iter());
    }
    out
}

/// Return `url` trimmed, or an error unless it is a well-formed http(s) URL.
fn validate_url(url: &str) -> Result<String> {
    let url = url.trim();
    if url.is_empty() || !url.chars().all(is_url_char) {
        return Err(Error::other(
            "web_fetch URLs may only contain the characters RFC 3986 allows; percent-encode spaces \
             and non-ASCII characters (and punycode the host) and retry."
                .to_string(),
        ));
    }
    let lower = url.to_lowercase();
    let scheme_len = if lower.starts_with("http://") {
        7
    } else if lower.starts_with("https://") {
        8
    } else {
        return if lower.contains(':') {
            Err(Error::other(format!(
                "web_fetch only supports http(s) URLs, got {}.",
                quote(url)
            )))
        } else {
            Err(Error::other(format!(
                "web_fetch could not parse the URL {}.",
                quote(url)
            )))
        };
    };
    // The authority must be non-empty: `https:///host` has no host.
    let authority = &url[scheme_len..];
    if authority.is_empty() || authority.starts_with(['/', '?', '#']) {
        return Err(Error::other(format!(
            "web_fetch URL has no host: {}.",
            quote(url)
        )));
    }
    Ok(url.to_string())
}

fn quote(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| format!("\"{value}\""))
}

fn to_text(page: &FetchedPage, url: &str) -> (String, String) {
    let raw = String::from_utf8_lossy(&page.body);
    let text = if page.content_type.to_lowercase().contains("html") {
        html_to_text(&raw)
    } else {
        raw.to_string()
    };
    let resolved = if page.resolved_url.trim().is_empty() {
        url.to_string()
    } else {
        page.resolved_url.trim().to_string()
    };
    (resolved, text.chars().take(MAX_CHARS).collect())
}

/// Options for [`make_web_fetch`].
pub struct MakeWebFetchOptions {
    /// The summarizer that answers the prompt over the fetched page.
    pub model: Arc<dyn Model>,
    /// The HTTP seam; defaults to [`ReqwestTransport`].
    pub transport: Option<Arc<dyn WebFetchTransport>>,
}

impl MakeWebFetchOptions {
    /// Options with the default transport.
    pub fn new(model: Arc<dyn Model>) -> Self {
        Self {
            model,
            transport: None,
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct WebFetchInput {
    url: String,
    #[serde(default)]
    prompt: Option<String>,
}

struct CacheEntry {
    expires: Instant,
    resolved_url: String,
    text: String,
}

/// Build a `web_fetch` tool whose summarizer answers over the fetched page using `options.model`.
pub fn make_web_fetch(options: MakeWebFetchOptions) -> FunctionTool {
    let model = options.model;
    let transport: Arc<dyn WebFetchTransport> = options
        .transport
        .unwrap_or_else(|| Arc::new(ReqwestTransport) as Arc<dyn WebFetchTransport>);
    let cache: Arc<Mutex<HashMap<String, CacheEntry>>> = Arc::new(Mutex::new(HashMap::new()));

    FunctionTool::builder("web_fetch")
        .description(WEB_FETCH_DESCRIPTION)
        .input_schema(serde_json::json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "The http(s) URL to fetch." },
                "prompt": {
                    "type": "string",
                    "description": "What to extract from or answer about the fetched content. Leave empty to get the whole page content back verbatim instead of a model-generated answer.",
                },
            },
            "required": ["url"],
            "additionalProperties": false,
        }))
        .typed_handler(move |input: WebFetchInput, _ctx| {
            let model = Arc::clone(&model);
            let transport = Arc::clone(&transport);
            let cache = Arc::clone(&cache);
            async move {
                let cached = cache.lock().get(&input.url).and_then(|entry| {
                    (entry.expires > Instant::now()).then(|| (entry.resolved_url.clone(), entry.text.clone()))
                });
                let (resolved_url, text) = match cached {
                    Some(hit) => hit,
                    None => {
                        let fetched = async {
                            let url = validate_url(&input.url)?;
                            let page = transport.fetch(&url).await?;
                            Ok::<_, Error>(to_text(&page, &url))
                        }
                        .await;
                        match fetched {
                            Ok((resolved_url, text)) => {
                                cache.lock().insert(
                                    input.url.clone(),
                                    CacheEntry {
                                        expires: Instant::now() + CACHE_TTL,
                                        resolved_url: resolved_url.clone(),
                                        text: text.clone(),
                                    },
                                );
                                (resolved_url, text)
                            }
                            Err(err) => {
                                return Ok(format!("Failed to fetch {}: {}", input.url, error_message(&err)))
                            }
                        }
                    }
                };

                let prompt = input.prompt.unwrap_or_default();
                if prompt.trim().is_empty() {
                    return Ok(text);
                }

                let summarizer = Agent::builder()
                    .model_arc(model)
                    .system_prompt(SUMMARIZER_PROMPT)
                    .build()
                    .await?;
                let result = summarizer
                    .invoke(format!(
                        "Fetched URL: {resolved_url}\n\nRequest: {prompt}\n\n--- Content ---\n{text}"
                    ))
                    .await?;
                Ok(result.text())
            }
        })
}

fn error_message(err: &Error) -> String {
    err.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_support::{invoke, test_agent, text_of};
    use strands_agents::test_support::{MockMessageModel, MockResponse};
    use strands_agents::types::Message;

    struct StubTransport {
        page: FetchedPage,
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl StubTransport {
        fn html(body: &str) -> Self {
            Self {
                page: FetchedPage {
                    resolved_url: String::new(),
                    content_type: "text/html; charset=utf-8".into(),
                    body: body.as_bytes().to_vec(),
                },
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn text(body: &str) -> Self {
            Self {
                page: FetchedPage {
                    resolved_url: String::new(),
                    content_type: "text/plain".into(),
                    body: body.as_bytes().to_vec(),
                },
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl WebFetchTransport for StubTransport {
        async fn fetch(&self, url: &str) -> Result<FetchedPage> {
            self.calls.lock().push(url.to_string());
            Ok(self.page.clone())
        }
    }

    struct FailingTransport;

    #[async_trait]
    impl WebFetchTransport for FailingTransport {
        async fn fetch(&self, _url: &str) -> Result<FetchedPage> {
            Err(Error::other("HTTP 503 Service Unavailable".to_string()))
        }
    }

    fn model(reply: &str) -> Arc<dyn Model> {
        Arc::new(MockMessageModel::new(vec![MockResponse::message(
            Message::assistant(reply),
        )]))
    }

    fn tool(transport: Arc<dyn WebFetchTransport>, reply: &str) -> FunctionTool {
        make_web_fetch(MakeWebFetchOptions {
            model: model(reply),
            transport: Some(transport),
        })
    }

    #[tokio::test]
    async fn empty_prompt_returns_the_raw_content() {
        let t = tool(Arc::new(StubTransport::text("plain body")), "unused");
        let result = invoke(&t, serde_json::json!({ "url": "https://example.com" })).await;
        assert_eq!(text_of(&result), "plain body");
    }

    #[tokio::test]
    async fn blank_prompt_returns_the_raw_content() {
        let t = tool(Arc::new(StubTransport::text("plain body")), "unused");
        let result = invoke(
            &t,
            serde_json::json!({ "url": "https://example.com", "prompt": "   " }),
        )
        .await;
        assert_eq!(text_of(&result), "plain body");
    }

    #[tokio::test]
    async fn html_is_reduced_to_text() {
        let t = tool(
            Arc::new(StubTransport::html(
                "<html><head><style>b {color:red}</style></head><body><script>var x=1</script><p>Hello   world</p></body></html>",
            )),
            "unused",
        );
        let result = invoke(&t, serde_json::json!({ "url": "https://example.com" })).await;
        assert_eq!(text_of(&result), "Hello world");
    }

    #[tokio::test]
    async fn a_prompt_is_answered_by_the_summarizer() {
        let t = tool(Arc::new(StubTransport::text("the body")), "the answer");
        let result = invoke(
            &t,
            serde_json::json!({ "url": "https://example.com", "prompt": "what?" }),
        )
        .await;
        assert_eq!(text_of(&result), "the answer");
    }

    #[tokio::test]
    async fn content_is_cached_across_calls() {
        let stub = Arc::new(StubTransport::text("cached body"));
        let calls = Arc::clone(&stub.calls);
        let t = tool(stub, "unused");
        for _ in 0..2 {
            invoke(&t, serde_json::json!({ "url": "https://example.com" })).await;
        }
        assert_eq!(calls.lock().len(), 1);
    }

    #[tokio::test]
    async fn a_fetch_failure_is_reported_to_the_model() {
        let t = tool(Arc::new(FailingTransport), "unused");
        let result = invoke(&t, serde_json::json!({ "url": "https://example.com" })).await;
        assert_eq!(
            text_of(&result),
            "Failed to fetch https://example.com: HTTP 503 Service Unavailable"
        );
    }

    #[tokio::test]
    async fn rejects_a_non_http_scheme() {
        let t = tool(Arc::new(StubTransport::text("x")), "unused");
        let result = invoke(&t, serde_json::json!({ "url": "ftp://example.com" })).await;
        assert_eq!(
            text_of(&result),
            "Failed to fetch ftp://example.com: web_fetch only supports http(s) URLs, got \"ftp://example.com\"."
        );
    }

    #[tokio::test]
    async fn rejects_a_url_with_no_host() {
        let t = tool(Arc::new(StubTransport::text("x")), "unused");
        let result = invoke(&t, serde_json::json!({ "url": "https:///path" })).await;
        assert!(
            text_of(&result).contains("web_fetch URL has no host: \"https:///path\"."),
            "{}",
            text_of(&result)
        );
    }

    #[tokio::test]
    async fn rejects_disallowed_url_characters() {
        let t = tool(Arc::new(StubTransport::text("x")), "unused");
        let result = invoke(&t, serde_json::json!({ "url": "https://example.com/a b" })).await;
        assert!(
            text_of(&result).contains("web_fetch URLs may only contain the characters RFC 3986 allows"),
            "{}",
            text_of(&result)
        );
    }

    #[tokio::test]
    async fn spec_matches_typescript() {
        let t = tool(Arc::new(StubTransport::text("x")), "unused");
        let agent = test_agent("x").await;
        drop(agent);
        assert_eq!(strands_agents::tools::Tool::name(&t), "web_fetch");
        assert_eq!(
            strands_agents::tools::Tool::description(&t),
            WEB_FETCH_DESCRIPTION
        );
    }

    #[test]
    fn html_to_text_collapses_blank_lines() {
        // Verbatim TS output: tag removal leaves the spaces the regexes do not touch.
        assert_eq!(html_to_text("<p>a</p>\n \n \n<p>b</p>"), "a \n\n b");
        assert_eq!(html_to_text("<p>a</p>\n\n<p>b</p>"), "a \n\n b");
    }

    #[test]
    fn summarizer_prompt_is_verbatim() {
        assert!(SUMMARIZER_PROMPT.starts_with("You answer a request about a single fetched web page."));
        assert!(
            SUMMARIZER_PROMPT.ends_with("details (names, numbers, quotes, links) relevant to the request.")
        );
    }
}
