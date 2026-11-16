//! The web fetch tool (TS `vended-tools/web-fetch/`): fetches a URL and returns clean markdown,
//! or answers a prompt about the page with an analyst agent. Distinct from
//! [`http_request`](super::http_request()) (raw API calls).

mod extract;

use std::sync::{Arc, LazyLock};

use futures::StreamExt;
use regex::Regex;
use serde::Deserialize;

use crate::JsonValue;
use crate::agent::{Agent, InvocationContext};
use crate::error::{Error, Result};
use crate::models::model::Model;
use crate::tools::function_tool::InputValidator;
use crate::tools::{FunctionTool, ToolContext, ToolError};
use crate::types::agent::InvokeOptions;
use crate::types::messages::ContentBlock;

use super::{no_precheck, schema_tool};

use extract::html_to_markdown;

/// The default response body cap in bytes (TS `DEFAULT_MAX_BYTES`, 5 MiB).
pub(crate) const DEFAULT_MAX_BYTES: usize = 5 * 1024 * 1024;
/// The default cap on extracted characters delivered to the model (TS
/// `DEFAULT_MAX_CONTENT_CHARS`).
pub(crate) const DEFAULT_MAX_CONTENT_CHARS: usize = 50_000;

/// The markdown mode description (TS `WEB_FETCH_DESCRIPTION_MARKDOWN`).
pub(crate) const WEB_FETCH_DESCRIPTION_MARKDOWN: &str = "Fetches an HTTP(S) URL and returns its content as clean markdown. HTML pages are converted to markdown with scripts, styles, and noise stripped; other content types are returned as-is.";
/// The agentic mode description (TS `WEB_FETCH_DESCRIPTION_AGENTIC`).
pub(crate) const WEB_FETCH_DESCRIPTION_AGENTIC: &str = "Fetches an HTTP(S) URL and answers a prompt about its content. The analyst processes the page directly so the full content never enters the main agent's context. The prompt parameter is required.";

const USER_AGENT: &str = "strands-agents-web-fetch/1.0";
const ACCEPT: &str = "text/html,application/xhtml+xml;q=0.9,*/*;q=0.8";
const CANCELLED: &str = "Web fetch tool request cancelled";
const URL_DESCRIPTION: &str = "URL to fetch. Must be http:// or https://.";
const ANALYST_PROMPT: &str = "You answer a request about a single fetched web page. Use only the provided content; if it does not contain the answer, say so plainly. Be concise and factual, and preserve concrete details (names, numbers, quotes, links) relevant to the request.";

static CHARSET: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r#"(?i)charset=(?:"([^"]+)"|'([^']+)'|([^;\s]+))"#).ok());

/// What the web fetch tool returns (TS `mode`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum WebFetchMode {
    /// The page as markdown.
    Markdown,
    /// An analyst agent's answer to the call's `prompt` about the page.
    #[default]
    Agentic,
}

/// Options for [`make_web_fetch`] (TS `MakeWebFetchOptions`).
///
/// ```
/// use strands::Tool;
/// use strands::vended_tools::{MakeWebFetchOptions, WebFetchMode, make_web_fetch};
///
/// let tool = make_web_fetch(MakeWebFetchOptions::builder().mode(WebFetchMode::Markdown).build())?;
/// assert_eq!(tool.name(), "web_fetch");
/// # Ok::<(), strands::Error>(())
/// ```
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(on(String, into), state_mod(vis = "pub"), derive(Debug))]
pub struct MakeWebFetchOptions {
    /// The tool name (default `web_fetch`).
    pub name: Option<String>,
    /// The description (default the mode's description).
    pub description: Option<String>,
    /// The response body cap in bytes (default 5 MiB).
    pub max_bytes: Option<usize>,
    /// The cap on extracted characters (default 50,000).
    pub max_content_chars: Option<usize>,
    /// The analyst's model in agentic mode. Resolution order: this model > `agent.aux_model` >
    /// `agent.model` of the calling agent.
    pub model: Option<Arc<dyn Model>>,
    /// The extraction mode (default [`WebFetchMode::Agentic`]).
    #[builder(default)]
    pub mode: WebFetchMode,
}

