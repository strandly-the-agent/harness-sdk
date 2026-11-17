//! Ported from harness-ts/test/tools/web-fetch.test.ts

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use base64::Engine;
use futures::StreamExt;
use serde_json::json;
use strands::JsonValue;
use strands::models::model::BoxStream;
use strands::sandbox::{
    ExecuteOptions, ExecutionEvent, ExecutionResult, FileInfo, NotASandboxLocalEnvironment, Sandbox,
};
use strands::testing::{MockMessageModel, tool_context_with_sandbox};
use strands::tools::FunctionTool;
use strands::types::messages::{ContentBlock, SystemPrompt, TextBlock};
use strands::types::tools::ToolUse;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::{MakeWebFetchOptions, make_web_fetch};
use crate::sandbox::SHELL_PROBE;
use crate::types::WebFetchTransport;

#[derive(Default, Clone)]
struct Response {
    body: Vec<u8>,
    content_type: Option<&'static str>,
    url: Option<&'static str>,
    exit_code: i32,
    stderr: &'static str,
}

fn page(body: &str, content_type: &'static str) -> Response {
    Response {
        body: body.as_bytes().to_vec(),
        content_type: Some(content_type),
        ..Response::default()
    }
}

/// A sandbox that answers the shell probe and the curl pipeline from a canned response. With
/// `powershell`, the probe reports the edition, lines end in CRLF, and the base64 body is one
/// unwrapped line.
#[derive(Default)]
struct FakeSandbox {
    commands: Mutex<Vec<String>>,
    response: Response,
    powershell: bool,
    fails: bool,
}

impl FakeSandbox {
    fn new(response: Response) -> Arc<Self> {
        Arc::new(Self {
            response,
            ..Self::default()
        })
    }

    fn powershell(response: Response) -> Arc<Self> {
        Arc::new(Self {
            response,
            powershell: true,
            ..Self::default()
        })
    }

    fn commands(&self) -> Vec<String> {
        self.commands.lock().unwrap().clone()
    }

    fn curl(&self) -> String {
        let is_curl = |command: &String| {
            if self.powershell {
                command.contains("curl.exe ")
            } else {
                command.starts_with("f=$(mktemp") && command.contains("curl ")
            }
        };
        self.commands().into_iter().find(is_curl).unwrap()
    }

    /// What the shell pipeline prints: the two trailer lines, then the base64 body (wrapped at
    /// 76 columns by base64(1)).
    fn render(&self) -> String {
        let encoded = base64::engine::general_purpose::STANDARD.encode(&self.response.body);
        let trailer = [
            self.response.content_type.unwrap_or("text/html"),
            self.response.url.unwrap_or("https://example.com/final"),
        ];
        if self.powershell {
            return format!("{}\r\n{encoded}\r\n", trailer.join("\r\n"));
        }
        let wrapped: Vec<&str> = encoded
            .as_bytes()
            .chunks(76)
            .map(|line| std::str::from_utf8(line).unwrap())
            .collect();
        format!("{}\n{}\n", trailer.join("\n"), wrapped.join("\n"))
    }
}

