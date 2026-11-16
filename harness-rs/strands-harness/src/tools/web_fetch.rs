//! `web_fetch`: fetch a URL and answer a prompt about its content (TS `tools/web-fetch.ts`).
//!
//! Fetches the URL, reduces it to text, and asks a small fast model to answer the prompt over
//! that content, so large pages never reach the main agent's context. By default the request
//! runs as `curl` inside the agent's sandbox, so the sandbox's network controls cover it;
//! [`WebFetchTransport::Direct`] fetches from the harness process instead.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::Duration;

use base64::Engine;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use futures::StreamExt;
use regex::Regex;
use serde::Deserialize;
use strands::Agent;
use strands::JsonValue;
use strands::models::model::Model;
use strands::sandbox::{ExecuteOptions, Sandbox};
use strands::tools::function_tool::InputValidator;
use strands::tools::{FunctionTool, ToolContext, ToolError};
use tokio::time::Instant;

use crate::sandbox::{SandboxShell, ShellCache};
use crate::types::WebFetchTransport;

const USER_AGENT: &str = "strands-harness/1.0";
const TIMEOUT_SECONDS: u64 = 30;
const MAX_BYTES: usize = 5 * 1024 * 1024;
const MAX_CHARS: usize = 50_000;
const CACHE_TTL: Duration = Duration::from_secs(15 * 60);
// The characters RFC 3986 allows anywhere in a URL; anything else (whitespace, quotes, control
// characters, non-ASCII) is rejected before the URL reaches a shell or a socket.
const URL_PUNCTUATION: &str = "-._~:/?#[]@!$&'()*+,;=%";

const SUMMARIZER_PROMPT: &str = "You answer a request about a single fetched web page. Use only the provided content; if it \
does not contain the answer, say so plainly. Be concise and factual, and preserve concrete \
details (names, numbers, quotes, links) relevant to the request.";

const DESCRIPTION: &str = "Fetch a URL and answer a prompt about its content. Performs an HTTP GET, reduces the \
response to text, and asks a small fast model to answer the prompt using only that content, \
returning the answer rather than the raw page so large pages do not flood the conversation. \
Fetched content is cached for 15 minutes, so repeated fetches of the same URL are fast while \
each prompt is answered fresh.";

const PROMPT_DESCRIPTION: &str = "What to extract from or answer about the fetched content. Leave empty to get the \
whole page content back verbatim instead of a model-generated answer.";

/// A compiled pattern; `None` only if the literal were not a valid pattern.
type Pattern = LazyLock<Option<Regex>>;

static SCRIPT_OR_STYLE: Pattern =
    LazyLock::new(|| Regex::new(r"(?is)<script\b[^>]*>.*?</script>|<style\b[^>]*>.*?</style>").ok());
static TAG: Pattern = LazyLock::new(|| Regex::new(r"<[^>]+>").ok());
static SPACES: Pattern = LazyLock::new(|| Regex::new(r"[ \t]+").ok());
static BLANK_LINES: Pattern = LazyLock::new(|| Regex::new(r"\n\s*\n\s*\n+").ok());
static CHARSET: Pattern = LazyLock::new(|| Regex::new(r#"(?i)charset=["']?([\w.-]+)"#).ok());
static HAS_AUTHORITY: Pattern = LazyLock::new(|| Regex::new(r"(?i)^https?://[^/?#]").ok());

fn replace_all(pattern: &Pattern, text: &str, with: &str) -> String {
    match pattern.as_ref() {
        Some(pattern) => pattern.replace_all(text, with).into_owned(),
        None => text.to_owned(),
    }
}

/// JS `String.prototype.trim`, which also strips the byte-order mark.
fn js_trim(value: &str) -> &str {
    value.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
}

fn html_to_text(html: &str) -> String {
    let text = replace_all(&SCRIPT_OR_STYLE, html, " ");
    let text = replace_all(&TAG, &text, " ");
    let text = replace_all(&SPACES, &text, " ");
    let text = replace_all(&BLANK_LINES, &text, "\n\n");
    js_trim(&text).to_owned()
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| format!("\"{value}\""))
}

/// `url` trimmed, or the reason it is not a well-formed http(s) URL.
fn validate_url(url: &str) -> Result<&str, String> {
    let url = js_trim(url);
    let allowed = |c: char| c.is_ascii_alphanumeric() || URL_PUNCTUATION.contains(c);
    if url.is_empty() || !url.chars().all(allowed) {
        return Err(
            "web_fetch URLs may only contain the characters RFC 3986 allows; percent-encode spaces and non-ASCII \
             characters (and punycode the host) and retry."
                .to_owned(),
        );
    }
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return Err(format!("web_fetch could not parse the URL {}.", json_string(url)));
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!(
            "web_fetch only supports http(s) URLs, got {}.",
            json_string(url)
        ));
    }
    // Check the raw authority too: the WHATWG parser silently repairs forms like `https:///host`.
    if parsed.host_str().is_none_or(str::is_empty)
        || !HAS_AUTHORITY
            .as_ref()
            .is_some_and(|pattern| pattern.is_match(url))
    {
        return Err(format!("web_fetch URL has no host: {}.", json_string(url)));
    }
    Ok(url)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// The `sh` pipeline. `-g` keeps `{}` and `[]` in the URL literal; `--proto` and `--proto-redir`
