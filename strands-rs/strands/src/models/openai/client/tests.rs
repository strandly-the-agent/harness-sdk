//! The vendor SDK transport behaviour (`openai` `client.js`, `core/streaming.js`,
//! `core/error.js`): headers, the transport retry and its backoff, the request timeout, SSE
//! decoding and error wording, against a local HTTP server.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use futures::future::BoxFuture;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::*;
use crate::models::http_transport::parse_custom_headers;

/// One scripted HTTP response; `None` in a script never answers.
#[derive(Clone)]
struct Reply {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: String,
}

fn reply(status: u16, body: &str) -> Reply {
    Reply {
        status,
        headers: Vec::new(),
        body: body.to_owned(),
    }
}

fn sse(events: &[(&str, JsonValue)]) -> Reply {
    let body: String = events
        .iter()
        .map(|(name, data)| format!("event: {name}\ndata: {data}\n\n"))
        .collect();
    Reply {
        headers: vec![("content-type", "text/event-stream".to_owned())],
        ..reply(200, &body)
    }
}

/// A request the server received.
#[derive(Debug, Clone)]
struct Received {
    target: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Received {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

struct Server {
    url: String,
    received: Arc<Mutex<Vec<Received>>>,
}

impl Server {
    fn received(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }
}

/// Serves one scripted reply per connection, in order; the last one repeats.
async fn serve(replies: Vec<Option<Reply>>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let received = Arc::new(Mutex::new(Vec::new()));
    let log = received.clone();
    tokio::spawn(async move {
        let mut index = 0usize;
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            let reply = replies.get(index).or(replies.last()).cloned().flatten();
            index += 1;
            tokio::spawn(answer(socket, reply, log.clone()));
        }
    });
    Server { url, received }
}

async fn answer(mut socket: TcpStream, reply: Option<Reply>, log: Arc<Mutex<Vec<Received>>>) {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let read = socket.read(&mut chunk).await.unwrap_or(0);
        if read == 0 {
            return;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let target = lines
        .next()
        .unwrap_or_default()
        .split(' ')
        .nth(1)
        .unwrap_or_default()
        .to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(0);
    while buffer.len() < header_end + length {
        let read = socket.read(&mut chunk).await.unwrap_or(0);
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let body = String::from_utf8_lossy(&buffer[header_end..]).into_owned();
    log.lock().unwrap().push(Received {
        target,
        headers,
        body,
    });
    let Some(reply) = reply else {
        // Hold the connection open without answering.
        let _ = socket.read(&mut chunk).await;
        return;
    };
    let mut response = format!(
        "HTTP/1.1 {} X\r\ncontent-length: {}\r\nconnection: close\r\n",
        reply.status,
        reply.body.len()
    );
    for (name, value) in &reply.headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str("\r\n");
    response.push_str(&reply.body);
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;
}

/// Unnamed SSE events with `data:` lines, as OpenAI streams them, then `[DONE]`.
fn data_sse(events: &[JsonValue]) -> Reply {
    let mut body: String = events.iter().map(|data| format!("data: {data}\n\n")).collect();
    body.push_str("data: [DONE]\n\n");
    Reply {
        headers: vec![("content-type", "text/event-stream".to_owned())],
        ..reply(200, &body)
    }
}

/// A sleeper that records every wait and returns at once, except the default request timeout,
/// which never elapses.
fn recording_sleeper() -> (Arc<Mutex<Vec<Duration>>>, Sleeper) {
    let waits = Arc::new(Mutex::new(Vec::new()));
    let log = waits.clone();
    let sleeper: Sleeper = Arc::new(move |delay: Duration| -> BoxFuture<'static, ()> {
        log.lock().unwrap().push(delay);
        if delay >= DEFAULT_TIMEOUT {
            Box::pin(futures::future::pending())
        } else {
            Box::pin(futures::future::ready(()))
        }
    });
    (waits, sleeper)
}

fn client_parts(base_url: &str, sleeper: Sleeper) -> HttpClientParts {
    HttpClientParts {
        http: reqwest::Client::new(),
        base_url: base_url.to_owned(),
        key: KeySource::Static("test-key".to_owned()),
        headers: HeaderMap::new(),
        max_retries: DEFAULT_MAX_RETRIES,
        timeout: DEFAULT_TIMEOUT,
        sleeper,
        random: Arc::new(|| 0.0),
    }
}

fn api_request() -> ApiRequest {
    ApiRequest {
        path: RESPONSES_PATH,
        body: json!({ "model": "m1", "input": [], "stream": true }),
    }
}

async fn drain(client: &HttpClient, request: ApiRequest) -> (Vec<JsonValue>, Option<ApiError>) {
    let mut events = client.stream(request);
    let mut collected = Vec::new();
    while let Some(item) = events.next().await {
        match item {
            Ok(event) => collected.push(event),
            Err(error) => return (collected, Some(error)),
        }
    }
    (collected, None)
}

fn rate_limit_body() -> String {
    json!({ "error": { "message": "Rate limit reached", "type": "requests", "code": "rate_limit_exceeded" } })
        .to_string()
}

#[tokio::test]
async fn streams_sse_events_with_the_sdk_headers() {
    let server = serve(vec![Some(data_sse(&[
        json!({ "type": "response.created", "response": { "id": "r" } }),
        json!({ "type": "response.completed", "response": {} }),
    ]))])
    .await;
    let (_, sleeper) = recording_sleeper();
    let client = HttpClient::new(client_parts(&format!("{}/v1/", server.url), sleeper));
    let request = api_request();
    let (events, error) = drain(&client, request.clone()).await;
    assert!(error.is_none(), "{error:?}");
    assert_eq!(
        events,
        [
            json!({ "type": "response.created", "response": { "id": "r" } }),
            json!({ "type": "response.completed", "response": {} }),
        ]
    );
    let received = server.received();
    assert_eq!(received.len(), 1);
    let sent = &received[0];
    assert_eq!(sent.target, "/v1/responses");
    assert_eq!(sent.header("authorization"), Some("Bearer test-key"));
    assert_eq!(sent.header("content-type"), Some("application/json"));
    assert_eq!(sent.header("accept"), Some("application/json"));
    assert_eq!(
        serde_json::from_str::<JsonValue>(&sent.body).unwrap(),
        request.body
    );
}

#[tokio::test]
async fn ignores_events_after_done() {
    let mut reply = data_sse(&[json!({ "type": "response.created", "response": { "id": "r" } })]);
    reply.body.push_str("data: {\"type\":\"late\"}\n\n");
    let server = serve(vec![Some(reply)]).await;
    let (_, sleeper) = recording_sleeper();
    let client = HttpClient::new(client_parts(&server.url, sleeper));
    let (events, error) = drain(&client, api_request()).await;
    assert!(error.is_none());
    assert_eq!(
        events,
        [json!({ "type": "response.created", "response": { "id": "r" } })]
    );
}

#[tokio::test]
async fn sends_configured_headers_over_the_defaults() {
    let server = serve(vec![Some(data_sse(&[]))]).await;
    let (_, sleeper) = recording_sleeper();
    let mut headers = HeaderMap::new();
    headers.insert("openai-organization", HeaderValue::from_static("org-1"));
    headers.insert("accept", HeaderValue::from_static("text/event-stream"));
    let client = HttpClient::new(HttpClientParts {
        headers,
        ..client_parts(&server.url, sleeper)
    });
    drain(&client, api_request()).await;
    let sent = &server.received()[0];
    assert_eq!(sent.header("openai-organization"), Some("org-1"));
    assert_eq!(sent.header("accept"), Some("text/event-stream"));
}

#[tokio::test]
async fn retries_retryable_statuses_with_exponential_backoff() {
    let server = serve(vec![
        Some(reply(500, "{}")),
        Some(reply(429, &rate_limit_body())),
        Some(data_sse(&[json!({ "type": "response.completed" })])),
    ])
    .await;
    let (waits, sleeper) = recording_sleeper();
    let client = HttpClient::new(client_parts(&server.url, sleeper));
    let (events, error) = drain(&client, api_request()).await;
    assert!(error.is_none(), "{error:?}");
    assert_eq!(events.len(), 1);
    assert_eq!(server.received().len(), 3);
    let backoff: Vec<Duration> = waits
        .lock()
        .unwrap()
        .iter()
        .copied()
        .filter(|wait| *wait < DEFAULT_TIMEOUT)
        .collect();
    assert_eq!(backoff, [Duration::from_millis(500), Duration::from_secs(1)]);
}

#[tokio::test]
async fn gives_up_after_two_retries_with_the_sdk_error_message() {
    let server = serve(vec![Some(reply(429, &rate_limit_body()))]).await;
    let (_, sleeper) = recording_sleeper();
    let client = HttpClient::new(client_parts(&server.url, sleeper));
    let error = drain(&client, api_request()).await.1.unwrap();
    assert_eq!(server.received().len(), 3);
    assert_eq!(error.kind, ApiErrorKind::Api);
    assert_eq!(error.status, Some(429));
    assert_eq!(error.message, "429 Rate limit reached");
    assert_eq!(error.code.as_deref(), Some("rate_limit_exceeded"));
}

#[tokio::test]
async fn does_not_retry_a_bad_request() {
    let server = serve(vec![Some(reply(
        400,
        r#"{"error":{"message":"bad input","code":"invalid_value"}}"#,
    ))])
    .await;
    let (_, sleeper) = recording_sleeper();
    let client = HttpClient::new(client_parts(&server.url, sleeper));
    let error = drain(&client, api_request()).await.1.unwrap();
    assert_eq!(server.received().len(), 1);
    assert_eq!(error.message, "400 bad input");
}

#[tokio::test]
async fn obeys_x_should_retry() {
    let never = Reply {
        headers: vec![("x-should-retry", "false".to_owned())],
        ..reply(503, "{}")
    };
    let server = serve(vec![Some(never)]).await;
    let (_, sleeper) = recording_sleeper();
    let client = HttpClient::new(client_parts(&server.url, sleeper));
    drain(&client, api_request()).await;
    assert_eq!(server.received().len(), 1);

    let always = Reply {
        headers: vec![("x-should-retry", "true".to_owned())],
        ..reply(400, "{}")
    };
    let server = serve(vec![Some(always), Some(data_sse(&[]))]).await;
    let (_, sleeper) = recording_sleeper();
    let client = HttpClient::new(client_parts(&server.url, sleeper));
    let (_, error) = drain(&client, api_request()).await;
    assert!(error.is_none());
    assert_eq!(server.received().len(), 2);
}

#[tokio::test]
async fn waits_as_long_as_retry_after_asks() {
    let limited = Reply {
        headers: vec![("retry-after", "3".to_owned())],
        ..reply(429, &rate_limit_body())
    };
    let server = serve(vec![Some(limited), Some(data_sse(&[]))]).await;
    let (waits, sleeper) = recording_sleeper();
    let client = HttpClient::new(client_parts(&server.url, sleeper));
    drain(&client, api_request()).await;
    assert!(waits.lock().unwrap().contains(&Duration::from_secs(3)));
}

#[tokio::test]
async fn retries_connection_errors_then_reports_a_connection_error() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let (waits, sleeper) = recording_sleeper();
    let client = HttpClient::new(client_parts(&url, sleeper));
    let error = drain(&client, api_request()).await.1.unwrap();
    assert_eq!(error.kind, ApiErrorKind::Connection);
    assert_eq!(error.message, "Connection error.");
    let backoff = waits
        .lock()
        .unwrap()
        .iter()
        .filter(|wait| **wait < DEFAULT_TIMEOUT)
        .count();
    assert_eq!(backoff, 2);
}

#[tokio::test]
async fn times_out_waiting_for_the_response_headers() {
    let server = serve(vec![None]).await;
    let (waits, sleeper) = recording_sleeper();
    let timer: Sleeper = Arc::new(move |delay: Duration| -> BoxFuture<'static, ()> {
        let pending = delay == Duration::from_secs(7);
        let inner = sleeper(delay);
        if pending { Box::pin(async {}) } else { inner }
    });
    let client = HttpClient::new(HttpClientParts {
        timeout: Duration::from_secs(7),
        ..client_parts(&server.url, timer)
    });
    let error = drain(&client, api_request()).await.1.unwrap();
    assert_eq!(error.kind, ApiErrorKind::Timeout);
    assert_eq!(error.message, "Request timed out.");
    let timeouts = waits
        .lock()
        .unwrap()
        .iter()
        .filter(|wait| **wait == Duration::from_secs(7))
        .count();
    assert_eq!(timeouts, 3);
}