#[async_trait]
impl Sandbox for FakeSandbox {
    fn execute_streaming<'a>(
        &'a self,
        _command: &'a str,
        _options: ExecuteOptions,
    ) -> BoxStream<'a, strands::Result<ExecutionEvent>> {
        futures::stream::empty().boxed()
    }

    fn execute_code_streaming<'a>(
        &'a self,
        _code: &'a str,
        _language: &'a str,
        _options: ExecuteOptions,
    ) -> BoxStream<'a, strands::Result<ExecutionEvent>> {
        futures::stream::empty().boxed()
    }

    async fn execute(&self, command: &str, _options: ExecuteOptions) -> strands::Result<ExecutionResult> {
        if self.fails {
            return Err(strands::Error::other("sandbox gone"));
        }
        self.commands.lock().unwrap().push(command.to_owned());
        if command == SHELL_PROBE {
            let edition = if self.powershell { "Desktop\r\n" } else { "\n" };
            return Ok(ExecutionResult::new(0, edition, ""));
        }
        let exit_code = self.response.exit_code;
        let stdout = if exit_code == 0 {
            self.render()
        } else {
            String::new()
        };
        Ok(ExecutionResult::new(exit_code, stdout, self.response.stderr))
    }

    async fn read_file(&self, _path: &str) -> strands::Result<Vec<u8>> {
        Ok(Vec::new())
    }

    async fn write_file(&self, _path: &str, _content: &[u8]) -> strands::Result<()> {
        Ok(())
    }

    async fn remove_file(&self, _path: &str) -> strands::Result<()> {
        Ok(())
    }

    async fn list_files(&self, _path: &str) -> strands::Result<Vec<FileInfo>> {
        Ok(Vec::new())
    }
}

/// A summarizer model that answers `summary answer` to every request.
fn model() -> Arc<MockMessageModel> {
    let model = MockMessageModel::new();
    for _ in 0..4 {
        model.add_turn(vec![TextBlock::new("summary answer").into()]);
    }
    Arc::new(model)
}

fn web_fetch(model: &Arc<MockMessageModel>) -> FunctionTool {
    make_web_fetch(MakeWebFetchOptions {
        model: model.clone(),
        transport: None,
    })
}

fn direct(model: &Arc<MockMessageModel>) -> FunctionTool {
    make_web_fetch(
        MakeWebFetchOptions::builder()
            .model(model.clone())
            .transport(WebFetchTransport::Direct)
            .build(),
    )
}

async fn invoke(tool: &FunctionTool, input: JsonValue, sandbox: Arc<dyn Sandbox>) -> String {
    let context = tool_context_with_sandbox(ToolUse::new("web_fetch", "tool-1", input.clone()), sandbox);
    let output = tool.invoke(input, context).await.unwrap();
    output.as_str().unwrap().to_owned()
}

/// The prompts the summarizer was invoked with.
fn invoke_calls(model: &MockMessageModel) -> Vec<String> {
    model
        .requests()
        .iter()
        .map(|request| {
            let message = request.messages.last().unwrap();
            message
                .content()
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text(text) => Some(text.text()),
                    _ => None,
                })
                .collect()
        })
        .collect()
}

#[tokio::test]
async fn reduces_html_to_text_and_answers_via_the_summarizer_model() {
    let sandbox = FakeSandbox::new(Response {
        body: b"<html><body><script>x=1</script><p>Hello  <b>world</b></p></body></html>".to_vec(),
        ..Response::default()
    });
    let model = model();
    let tool = web_fetch(&model);
    let input = json!({ "url": "https://example.com", "prompt": "What does it say?" });
    let answer = invoke(&tool, input, sandbox).await;

    assert_eq!(answer, "summary answer");
    let requests = model.requests();
    assert_eq!(
        requests[0].system_prompt,
        Some(SystemPrompt::Text(super::SUMMARIZER_PROMPT.to_owned()))
    );
    assert_eq!(
        invoke_calls(&model),
        [
            "Fetched URL: https://example.com/final\n\nRequest: What does it say?\n\n--- Content ---\nHello world"
        ]
    );
}