#[derive(Deserialize)]
struct WebFetchInput {
    url: String,
    prompt: Option<String>,
}

#[derive(Clone)]
struct Limits {
    max_bytes: usize,
    max_content_chars: usize,
}

/// A web fetch tool (TS `makeWebFetch`). Each call makes one `GET`; HTML and XML are converted
/// to markdown, other content types are returned as-is, and the content is cut at
/// `max_content_chars`. In agentic mode a fresh analyst agent answers the call's `prompt`
/// about the content, and its usage adds to the calling request.
///
/// # Errors
///
/// `max_bytes` or `max_content_chars` is zero.
pub fn make_web_fetch(options: MakeWebFetchOptions) -> Result<FunctionTool> {
    let max_bytes = options.max_bytes.unwrap_or(DEFAULT_MAX_BYTES);
    let max_content_chars = options.max_content_chars.unwrap_or(DEFAULT_MAX_CONTENT_CHARS);
    if max_bytes == 0 {
        return Err(Error::other("maxBytes must be a positive number, got 0"));
    }
    if max_content_chars == 0 {
        return Err(Error::other("maxContentChars must be a positive number, got 0"));
    }
    let limits = Limits {
        max_bytes,
        max_content_chars,
    };
    let name = options.name.unwrap_or_else(|| "web_fetch".to_owned());
    let client = reqwest::Client::new();
    Ok(match options.mode {
        WebFetchMode::Markdown => {
            let description = options
                .description
                .unwrap_or_else(|| WEB_FETCH_DESCRIPTION_MARKDOWN.to_owned());
            markdown_tool(name, description, client, limits)
        }
        _ => {
            let description = options
                .description
                .unwrap_or_else(|| WEB_FETCH_DESCRIPTION_AGENTIC.to_owned());
            agentic_tool(name, description, client, limits, options.model)
        }
    })
}

/// The default web fetch tool (TS `webFetch`), in agentic mode with the default limits.
pub fn web_fetch() -> FunctionTool {
    agentic_tool(
        "web_fetch".to_owned(),
        WEB_FETCH_DESCRIPTION_AGENTIC.to_owned(),
        reqwest::Client::new(),
        Limits {
            max_bytes: DEFAULT_MAX_BYTES,
            max_content_chars: DEFAULT_MAX_CONTENT_CHARS,
        },
        None,
    )
}

fn url_property() -> JsonValue {
    serde_json::json!({ "type": "string", "description": URL_DESCRIPTION })
}

fn markdown_tool(name: String, description: String, client: reqwest::Client, limits: Limits) -> FunctionTool {
    let schema = serde_json::json!({
        "type": "object",
        "properties": { "url": url_property() },
        "required": ["url"],
        "additionalProperties": false,
    });
    schema_tool(
        name,
        description,
        schema.clone(),
        InputValidator::new(&schema),
        no_precheck,
        move |input: WebFetchInput, context: ToolContext| {
            let (client, limits) = (client.clone(), limits.clone());
            async move { fetch_content(&client, &input.url, &limits, &context).await }
        },
    )
}

fn agentic_tool(
    name: String,
    description: String,
    client: reqwest::Client,
    limits: Limits,
    model: Option<Arc<dyn Model>>,
) -> FunctionTool {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "url": url_property(),
            "prompt": { "type": "string", "description": "Question or instruction about the page content." },
        },
        "required": ["url", "prompt"],
        "additionalProperties": false,
    });
    schema_tool(
        name,
        description,
        schema.clone(),
        InputValidator::new(&schema),
        no_precheck,
        move |input: WebFetchInput, context: ToolContext| {
            let (client, limits, model) = (client.clone(), limits.clone(), model.clone());
            async move {
                let prompt = input.prompt.unwrap_or_default();
                if prompt.trim().is_empty() {
                    return Err(ToolError::msg(
                        "web_fetch: agentic mode requires a non-empty prompt.",
                    ));
                }
                let model = model.unwrap_or_else(|| Arc::clone(context.agent().aux_model()));
                let content = fetch_content(&client, &input.url, &limits, &context).await?;
                analyze(model, &input.url, &prompt, &content, &context).await
            }
        },
    )
}