#[tokio::test]
async fn surfaces_a_data_error_as_an_api_error() {
    let server = serve(vec![Some(data_sse(&[
        json!({ "id": "c1", "choices": [] }),
        json!({ "error": { "message": "upstream broke", "code": "server_error" } }),
    ]))])
    .await;
    let (_, sleeper) = recording_sleeper();
    let client = HttpClient::new(client_parts(&server.url, sleeper));
    let (events, error) = drain(&client, api_request()).await;
    assert_eq!(events.len(), 1);
    let error = error.unwrap();
    assert_eq!(error.kind, ApiErrorKind::Api);
    assert_eq!(error.status, None);
    assert_eq!(error.message, "upstream broke");
    assert_eq!(error.code.as_deref(), Some("server_error"));
}

#[tokio::test]
async fn passes_a_responses_error_event_through_to_the_adapter() {
    let server = serve(vec![Some(sse(&[(
        "error",
        json!({ "type": "error", "code": "rate_limit_exceeded", "message": "slow down" }),
    )]))])
    .await;
    let (_, sleeper) = recording_sleeper();
    let client = HttpClient::new(client_parts(&server.url, sleeper));
    let (events, error) = drain(&client, api_request()).await;
    assert!(error.is_none());
    assert_eq!(events[0]["type"], "error");
}