#[tokio::test]
async fn runs_curl_in_the_agent_sandbox_with_the_url_quoted_and_globbing_off_then_removes_the_body_file() {
    let sandbox = FakeSandbox::new(page("body", "text/plain"));
    let tool = web_fetch(&model());
    invoke(
        &tool,
        json!({ "url": "https://example.com/a'b$(id);c" }),
        sandbox.clone(),
    )
    .await;

    // Legal URL characters that mean something to a shell arrive single-quoted.
    let command = sandbox.curl();
    assert!(
        command.contains("'https://example.com/a'\\''b$(id);c'"),
        "{command}"
    );
    for flag in [
        "-g",
        "--fail",
        "--proto '=http,https'",
        "--proto-redir '=http,https'",
        "--max-time",
        "-o \"$f\"",
        "%{content_type}",
        "%{url_effective}",
        " -- '",
    ] {
        assert!(command.contains(flag), "{flag}");
    }
    // One shell probe, then one round-trip: the shell picks the scratch file, returns the body
    // as base64, and removes the file itself.
    assert_eq!(sandbox.commands(), [SHELL_PROBE.to_owned(), command.clone()]);
    assert!(command.starts_with("f=$(mktemp -t strands-web-fetch.XXXXXX) && "));
    assert!(command.ends_with("| base64; s=$?; rm -f \"$f\"; exit $s"));
    assert_eq!(command, golden()["sh"].as_str().unwrap());
}

#[tokio::test]
async fn handles_a_missing_content_type_without_shifting_the_trailer() {
    let sandbox = FakeSandbox::new(Response {
        body: b"a < b <i>".to_vec(),
        content_type: Some(""),
        url: Some("https://example.com/report.html"),
        ..Response::default()
    });
    let tool = web_fetch(&model());
    assert_eq!(
        invoke(&tool, json!({ "url": "https://example.com" }), sandbox).await,
        "a < b <i>"
    );
}

#[tokio::test]
async fn truncates_the_body_in_the_sandbox_instead_of_rejecting_large_pages() {
    let sandbox = FakeSandbox::new(page("x", "text/html"));
    let tool = web_fetch(&model());
    invoke(&tool, json!({ "url": "https://example.com" }), sandbox.clone()).await;
    assert!(sandbox.curl().contains("&& head -c 5242880 \"$f\" | base64"));
    assert!(!sandbox.curl().contains("--max-filesize"));
}

#[tokio::test]
async fn honours_the_response_charset_and_falls_back_to_utf_8_for_an_unknown_one() {
    let tool = web_fetch(&model());
    let latin1 = FakeSandbox::new(Response {
        body: b"caf\xe9".to_vec(),
        content_type: Some("text/plain; charset=ISO-8859-1"),
        ..Response::default()
    });
    assert_eq!(
        invoke(&tool, json!({ "url": "https://example.com/latin1" }), latin1).await,
        "café"
    );
    let unknown = FakeSandbox::new(page("é", "text/plain; charset=not-a-codec"));
    assert_eq!(
        invoke(&tool, json!({ "url": "https://example.com/unknown" }), unknown).await,
        "é"
    );
}

#[tokio::test]
async fn returns_the_raw_page_content_when_no_prompt_is_given() {
    let sandbox = FakeSandbox::new(page("body text", "text/plain"));
    let model = model();
    let tool = web_fetch(&model);
    let without_prompt = invoke(&tool, json!({ "url": "https://example.com" }), sandbox.clone()).await;
    assert_eq!(without_prompt, "body text");
    let blank = json!({ "url": "https://example.com", "prompt": "   " });
    assert_eq!(invoke(&tool, blank, sandbox).await, "body text");
    assert!(invoke_calls(&model).is_empty());
}

#[tokio::test]
async fn reports_a_curl_failure_without_invoking_the_summarizer_and_still_cleans_up() {
    let sandbox = FakeSandbox::new(Response {
        exit_code: 22,
        stderr: "curl: (22) The requested URL returned error: 500\n",
        ..Response::default()
    });
    let model = model();
    let tool = web_fetch(&model);
    let input = json!({ "url": "https://example.com", "prompt": "anything" });
    let answer = invoke(&tool, input, sandbox.clone()).await;

    assert_eq!(
        answer,
        "Failed to fetch https://example.com: curl: (22) The requested URL returned error: 500"
    );
    assert!(invoke_calls(&model).is_empty());
    assert_eq!(sandbox.commands().len(), 2);
}

