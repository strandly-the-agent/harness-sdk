//! Ported from strands-ts/src/vended-tools/http-request/__tests__/http-request.test.ts
//!
//! Requests go to a local HTTP server instead of a mocked `fetch`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::sync::{Arc, Mutex};

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;
use crate::sandbox::test_sandbox::{instant_sleeper, never_sleeper};
use crate::tools::Tool;
use crate::vended_tools::test_support::{cancellable_context, context};

/// One scripted response: status line, headers and body; `None` never answers.
type Reply = Option<(&'static str, Vec<(&'static str, &'static str)>, String)>;

/// A local server answering every request with `reply`; returns its base URL and the raw
/// requests it received.
async fn server(reply: Reply) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&requests);
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let raw = read_request(&mut socket).await;
            seen.lock().unwrap().push(raw);
            let Some((status, headers, body)) = reply.clone() else {
                std::mem::forget(socket);
                continue;
            };
            let mut response = format!(
                "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n",
                body.len()
            );
            for (name, value) in headers {
                response.push_str(&format!("{name}: {value}\r\n"));
            }
            response.push_str("\r\n");
            response.push_str(&body);
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });
    (url, requests)
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = socket.read(&mut buf).await.unwrap_or(0);
        data.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&data).into_owned();
        if let Some(end) = text.find("\r\n\r\n") {
            let length = text
                .lines()
                .find_map(|line| {
                    line.to_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if data.len() >= end + 4 + length || n == 0 {
                return text;
            }
        }
        if n == 0 {
            return text;
        }
    }
}

async fn invoke(input: JsonValue) -> Result<HttpRequestOutput, ToolError> {
    let value = http_request_with(never_sleeper())
        .invoke(input, context(None))
        .await?;
    Ok(serde_json::from_value(value).unwrap())
}

#[tokio::test]
async fn returns_successful_response() {
    let cases = [
        ("GET", "200 OK", 200, "OK"),
        ("POST", "201 Created", 201, "Created"),
        ("PUT", "200 OK", 200, "OK"),
        ("DELETE", "204 No Content", 204, "No Content"),
        ("PATCH", "200 OK", 200, "OK"),
        ("HEAD", "200 OK", 200, "OK"),
        ("OPTIONS", "200 OK", 200, "OK"),
    ];
    for (method, status_line, status, status_text) in cases {
        let body = if matches!(method, "HEAD" | "DELETE") {
            String::new()
        } else {
            "{\"success\":true}".to_owned()
        };
        let (url, _) = server(Some((
            status_line,
            vec![("content-type", "application/json")],
            body.clone(),
        )))
        .await;
        let result = invoke(json!({ "method": method, "url": format!("{url}/resource") }))
            .await
            .unwrap();
        assert_eq!(result.status, status, "{method}");
        assert_eq!(result.status_text, status_text, "{method}");
        assert_eq!(result.headers["content-type"], "application/json", "{method}");
        assert_eq!(result.body, body, "{method}");
    }
}

#[tokio::test]
async fn sends_request_with_custom_headers_and_body() {
    let (url, requests) = server(Some(("200 OK", vec![], "{\"id\":123}".to_owned()))).await;
    invoke(json!({
        "method": "POST",
        "url": format!("{url}/users"),
        "body": "{\"name\":\"test\"}",
        "headers": { "Content-Type": "application/json" },
    }))
    .await
    .unwrap();
    let request = requests.lock().unwrap()[0].clone();
    assert!(request.starts_with("POST /users HTTP/1.1\r\n"), "{request}");
    assert!(
        request
            .to_lowercase()
            .contains("content-type: application/json\r\n"),
        "{request}"
    );
    assert!(request.ends_with("\r\n\r\n{\"name\":\"test\"}"), "{request}");
}

#[tokio::test]
async fn handles_empty_response_body() {
    let (url, _) = server(Some(("200 OK", vec![], String::new()))).await;
    assert_eq!(
        invoke(json!({ "method": "GET", "url": url })).await.unwrap().body,
        ""
    );
}

#[tokio::test]
async fn handles_string_response_body() {
    let (url, _) = server(Some((
        "200 OK",
        vec![("content-type", "text/plain")],
        "Plain text response".to_owned(),
    )))
    .await;
    assert_eq!(
        invoke(json!({ "method": "GET", "url": url })).await.unwrap().body,
        "Plain text response"
    );
}