/// keep the request and any redirect on http(s); `--fail` turns HTTP errors into a non-zero exit.
/// The body is spooled to a file the shell picks with `mktemp`, truncated there, and returned as
/// base64 after two lines: the final hop's content type and URL.
fn curl_command(url: &str) -> String {
    format!(
        "f=$(mktemp -t strands-web-fetch.XXXXXX) && \
         curl -sSL -g --fail --proto '=http,https' --proto-redir '=http,https' --max-time {TIMEOUT_SECONDS} \
         -A {} -o \"$f\" -w '%{{content_type}}\\n%{{url_effective}}\\n' -- {} \
         && head -c {MAX_BYTES} \"$f\" | base64; s=$?; rm -f \"$f\"; exit $s",
        shell_quote(USER_AGENT),
        shell_quote(url)
    )
}

/// The same contract for a sandbox whose shell is PowerShell. `curl.exe` because plain `curl` is
/// an `Invoke-WebRequest` alias there; errors go through the console handle because PowerShell's
/// own error stream is CLIXML-wrapped when stderr is a pipe.
fn powershell_curl_command(url: &str) -> String {
    let quoted = format!("'{}'", url.replace('\'', "''"));
    [
        "$f = Join-Path ([IO.Path]::GetTempPath()) \"strands-web-fetch-$([guid]::NewGuid())\"".to_owned(),
        "$s = 127".to_owned(),
        "try {".to_owned(),
        format!(
            "  curl.exe -sSL -g --fail --proto '=http,https' --proto-redir '=http,https' --max-time {TIMEOUT_SECONDS} \
             -A '{USER_AGENT}' -o $f --stderr \"$f.err\" -w '%{{content_type}}\\n%{{url_effective}}\\n' {quoted}"
        ),
        "  $s = $LASTEXITCODE".to_owned(),
        "  if ($s -eq 0) {".to_owned(),
        "    $in = [IO.File]::OpenRead($f)".to_owned(),
        format!(
            "    try {{ $b = (New-Object IO.BinaryReader $in).ReadBytes([Math]::Min($in.Length, {MAX_BYTES})) }} \
             finally {{ $in.Dispose() }}"
        ),
        "    [Convert]::ToBase64String($b)".to_owned(),
        "  } elseif (Test-Path -LiteralPath \"$f.err\") { [Console]::Error.Write([IO.File]::ReadAllText(\"$f.err\")) }"
            .to_owned(),
        "} catch { if ($s -eq 0) { $s = 1 }; [Console]::Error.Write($_.Exception.Message) }".to_owned(),
        "Remove-Item -Force -LiteralPath $f, \"$f.err\" -ErrorAction SilentlyContinue".to_owned(),
        "exit $s".to_owned(),
    ]
    .join("\n")
}