#[tokio::test]
async fn probes_the_shell_once_per_sandbox() {
    let sandbox = FakeSandbox::new(page("page", "text/plain"));
    let tool = web_fetch(&model());
    invoke(&tool, json!({ "url": "https://example.com/a" }), sandbox.clone()).await;
    invoke(&tool, json!({ "url": "https://example.com/b" }), sandbox.clone()).await;
    let commands = sandbox.commands();
    assert_eq!(
        commands.iter().filter(|command| *command == SHELL_PROBE).count(),
        1
    );
    assert_eq!(commands.len(), 3);
}

#[tokio::test]
async fn runs_curl_exe_through_powershell_and_reads_crlf_output() {
    let sandbox = FakeSandbox::powershell(page("Hello wörld", "text/plain; charset=utf-8"));
    let tool = web_fetch(&model());
    assert_eq!(
        invoke(
            &tool,
            json!({ "url": "https://example.com/it's" }),
            sandbox.clone()
        )
        .await,
        "Hello wörld"
    );
    let command = sandbox.curl();
    assert!(command.contains(
        "\n  curl.exe -sSL -g --fail --proto '=http,https' --proto-redir '=http,https' --max-time "
    ));
    // The URL is a single-quoted PowerShell literal (no expansion), with its own quotes doubled.
    assert_eq!(command.matches("'https://example.com/it''s'").count(), 1);
    assert!(command.contains("$([guid]::NewGuid())"));
    assert!(command.contains("strands-web-fetch-"));
    assert!(command.contains(".ReadBytes([Math]::Min($in.Length, 5242880))"));
    assert!(command.ends_with("exit $s"));
    // Failures reach stderr through the console handle rather than PowerShell's CLIXML-wrapped
    // error stream, and a curl.exe that cannot run is exit 127.
    assert!(command.contains("\n$s = 127\ntry {\n"));
    assert!(
        command
            .contains("} catch { if ($s -eq 0) { $s = 1 }; [Console]::Error.Write($_.Exception.Message) }")
    );
    // Both the stderr check and the cleanup take the temp path literally.
    assert_eq!(command.matches("-LiteralPath").count(), 2);
    assert!(!command.contains("mktemp"));
    assert!(!command.contains("&&"));
    assert_eq!(command, golden()["ps"].as_str().unwrap());
}

#[tokio::test]
async fn reports_a_powershell_curl_failure() {
    let sandbox = FakeSandbox::powershell(Response {
        exit_code: 22,
        stderr: "curl: (22) The requested URL returned error: 404\r\n",
        ..Response::default()
    });
    let tool = web_fetch(&model());
    assert_eq!(
        invoke(&tool, json!({ "url": "https://example.com/missing" }), sandbox).await,
        "Failed to fetch https://example.com/missing: curl: (22) The requested URL returned error: 404"
    );
}

#[tokio::test]
async fn reports_a_sandbox_failure_as_a_fetch_failure() {
    let sandbox = Arc::new(FakeSandbox {
        fails: true,
        ..FakeSandbox::default()
    });
    let tool = web_fetch(&model());
    let answer = invoke(&tool, json!({ "url": "https://example.com" }), sandbox).await;
    assert_eq!(answer, "Failed to fetch https://example.com: sandbox gone");
}

#[tokio::test]
async fn rejects_a_non_http_scheme_before_touching_the_sandbox() {
    let sandbox = FakeSandbox::new(Response::default());
    let tool = web_fetch(&model());
    let input = json!({ "url": "ftp://example.com/f", "prompt": "x" });
    let answer = invoke(&tool, input, sandbox.clone()).await;
    assert_eq!(
        answer,
        "Failed to fetch ftp://example.com/f: web_fetch only supports http(s) URLs, got \"ftp://example.com/f\"."
    );
    assert!(sandbox.commands().is_empty());
}