#[tokio::test]
async fn runs_an_api_key_setter_before_every_attempt() {
    let server = serve(vec![Some(reply(500, "{}")), Some(data_sse(&[]))]).await;
    let (_, sleeper) = recording_sleeper();
    let calls = Arc::new(Mutex::new(0usize));
    let counter = Arc::clone(&calls);
    let setter: super::ApiKeySetter = Arc::new(move || {
        let mut calls = counter.lock().unwrap();
        *calls += 1;
        let token = format!("token-{calls}");
        Box::pin(async move { Ok(token) })
    });
    let client = HttpClient::new(HttpClientParts {
        key: KeySource::Setter(setter),
        ..client_parts(&server.url, sleeper)
    });
    drain(&client, api_request()).await;
    let received = server.received();
    assert_eq!(received[0].header("authorization"), Some("Bearer token-1"));
    assert_eq!(received[1].header("authorization"), Some("Bearer token-2"));
}

#[tokio::test]
async fn words_api_key_setter_failures_like_the_sdk() {
    let (_, sleeper) = recording_sleeper();
    let failing: super::ApiKeySetter = Arc::new(|| Box::pin(async { Err("no credentials".into()) }));
    let client = HttpClient::new(HttpClientParts {
        key: KeySource::Setter(failing),
        ..client_parts("http://127.0.0.1:9", Arc::clone(&sleeper))
    });
    let error = drain(&client, api_request()).await.1.unwrap();
    assert_eq!(error.kind, ApiErrorKind::Other);
    assert_eq!(
        error.message,
        "Failed to get token from 'apiKey' function: no credentials"
    );

    let empty: super::ApiKeySetter = Arc::new(|| Box::pin(async { Ok(String::new()) }));
    let client = HttpClient::new(HttpClientParts {
        key: KeySource::Setter(empty),
        ..client_parts("http://127.0.0.1:9", sleeper)
    });
    let error = drain(&client, api_request()).await.1.unwrap();
    assert_eq!(
        error.message,
        "Expected 'apiKey' function argument to return a string but it returned "
    );
}