#[tokio::test]
async fn converts_response_headers_to_plain_object() {
    let headers = vec![
        ("content-type", "application/json"),
        ("x-custom-header", "custom-value"),
        ("x-request-id", "abc123"),
    ];
    let (url, _) = server(Some(("200 OK", headers, "{}".to_owned()))).await;
    let result = invoke(json!({ "method": "GET", "url": url })).await.unwrap();
    assert_eq!(result.headers["content-type"], "application/json");
    assert_eq!(result.headers["x-custom-header"], "custom-value");
    assert_eq!(result.headers["x-request-id"], "abc123");
}

#[tokio::test]
async fn joins_repeated_headers_but_keeps_the_last_set_cookie() {
    let headers = vec![
        ("set-cookie", "a=1"),
        ("set-cookie", "b=2"),
        ("x-y", "1"),
        ("x-y", "2"),
    ];
    let (url, _) = server(Some(("200 OK", headers, "{}".to_owned()))).await;
    let result = invoke(json!({ "method": "GET", "url": url })).await.unwrap();
    assert_eq!(result.headers["set-cookie"], "b=2");
    assert_eq!(result.headers["x-y"], "1, 2");
}

#[tokio::test]
async fn succeeds_for_2xx_status_codes() {
    let (url, _) = server(Some(("201 Created", vec![], "{}".to_owned()))).await;
    assert_eq!(
        invoke(json!({ "method": "POST", "url": url }))
            .await
            .unwrap()
            .status,
        201
    );
}

async fn status_error(status_line: &'static str) -> String {
    let (url, _) = server(Some((status_line, vec![], String::new()))).await;
    invoke(json!({ "method": "GET", "url": url }))
        .await
        .unwrap_err()
        .to_string()
}

#[tokio::test]
async fn throws_error_for_3xx_redirect_status_codes() {
    assert!(
        status_error("301 Moved Permanently")
            .await
            .starts_with("HTTP 301")
    );
}

#[tokio::test]
async fn throws_error_for_4xx_client_error_codes() {
    assert!(
        status_error("404 Not Found")
            .await
            .starts_with("HTTP 404 Not Found: GET http://")
    );
}

#[tokio::test]
async fn throws_error_for_5xx_server_error_codes() {
    assert!(
        status_error("500 Internal Server Error")
            .await
            .starts_with("HTTP 500")
    );
}

#[tokio::test]
async fn throws_timeout_error_when_request_exceeds_timeout() {
    let (url, _) = server(None).await;
    let error = http_request_with(instant_sleeper())
        .invoke(
            json!({ "method": "GET", "url": url, "timeout": 0.1 }),
            context(None),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("Request timed out after 0.1 seconds: GET {url}")
    );
}

#[tokio::test]
async fn throws_error_for_network_failures() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    assert!(invoke(json!({ "method": "GET", "url": url })).await.is_err());
}

#[tokio::test]
async fn uses_context_cancel_signal_when_it_differs_from_the_agent_signal() {
    let (url, _) = server(None).await;
    let (ctx, handle) = cancellable_context(None);
    let tool = http_request_with(never_sleeper());
    let call = tool.invoke(json!({ "method": "GET", "url": url }), ctx);
    let (result, ()) = tokio::join!(call, async {
        tokio::task::yield_now().await;
        handle.cancel(crate::CancelOptions::default());
    });
    assert_eq!(
        result.unwrap_err().to_string(),
        format!("Request cancelled: GET {url}")
    );
}

#[tokio::test]
async fn caps_the_response_body_while_streaming() {
    let (url, _) = server(Some(("200 OK", vec![], "x".repeat(MAX_BODY_BYTES + 1)))).await;
    let error = invoke(json!({ "method": "GET", "url": url })).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("Response body exceeds the 10485760-byte limit: GET {url}")
    );
}

#[tokio::test]
async fn rejects_an_invalid_url() {
    assert!(
        invoke(json!({ "method": "GET", "url": "not a url" }))
            .await
            .is_err()
    );
}

#[test]
fn exposes_the_zod_schema() {
    let tool = http_request();
    assert_eq!(tool.name(), "http_request");
    assert_eq!(
        tool.tool_spec().input_schema(),
        Some(&http_request_input_schema())
    );
}