#[tokio::test]
async fn rejects_the_malformed_url_before_touching_the_sandbox() {
    const CHARACTERS: &str = "web_fetch URLs may only contain the characters RFC 3986 allows; percent-encode \
        spaces and non-ASCII characters (and punycode the host) and retry.";
    let cases = [
        ("https://example.com/a b", CHARACTERS),
        ("https://example.com/x`id`", CHARACTERS),
        ("https://example.com/\"x\"", CHARACTERS),
        ("https://example.com/x\nrm -rf /", CHARACTERS),
        ("https://example.com/{a,b}", CHARACTERS),
        ("https://example.com/日本", CHARACTERS),
        (
            "https:///no-host",
            "web_fetch URL has no host: \"https:///no-host\".",
        ),
        (
            "javascript:alert(1)",
            "web_fetch only supports http(s) URLs, got \"javascript:alert(1)\".",
        ),
        ("", CHARACTERS),
    ];
    for (url, reason) in cases {
        let sandbox = FakeSandbox::new(Response::default());
        let tool = web_fetch(&model());
        let answer = invoke(&tool, json!({ "url": url }), sandbox.clone()).await;
        assert_eq!(answer, format!("Failed to fetch {url}: {reason}"));
        assert!(sandbox.commands().is_empty(), "{url}");
    }
}

#[tokio::test]
async fn accepts_rfc_3986_urls_and_trims_whitespace() {
    let sandbox = FakeSandbox::new(page("ok", "text/plain"));
    let tool = web_fetch(&model());
    let input = json!({ "url": " https://e.com/a?b=c&d=%20#f " });
    assert_eq!(invoke(&tool, input, sandbox.clone()).await, "ok");
    assert!(sandbox.curl().contains("'https://e.com/a?b=c&d=%20#f'"));
    assert_eq!(
        invoke(&tool, json!({ "url": "http://[::1]:8080/x" }), sandbox).await,
        "ok"
    );
}