#[test]
fn retry_after_follows_the_sdk_and_node_timer_rules() {
    let headers = |pairs: &[(&'static str, &str)]| {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(*name, HeaderValue::from_str(value).unwrap());
        }
        headers
    };
    assert_eq!(retry_after(&headers(&[])), None);
    assert_eq!(
        retry_after(&headers(&[("retry-after-ms", "250")])),
        Some(Duration::from_millis(250))
    );
    assert_eq!(
        retry_after(&headers(&[("retry-after-ms", "250"), ("retry-after", "9")])),
        Some(Duration::from_millis(250))
    );
    assert_eq!(
        retry_after(&headers(&[("retry-after-ms", "0"), ("retry-after", "2")])),
        Some(Duration::from_secs(2))
    );
    assert_eq!(
        retry_after(&headers(&[("retry-after-ms", "0")])),
        Some(Duration::ZERO)
    );
    assert_eq!(
        retry_after(&headers(&[("retry-after", "1.5")])),
        Some(Duration::from_millis(1500))
    );
    assert_eq!(retry_after(&headers(&[("retry-after-ms", "abc")])), None);
    // Node runs a negative, unparseable-date or overlong timer after 1 ms.
    assert_eq!(
        retry_after(&headers(&[("retry-after", "-5")])),
        Some(Duration::from_millis(1))
    );
    assert_eq!(
        retry_after(&headers(&[("retry-after", "soon")])),
        Some(Duration::from_millis(1))
    );
    assert_eq!(
        retry_after(&headers(&[("retry-after", "9999999999")])),
        Some(Duration::from_millis(1))
    );
}