/// Decodes with the content type's charset, or UTF-8 when it names none or an unknown one.
fn decode(data: &[u8], content_type: &str) -> String {
    let encoding = CHARSET
        .as_ref()
        .and_then(|pattern| pattern.captures(content_type))
        .and_then(|captures| captures.get(1))
        .and_then(|label| encoding_rs::Encoding::for_label(label.as_str().as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    encoding.decode_with_bom_removal(data).0.into_owned()
}

/// The first `MAX_CHARS` UTF-16 code units of `text` (TS `text.slice(0, MAX_CHARS)`); a
/// character the cut would split is dropped.
fn cap_chars(mut text: String) -> String {
    let mut units = 0;
    let cut = text.char_indices().find_map(|(index, c)| {
        units += c.len_utf16();
        (units > MAX_CHARS).then_some(index)
    });
    if let Some(index) = cut {
        text.truncate(index);
    }
    text
}

#[derive(Debug, Clone)]
struct Fetched {
    resolved_url: String,
    text: String,
}

fn to_text(data: &[u8], content_type: &str, resolved_url: &str, url: &str) -> Fetched {
    let raw = decode(data, content_type);
    let text = if content_type.to_lowercase().contains("html") {
        html_to_text(&raw)
    } else {
        raw
    };
    let resolved_url = js_trim(resolved_url);
    Fetched {
        resolved_url: if resolved_url.is_empty() {
            url
        } else {
            resolved_url
        }
        .to_owned(),
        text: cap_chars(text),
    }
}

/// Decodes base64 as Node's `Buffer.from(text, 'base64')` does: padding is optional and
/// characters outside the alphabet are skipped.
fn decode_base64(encoded: &str) -> Vec<u8> {
    const LENIENT: GeneralPurpose = GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        GeneralPurposeConfig::new()
            .with_decode_padding_mode(DecodePaddingMode::Indifferent)
            .with_decode_allow_trailing_bits(true),
    );
    let mut symbols: String = encoded
        .chars()
        .take_while(|c| *c != '=')
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/'))
        .collect();
    // A lone trailing symbol carries less than a byte.
    if symbols.len() % 4 == 1 {
        symbols.pop();
    }
    LENIENT.decode(symbols).unwrap_or_default()
}

/// Fetches a validated `url` with `curl` inside `sandbox`.
async fn fetch_curl(shells: &ShellCache, sandbox: &Arc<dyn Sandbox>, url: &str) -> Result<Fetched, String> {
    let shell = shells
        .shell_of(sandbox)
        .await
        .map_err(|error| error.to_string())?;
    let command = match shell {
        SandboxShell::PowerShell => powershell_curl_command(url),
        SandboxShell::Sh => curl_command(url),
    };
    let options = ExecuteOptions {
        timeout: Some(Duration::from_secs(TIMEOUT_SECONDS + 5)),
        ..ExecuteOptions::default()
    };
    let result = sandbox
        .execute(&command, options)
        .await
        .map_err(|error| error.to_string())?;
    if result.exit_code != 0 {
        let stderr = js_trim(&result.stderr);
        return Err(if stderr.is_empty() {
            format!("curl exited with code {}", result.exit_code)
        } else {
            stderr.to_owned()
        });
    }
    // Two header lines (the content-type line is empty when the header is absent), then the
    // base64 body, which base64(1) may wrap.
    let mut lines = result.stdout.split('\n');
    let content_type = js_trim(lines.next().unwrap_or_default());
    let resolved_url = lines.next().unwrap_or_default();
    let data = decode_base64(&lines.collect::<String>());
    Ok(to_text(&data, content_type, resolved_url, url))
}

/// Fetches a validated `url` from the harness process, bypassing the sandbox.
async fn fetch_direct(client: &reqwest::Client, url: &str) -> Result<Fetched, String> {
    let timed_out = |error: reqwest::Error| {
        if error.is_timeout() {
            format!("web_fetch gave up after {TIMEOUT_SECONDS}s.")
        } else {
            error.to_string()
        }
    };
    let response = client
        .get(url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .timeout(Duration::from_secs(TIMEOUT_SECONDS))
        .send()
        .await
        .map_err(timed_out)?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "HTTP {} {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or_default()
        ));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let resolved_url = response.url().to_string();
    // Read at most `MAX_BYTES`, then drop the stream rather than buffering the rest.
    let mut data = Vec::new();
    let mut chunks = response.bytes_stream();
    while data.len() < MAX_BYTES {
        let Some(chunk) = chunks.next().await else { break };
        data.extend_from_slice(&chunk.map_err(timed_out)?);
    }
    data.truncate(MAX_BYTES);
    Ok(to_text(&data, &content_type, &resolved_url, url))
}