/// What a test server sends for one request.
struct Reply {
    status: &'static str,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

impl Reply {
    fn new(status: &'static str, content_type: Option<&'static str>, body: impl Into<Vec<u8>>) -> Self {
        let headers = content_type
            .map(|value| ("Content-Type", value.to_owned()))
            .into_iter()
            .collect();
        Self {
            status,
            headers,
            body: body.into(),
        }
    }
}

/// A localhost HTTP/1.1 server answering each request with `route(path)`. Returns its base URL
/// and the request heads it received.
async fn serve(route: impl Fn(&str) -> Reply + Send + Sync + 'static) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let heads = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&heads);
    let route = Arc::new(route);
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let (seen, route) = (Arc::clone(&seen), Arc::clone(&route));
            tokio::spawn(async move {
                let head = read_head(&mut socket).await;
                let path = head.split(' ').nth(1).unwrap_or_default().to_owned();
                seen.lock().unwrap().push(head);
                let reply = route(&path);
                let mut response = format!("HTTP/1.1 {}\r\nConnection: close\r\n", reply.status);
                for (name, value) in &reply.headers {
                    response.push_str(&format!("{name}: {value}\r\n"));
                }
                response.push_str(&format!("Content-Length: {}\r\n\r\n", reply.body.len()));
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.write_all(&reply.body).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    (base, heads)
}

async fn read_head(socket: &mut tokio::net::TcpStream) -> String {
    let mut head = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        match socket.read(&mut buffer).await {
            Ok(read) if read > 0 => head.extend_from_slice(&buffer[..read]),
            _ => break,
        }
    }
    String::from_utf8_lossy(&head).into_owned()
}

#[tokio::test]
async fn fetches_from_the_process_with_the_direct_transport_skipping_the_sandbox() {
    let (base, heads) = serve(|_| {
        Reply::new(
            "200 OK",
            Some("text/html; charset=utf-8"),
            "<p>Hi <b>there</b></p>",
        )
    })
    .await;
    let sandbox = FakeSandbox::new(Response::default());
    let tool = direct(&model());
    assert_eq!(
        invoke(&tool, json!({ "url": base }), sandbox.clone()).await,
        "Hi there"
    );
    let seen = heads.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].starts_with("GET / HTTP/1.1\r\n"), "{}", seen[0]);
    assert!(
        seen[0]
            .to_lowercase()
            .contains("\r\nuser-agent: strands-harness/1.0\r\n"),
        "{}",
        seen[0]
    );
    assert!(sandbox.commands().is_empty());
    let answer = invoke(&tool, json!({ "url": format!("{base}/a b") }), sandbox).await;
    assert!(answer.contains("Failed to fetch"), "{answer}");
    assert_eq!(heads.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn caps_a_direct_transport_body_without_buffering_the_rest_of_the_stream() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let pulls = Arc::new(AtomicUsize::new(0));
    let cancelled = Arc::new(AtomicBool::new(false));
    let (server_pulls, server_cancelled) = (Arc::clone(&pulls), Arc::clone(&cancelled));
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_head(&mut socket).await;
        let head = "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: text/plain\r\n\r\n";
        socket.write_all(head.as_bytes()).await.unwrap();
        let chunk = vec![0x78_u8; 64 * 1024];
        // An endless body: the write fails once the client drops the connection.
        while socket.write_all(&chunk).await.is_ok() {
            server_pulls.fetch_add(1, Ordering::SeqCst);
        }
        server_cancelled.store(true, Ordering::SeqCst);
    });

    let tool = direct(&model());
    let sandbox = FakeSandbox::new(Response::default());
    let text = invoke(&tool, json!({ "url": format!("{base}/endless") }), sandbox).await;
    assert_eq!(text, "x".repeat(50_000));
    while !cancelled.load(Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }
    // 5 MiB is 80 chunks; the rest is what the socket buffers held when the client hung up.
    assert!(pulls.load(Ordering::SeqCst) < 1000, "{pulls:?}");
}

#[tokio::test]
async fn caches_the_fetched_page_within_the_ttl() {
    let sandbox = FakeSandbox::new(page("body text", "text/plain"));
    let model = model();
    let tool = web_fetch(&model);
    let first = json!({ "url": "https://example.com", "prompt": "first" });
    invoke(&tool, first, sandbox.clone()).await;
    let second = json!({ "url": "https://example.com", "prompt": "second" });
    invoke(&tool, second, sandbox.clone()).await;
    assert_eq!(sandbox.commands().len(), 2);
    assert_eq!(invoke_calls(&model).len(), 2);
}

/// The local environment with `TMPDIR` set to a directory of its own, so the leftover check
/// doesn't see the scratch files of other test processes.
struct ScratchSandbox {
    local: NotASandboxLocalEnvironment,
    tmpdir: String,
}

#[async_trait]
impl Sandbox for ScratchSandbox {
    fn execute_streaming<'a>(
        &'a self,
        command: &'a str,
        mut options: ExecuteOptions,
    ) -> BoxStream<'a, strands::Result<ExecutionEvent>> {
        options
            .env
            .get_or_insert_default()
            .insert("TMPDIR".to_owned(), self.tmpdir.clone());
        self.local.execute_streaming(command, options)
    }

    fn execute_code_streaming<'a>(
        &'a self,
        code: &'a str,
        language: &'a str,
        options: ExecuteOptions,
    ) -> BoxStream<'a, strands::Result<ExecutionEvent>> {
        self.local.execute_code_streaming(code, language, options)
    }

    async fn read_file(&self, path: &str) -> strands::Result<Vec<u8>> {
        self.local.read_file(path).await
    }

    async fn write_file(&self, path: &str, content: &[u8]) -> strands::Result<()> {
        self.local.write_file(path, content).await
    }

    async fn remove_file(&self, path: &str) -> strands::Result<()> {
        self.local.remove_file(path).await
    }

    async fn list_files(&self, path: &str) -> strands::Result<Vec<FileInfo>> {
        self.local.list_files(path).await
    }
}