#[test]
fn parses_openai_custom_headers_lines() {
    let headers = parse_custom_headers(
        "x-one: 1\nbroken line\nx-two:  two words ",
        "OPENAI_CUSTOM_HEADERS",
    );
    assert_eq!(headers.get("x-one").unwrap(), "1");
    assert_eq!(headers.get("x-two").unwrap(), "two words");
    assert_eq!(headers.len(), 2);
}

#[test]
fn words_status_errors_like_the_sdk() {
    let cases = [
        (r#"{"error":{"message":"bad"}}"#, "400 bad"),
        (r#"{"error":{"message":{"detail":"x"}}}"#, r#"400 {"detail":"x"}"#),
        (r#"{"error":{"code":"x"}}"#, r#"400 {"code":"x"}"#),
        (r#"{"detail":"no error key"}"#, "400 status code (no body)"),
        ("plain text failure", "400 plain text failure"),
        ("", "400 status code (no body)"),
    ];
    for (body, message) in cases {
        assert_eq!(ApiError::from_status(400, body).message, message, "{body}");
    }
    assert_eq!(ApiError::from_stream_error(&json!({})).message, "{}");
}

#[test]
fn redacts_the_api_key_in_debug() {
    let (_, sleeper) = recording_sleeper();
    let parts = HttpClientParts {
        key: KeySource::Static("sk-very-secret".to_owned()),
        ..client_parts("http://localhost", sleeper)
    };
    let printed = format!("{parts:?} {:?}", HttpClient::new(parts.clone()));
    assert!(!printed.contains("sk-very-secret"), "{printed}");
}