/// Options for [`make_web_fetch`] (TS `MakeWebFetchOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, bon::Builder)]
pub struct MakeWebFetchOptions {
    /// The summarizer that answers the prompt over the fetched page.
    #[builder(into)]
    pub model: Arc<dyn Model>,
    /// [`WebFetchTransport::Curl`] runs inside the agent's sandbox (the default);
    /// [`WebFetchTransport::Direct`] fetches from the harness process.
    pub transport: Option<WebFetchTransport>,
}

#[derive(Deserialize)]
struct WebFetchInput {
    url: String,
    prompt: Option<String>,
}

struct CacheEntry {
    expires: Instant,
    fetched: Fetched,
}

struct WebFetch {
    model: Arc<dyn Model>,
    transport: WebFetchTransport,
    client: reqwest::Client,
    shells: ShellCache,
    cache: Mutex<HashMap<String, CacheEntry>>,
}

impl WebFetch {
    fn cached(&self, url: &str) -> Option<Fetched> {
        let cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        cache
            .get(url)
            .filter(|entry| entry.expires > Instant::now())
            .map(|entry| entry.fetched.clone())
    }

    async fn fetch(&self, url: &str, context: &ToolContext) -> Result<Fetched, String> {
        let url = validate_url(url)?;
        match self.transport {
            WebFetchTransport::Direct => fetch_direct(&self.client, url).await,
            _ => {
                let sandbox = context.agent().sandbox().map_err(|error| error.to_string())?;
                fetch_curl(&self.shells, &sandbox, url).await
            }
        }
    }

    async fn run(&self, input: WebFetchInput, context: ToolContext) -> Result<JsonValue, ToolError> {
        let fetched = match self.cached(&input.url) {
            Some(fetched) => fetched,
            None => {
                let fetched = match self.fetch(&input.url, &context).await {
                    Ok(fetched) => fetched,
                    Err(message) => return Ok(format!("Failed to fetch {}: {message}", input.url).into()),
                };
                let entry = CacheEntry {
                    expires: Instant::now() + CACHE_TTL,
                    fetched: fetched.clone(),
                };
                self.cache
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(input.url.clone(), entry);
                fetched
            }
        };
        let Fetched { resolved_url, text } = fetched;
        let Some(prompt) = input.prompt.filter(|prompt| !js_trim(prompt).is_empty()) else {
            return Ok(text.into());
        };

        let mut summarizer = Agent::builder()
            .model(Arc::clone(&self.model))
            .system_prompt(SUMMARIZER_PROMPT)
            .printer(false)
            .build()?;
        let result = summarizer
            .invoke(format!(
                "Fetched URL: {resolved_url}\n\nRequest: {prompt}\n\n--- Content ---\n{text}"
            ))
            .await?;
        Ok(result.to_string().into())
    }
}

/// Builds a `web_fetch` tool whose summarizer answers over the fetched page using
/// `options.model` (TS `makeWebFetch`).
pub fn make_web_fetch(options: MakeWebFetchOptions) -> FunctionTool {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "url": { "type": "string", "description": "The http(s) URL to fetch." },
            "prompt": { "type": "string", "description": PROMPT_DESCRIPTION },
        },
        "required": ["url"],
        "additionalProperties": false,
    });
    let validator = InputValidator::new(&schema);
    let state = Arc::new(WebFetch {
        model: options.model,
        transport: options.transport.unwrap_or_default(),
        client: reqwest::Client::new(),
        shells: ShellCache::default(),
        cache: Mutex::default(),
    });
    FunctionTool::new(
        "web_fetch",
        DESCRIPTION,
        move |input: JsonValue, context: ToolContext| {
            let parsed = validator.parse::<WebFetchInput>(input);
            let state = Arc::clone(&state);
            async move { state.run(parsed?, context).await }
        },
    )
    .with_input_schema(schema)
}

#[cfg(test)]
mod tests;