#[tokio::test]
async fn fetches_end_to_end_through_the_local_environment_sandbox() {
    const PAGE: &str = "<html><body><script>x</script><p>Hello <b>wörld</b></p></body></html>";
    let (base, _) = serve(|path| match path {
        "/redirect" => Reply {
            status: "302 Found",
            headers: vec![("Location", "/page".to_owned())],
            body: Vec::new(),
        },
        "/huge" => Reply::new("200 OK", Some("text/plain"), vec![b'x'; 6 * 1024 * 1024]),
        "/notype" => Reply::new("200 OK", None, "a < b"),
        // Well past the sandbox's read chunks: multi-byte characters must survive intact.
        "/big" => Reply::new("200 OK", Some("text/plain; charset=utf-8"), "日".repeat(100_000)),
        "/missing" => Reply::new("404 Not Found", Some("text/html; charset=utf-8"), PAGE),
        _ => Reply::new("200 OK", Some("text/html; charset=utf-8"), PAGE),
    })
    .await;
    let scratch = tempfile::tempdir().unwrap();
    let sandbox: Arc<dyn Sandbox> = Arc::new(ScratchSandbox {
        local: NotASandboxLocalEnvironment::new(),
        tmpdir: scratch.path().to_str().unwrap().to_owned(),
    });
    let tool = web_fetch(&model());
    let fetch = |path: &str| invoke(&tool, json!({ "url": format!("{base}{path}") }), sandbox.clone());

    assert_eq!(fetch("/redirect").await, "Hello wörld");
    assert_eq!(fetch("/big").await, "日".repeat(50_000));
    assert_eq!(fetch("/huge").await, "x".repeat(50_000));
    assert_eq!(fetch("/notype").await, "a < b");
    // The scratch directory the tool derives, which this sandbox points at a directory of its own.
    let leftovers = "ls \"${TMPDIR:-/tmp}\" | grep -c strands-web-fetch";
    let listed = sandbox
        .execute(leftovers, ExecuteOptions::default())
        .await
        .unwrap();
    assert_eq!(listed.stdout.trim(), "0");
    assert!(fetch("/missing").await.contains("404"));
}

// Rust-only: byte parity with the TypeScript source, and the cache expiry.

/// The TS `curlCommand`, `powershellCurlCommand` and `htmlToText` outputs, generated by running
/// the functions of `harness-ts/src/tools/web-fetch.ts` under Node.
fn golden() -> JsonValue {
    serde_json::from_str(include_str!("golden.json")).unwrap()
}

#[test]
fn html_to_text_matches_the_typescript_function() {
    let html = "<html><head><STYLE a='1'>p{}</STYLE></head><body><script>x=1</script>\
                <p>Hello  <b>world</b></p>\n\n \n\n\n<p>a\tb</p></body></html>";
    assert_eq!(super::html_to_text(html), golden()["html"].as_str().unwrap());
}

#[test]
fn the_tool_spec_matches_the_typescript_tool() {
    use strands::tools::Tool;
    let tool = web_fetch(&model());
    assert_eq!(tool.name(), "web_fetch");
    let spec = serde_json::to_value(tool.tool_spec()).unwrap();
    assert_eq!(
        spec["inputSchema"],
        json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "The http(s) URL to fetch." },
                "prompt": { "type": "string", "description": super::PROMPT_DESCRIPTION },
            },
            "required": ["url"],
            "additionalProperties": false,
        })
    );
    let Some(source) = crate::test_support::ts_source("tools/web-fetch.ts") else {
        return;
    };
    // The TS source splits each text over several concatenated literals.
    let flattened = regex::Regex::new(r"' \+\s*'").unwrap().replace_all(&source, "");
    for text in [
        super::DESCRIPTION,
        super::PROMPT_DESCRIPTION,
        super::SUMMARIZER_PROMPT,
    ] {
        assert!(flattened.contains(text), "{text}");
    }
}