/// Runs a fresh analyst agent per call, so no history from one fetch reaches the next.
async fn analyze(
    model: Arc<dyn Model>,
    url: &str,
    prompt: &str,
    content: &str,
    context: &ToolContext,
) -> Result<String, ToolError> {
    let failed = |error: Error| ToolError::msg(format!("url=<{url}> | web fetch analyst failed: {error}"));
    let mut analyst = Agent::builder()
        .model(model)
        .system_prompt(ANALYST_PROMPT)
        .printer(false)
        .build()
        .map_err(failed)?;
    let options = InvokeOptions {
        cancel_signal: Some(context.cancel_signal().clone()),
        invocation_context: InvocationContext::auxiliary(context.invocation_context()),
        ..InvokeOptions::default()
    };
    let request = format!("URL: {url}\n\nRequest: {prompt}\n\n--- Content ---\n{content}");
    let result = analyst.invoke_with(request, options).await.map_err(failed)?;
    Ok(result
        .last_message()
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text()),
            _ => None,
        })
        .collect())
}

/// Fetches `url` and returns its content, as markdown for markup types, cut at the limit.
async fn fetch_content(
    client: &reqwest::Client,
    url: &str,
    limits: &Limits,
    context: &ToolContext,
) -> Result<String, ToolError> {
    let signal = context.cancel_signal().clone();
    let (content_type, raw) = tokio::select! {
        fetched = fetch_once(client, url, limits.max_bytes) => fetched?,
        () = signal.cancelled() => return Err(ToolError::msg(CANCELLED)),
    };
    let content_type = content_type.to_lowercase();
    let content = if content_type.contains("html") || content_type.contains("xml") {
        html_to_markdown(&raw)
    } else {
        raw
    };
    Ok(truncate(content, limits.max_content_chars))
}

fn truncate(content: String, max_chars: usize) -> String {
    if content.chars().nth(max_chars).is_none() {
        return content;
    }
    let mut truncated: String = content.chars().take(max_chars).collect();
    truncated.push_str("\n\n[content truncated]");
    truncated
}

/// One `GET`, returning the content type and the body decoded with its charset.
async fn fetch_once(
    client: &reqwest::Client,
    url: &str,
    max_bytes: usize,
) -> Result<(String, String), ToolError> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|_| ToolError::msg(format!("url=<{url}> | fetch failed: invalid URL")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(ToolError::msg(format!(
            "url=<{url}> | fetch failed: only http and https URLs are supported"
        )));
    }
    let response = client
        .get(parsed)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::ACCEPT, ACCEPT)
        .send()
        .await
        .map_err(|error| ToolError::msg(format!("url=<{url}> | fetch failed: {error}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(ToolError::msg(format!(
            "HTTP {} {}: GET {url}",
            status.as_u16(),
            status.canonical_reason().unwrap_or_default()
        )));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let mut body = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk?;
        if body.len() + chunk.len() > max_bytes {
            return Err(ToolError::msg(format!(
                "Response body exceeded max_bytes={max_bytes}. Refusing to buffer more."
            )));
        }
        body.extend_from_slice(&chunk);
    }
    let (text, _) = charset(&content_type).decode_with_bom_removal(&body);
    Ok((content_type, text.into_owned()))
}

/// The encoding named by the content type's `charset`, else UTF-8 (TS `_parseCharset`).
fn charset(content_type: &str) -> &'static encoding_rs::Encoding {
    CHARSET
        .as_ref()
        .and_then(|pattern| pattern.captures(content_type))
        .and_then(|captures| captures.iter().skip(1).flatten().next())
        .and_then(|label| encoding_rs::Encoding::for_label(label.as_str().as_bytes()))
        .unwrap_or(encoding_rs::UTF_8)
}

#[cfg(test)]
mod tests;