#[tokio::test(start_paused = true)]
async fn refetches_once_the_cache_entry_is_15_minutes_old() {
    let sandbox = FakeSandbox::new(page("body text", "text/plain"));
    let tool = web_fetch(&model());
    let input = json!({ "url": "https://example.com" });
    invoke(&tool, input.clone(), sandbox.clone()).await;
    tokio::time::advance(std::time::Duration::from_secs(15 * 60 - 1)).await;
    invoke(&tool, input.clone(), sandbox.clone()).await;
    assert_eq!(sandbox.commands().len(), 2);
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    invoke(&tool, input, sandbox.clone()).await;
    assert_eq!(sandbox.commands().len(), 3);
}

#[test]
fn caps_text_at_50_000_utf_16_code_units() {
    assert_eq!(
        super::cap_chars("a".repeat(50_001), super::MAX_CHARS).len(),
        50_000
    );
    // An astral character is two units; one the cut would split is dropped.
    let astral = format!("a{}", "😀".repeat(25_000));
    assert_eq!(super::cap_chars(astral, super::MAX_CHARS).chars().count(), 25_000);
}

#[test]
fn validate_url_gives_the_typescript_reason_for_each_malformed_form() {
    let reason = |url: &str| super::validate_url(url).unwrap_err();
    assert_eq!(
        reason("https://"),
        "web_fetch could not parse the URL \"https://\"."
    );
    assert_eq!(
        reason("https://[::1"),
        "web_fetch could not parse the URL \"https://[::1\"."
    );
    assert_eq!(
        reason("http:example.com"),
        "web_fetch URL has no host: \"http:example.com\"."
    );
    assert_eq!(
        super::validate_url("HTTPS://EXAMPLE.com"),
        Ok("HTTPS://EXAMPLE.com")
    );
}

/// The URL cache drops expired entries and is capped.
#[test]
fn the_url_cache_drops_expired_entries_and_is_capped() {
    let page = || super::Fetched {
        resolved_url: String::new(),
        text: String::new(),
    };
    let start = tokio::time::Instant::now();
    let mut cache = std::collections::HashMap::new();
    super::remember(&mut cache, "old".to_owned(), None, page(), start);
    let later = start + super::CACHE_TTL;
    super::remember(&mut cache, "new".to_owned(), None, page(), later);
    assert_eq!(cache.keys().collect::<Vec<_>>(), ["new"]);
    for index in 0..super::MAX_CACHE_ENTRIES {
        let now = later + std::time::Duration::from_secs(1 + index as u64);
        super::remember(&mut cache, format!("page-{index}"), None, page(), now);
    }
    assert_eq!(cache.len(), super::MAX_CACHE_ENTRIES);
    assert!(!cache.contains_key("new"));
}

/// A tool shared by agents on different sandboxes fetches the page through each sandbox instead
/// of serving one sandbox's copy to the other.
#[tokio::test]
async fn does_not_serve_a_page_cached_through_one_sandbox_to_another() {
    let first = FakeSandbox::new(page("from the first sandbox", "text/plain"));
    let second = FakeSandbox::new(page("from the second sandbox", "text/plain"));
    let tool = web_fetch(&model());
    let input = || json!({ "url": "https://example.com/shared" });
    assert_eq!(
        invoke(&tool, input(), first.clone()).await,
        "from the first sandbox"
    );
    assert_eq!(
        invoke(&tool, input(), second.clone()).await,
        "from the second sandbox"
    );
    assert_eq!(
        invoke(&tool, input(), second.clone()).await,
        "from the second sandbox"
    );
    assert_eq!(first.commands().len(), 2);
    assert_eq!(second.commands().len(), 2);
}
