//! The vendor SDK transport behaviour (`@anthropic-ai/sdk` `client.js`, `core/streaming.js`,
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
        base_url: base_url.to_owned(),
        api_key: "test-key".to_owned(),
        headers: HeaderMap::new(),
        max_retries: DEFAULT_MAX_RETRIES,
        timeout: DEFAULT_TIMEOUT,
        sleeper,
        random: Arc::new(|| 0.0),
    }
}

fn api_request() -> ApiRequest {
    ApiRequest {
        body: json!({ "model": "m1", "max_tokens": 1, "messages": [], "stream": true }),
        beta: None,
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
    json!({ "type": "error", "error": { "type": "rate_limit_error", "message": "Rate limited" } }).to_string()
}

#[tokio::test]
async fn streams_sse_events_with_the_sdk_headers() {
    let server = serve(vec![Some(sse(&[
        (
            "message_start",
            json!({ "type": "message_start", "message": { "role": "assistant" } }),
        ),
        ("ping", json!({ "type": "ping" })),
        ("message_stop", json!({ "type": "message_stop" })),
    ]))])
    .await;
    let (_, sleeper) = recording_sleeper();
    let client = HttpClient::new(client_parts(&format!("{}/", server.url), sleeper)).unwrap();
    let request = ApiRequest {
        beta: Some("beta-one,beta-two".to_owned()),
        ..api_request()
    };
    let (events, error) = drain(&client, request.clone()).await;
    assert!(error.is_none(), "{error:?}");
    assert_eq!(
        events,
        [
            json!({ "type": "message_start", "message": { "role": "assistant" } }),
            json!({ "type": "message_stop" }),
        ]
    );
    let received = server.received();
    assert_eq!(received.len(), 1);
    let sent = &received[0];
    assert_eq!(sent.target, "/v1/messages");
    assert_eq!(sent.header("x-api-key"), Some("test-key"));
    assert_eq!(sent.header("anthropic-version"), Some("2026-10-21"));
    assert_eq!(sent.header("content-type"), Some("application/json"));
    assert_eq!(sent.header("anthropic-beta"), Some("beta-one,beta-two"));
    assert_eq!(
        serde_json::from_str::<JsonValue>(&sent.body).unwrap(),
        request.body
    );
}

#[tokio::test]
async fn sends_configured_headers_over_the_defaults() {
    let server = serve(vec![Some(sse(&[]))]).await;
    let (_, sleeper) = recording_sleeper();
    let mut headers = HeaderMap::new();
    headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
    headers.insert("x-custom", HeaderValue::from_static("yes"));
    headers.insert("anthropic-beta", HeaderValue::from_static("client-beta"));
    headers.insert("x-api-key", HeaderValue::from_static("header-key"));
    let client = HttpClient::new(HttpClientParts {
        headers,
        ..client_parts(&server.url, sleeper)
    })
    .unwrap();
    drain(&client, api_request()).await;
    let request = ApiRequest {
        beta: Some("request-beta".to_owned()),
        ..api_request()
    };
    drain(&client, request).await;
    let received = server.received();
    assert_eq!(received[0].header("anthropic-version"), Some("2023-06-01"));
    assert_eq!(received[0].header("x-custom"), Some("yes"));
    assert_eq!(received[0].header("anthropic-beta"), Some("client-beta"));
    // Configured headers replace the defaults and the key, and request headers replace both,
    // rather than adding a second header of the same name.
    let values = |name: &str| -> Vec<String> {
        received[1]
            .headers
            .iter()
            .filter(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
            .collect()
    };
    assert_eq!(values("anthropic-beta"), ["request-beta"]);
    assert_eq!(values("x-api-key"), ["header-key"]);
}

#[tokio::test]
async fn retries_retryable_statuses_with_exponential_backoff() {
    let server = serve(vec![Some(reply(529, "")), Some(reply(500, "")), Some(sse(&[]))]).await;
    let (waits, sleeper) = recording_sleeper();
    let client = HttpClient::new(client_parts(&server.url, sleeper)).unwrap();
    let (_, error) = drain(&client, api_request()).await;
    assert!(error.is_none(), "{error:?}");
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
    let client = HttpClient::new(client_parts(&server.url, sleeper)).unwrap();
    let (_, error) = drain(&client, api_request()).await;
    let error = error.unwrap();
    assert_eq!(error.status(), Some(429));
    assert_eq!(error.message, format!("429 {}", rate_limit_body()));
    assert_eq!(error.api_message(), "Rate limited");
    assert_eq!(server.received().len(), 3);
}

#[tokio::test]
async fn does_not_retry_a_bad_request() {
    let server = serve(vec![Some(reply(400, "plain text failure"))]).await;
    let (_, sleeper) = recording_sleeper();
    let client = HttpClient::new(client_parts(&server.url, sleeper)).unwrap();
    let (_, error) = drain(&client, api_request()).await;
    assert_eq!(error.unwrap().message, "400 plain text failure");
    assert_eq!(server.received().len(), 1);
}

#[tokio::test]
async fn obeys_x_should_retry() {
    let forced = Reply {
        headers: vec![("x-should-retry", "true".to_owned())],
        ..reply(400, "")
    };
    let server = serve(vec![Some(forced), Some(sse(&[]))]).await;
    let (_, sleeper) = recording_sleeper();
    let client = HttpClient::new(client_parts(&server.url, sleeper.clone())).unwrap();
    assert!(drain(&client, api_request()).await.1.is_none());
    assert_eq!(server.received().len(), 2);

    let refused = Reply {
        headers: vec![("x-should-retry", "false".to_owned())],
        ..reply(503, "")
    };
    let server = serve(vec![Some(refused)]).await;
    let client = HttpClient::new(client_parts(&server.url, sleeper)).unwrap();
    let error = drain(&client, api_request()).await.1.unwrap();
    assert_eq!(error.message, "503 status code (no body)");
    assert_eq!(server.received().len(), 1);
}

#[tokio::test]
async fn waits_as_long_as_retry_after_asks() {
    let throttled = Reply {
        headers: vec![("retry-after", "3".to_owned())],
        ..reply(429, "")
    };
    let server = serve(vec![Some(throttled), Some(sse(&[]))]).await;
    let (waits, sleeper) = recording_sleeper();
    let client = HttpClient::new(client_parts(&server.url, sleeper)).unwrap();
    drain(&client, api_request()).await;
    assert!(waits.lock().unwrap().contains(&Duration::from_secs(3)));
}

#[tokio::test]
async fn retries_connection_errors_then_reports_a_connection_error() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let (waits, sleeper) = recording_sleeper();
    let client = HttpClient::new(client_parts(&url, sleeper)).unwrap();
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
    let (waits, _) = recording_sleeper();
    let log = waits.clone();
    let sleeper: Sleeper = Arc::new(move |delay| {
        log.lock().unwrap().push(delay);
        Box::pin(futures::future::ready(()))
    });
    let client = HttpClient::new(HttpClientParts {
        timeout: Duration::from_secs(7),
        ..client_parts(&server.url, sleeper)
    })
    .unwrap();
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
async fn surfaces_a_stream_error_event_as_an_api_error() {
    let overloaded =
        json!({ "type": "error", "error": { "type": "overloaded_error", "message": "Overloaded" } });
    let server = serve(vec![Some(sse(&[
        (
            "message_start",
            json!({ "type": "message_start", "message": { "role": "assistant" } }),
        ),
        ("error", overloaded.clone()),
    ]))])
    .await;
    let (_, sleeper) = recording_sleeper();
    let client = HttpClient::new(client_parts(&server.url, sleeper)).unwrap();
    let (events, error) = drain(&client, api_request()).await;
    assert_eq!(events.len(), 1);
    let error = error.unwrap();
    assert_eq!(error.kind, ApiErrorKind::Api);
    assert_eq!(error.status(), None);
    assert_eq!(error.message, overloaded.to_string());
}

#[tokio::test]
async fn counts_tokens_on_the_count_tokens_endpoint() {
    let server = serve(vec![Some(reply(200, r#"{"input_tokens":12}"#))]).await;
    let (_, sleeper) = recording_sleeper();
    let client = HttpClient::new(client_parts(&server.url, sleeper)).unwrap();
    let response = client.count_tokens(api_request()).await.unwrap();
    assert_eq!(response, json!({ "input_tokens": 12 }));
    assert_eq!(server.received()[0].target, "/v1/messages/count_tokens");
}

#[tokio::test]
async fn accumulates_the_beta_stream_into_its_final_message() {
    let server = serve(vec![Some(sse(&[
        (
            "message_start",
            json!({ "type": "message_start", "message": { "role": "assistant", "content": [], "usage": { "input_tokens": 0 } } }),
        ),
        (
            "content_block_start",
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "compaction", "content": null } }),
        ),
        (
            "content_block_delta",
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "compaction_delta", "content": "the summary" } }),
        ),
        ("content_block_stop", json!({ "type": "content_block_stop", "index": 0 })),
        (
            "message_delta",
            json!({ "type": "message_delta", "delta": { "stop_reason": "compaction" }, "usage": { "output_tokens": 9 } }),
        ),
        ("message_stop", json!({ "type": "message_stop" })),
    ]))])
    .await;
    let (_, sleeper) = recording_sleeper();
    let client = HttpClient::new(client_parts(&server.url, sleeper)).unwrap();
    let message = client.beta_final_message(api_request()).await.unwrap();
    assert_eq!(
        message,
        json!({
            "role": "assistant",
            "content": [{ "type": "compaction", "content": "the summary" }],
            "usage": { "input_tokens": 0, "output_tokens": 9 },
            "stop_reason": "compaction",
        })
    );
    assert_eq!(server.received()[0].target, "/v1/messages?beta=true");
}

#[tokio::test]
async fn fails_a_beta_stream_that_ends_without_message_stop() {
    let server = serve(vec![Some(sse(&[
        (
            "message_start",
            json!({ "type": "message_start", "message": { "role": "assistant", "content": [], "usage": { "input_tokens": 0 } } }),
        ),
        (
            "content_block_start",
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "compaction", "content": null } }),
        ),
        (
            "content_block_delta",
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "compaction_delta", "content": "the sum" } }),
        ),
    ]))])
    .await;
    let (_, sleeper) = recording_sleeper();
    let client = HttpClient::new(client_parts(&server.url, sleeper)).unwrap();
    let error = client.beta_final_message(api_request()).await.unwrap_err();
    assert_eq!(error.kind, ApiErrorKind::Other);
    assert_eq!(
        error.message,
        "stream ended without producing a Message with role=assistant"
    );
}

#[test]
fn should_retry_follows_the_sdk_status_rules() {
    let none = HeaderMap::new();
    for status in [408, 409, 429, 500, 502, 529] {
        assert!(should_retry_status(status, &none), "{status}");
    }
    for status in [400, 401, 403, 404, 413, 422] {
        assert!(!should_retry_status(status, &none), "{status}");
    }
}

#[test]
fn retry_after_prefers_milliseconds_and_ignores_unusable_values() {
    let headers = |pairs: &[(&'static str, &'static str)]| {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_static(value));
        }
        map
    };
    assert_eq!(
        retry_after(&headers(&[("retry-after-ms", "250")])),
        Some(Duration::from_millis(250))
    );
    assert_eq!(
        retry_after(&headers(&[("retry-after-ms", "250"), ("retry-after", "9")])),
        Some(Duration::from_millis(250))
    );
    assert_eq!(
        retry_after(&headers(&[("retry-after", "1.5")])),
        Some(Duration::from_millis(1500))
    );
    assert_eq!(retry_after(&headers(&[("retry-after", "-1")])), None);
    assert_eq!(retry_after(&headers(&[("retry-after", "soon")])), None);
    assert_eq!(
        retry_after(&headers(&[("retry-after", "Wed, 21 Oct 2015 07:28:00 GMT")])),
        None
    );
    assert_eq!(retry_after(&headers(&[("retry-after", "99999999")])), None);
}

#[test]
fn backoff_doubles_up_to_eight_seconds_with_up_to_a_quarter_of_jitter() {
    assert_eq!(default_retry_delay(2, 2, 0.0), Duration::from_millis(500));
    assert_eq!(default_retry_delay(1, 2, 0.0), Duration::from_secs(1));
    assert_eq!(default_retry_delay(0, 10, 0.0), Duration::from_secs(8));
    assert_eq!(default_retry_delay(2, 2, 1.0), Duration::from_millis(375));
}

#[test]
fn parses_anthropic_custom_headers_lines() {
    let headers = parse_custom_headers("anthropic-version: 2023-06-01\nx-team :  core \nnot a header\n");
    assert_eq!(headers.get("anthropic-version").unwrap(), "2023-06-01");
    assert_eq!(headers.get("x-team").unwrap(), "core");
    assert_eq!(headers.len(), 2);
}

#[test]
fn words_status_errors_like_the_sdk() {
    assert_eq!(
        ApiError::from_status(500, "").message,
        "500 status code (no body)"
    );
    assert_eq!(ApiError::from_status(404, "not here").message, "404 not here");
    assert_eq!(
        ApiError::from_status(400, r#"{"message":"top level"}"#).message,
        "400 top level"
    );
    let nested =
        r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long"}}"#;
    let error = ApiError::from_status(400, nested);
    assert_eq!(error.message, format!("400 {nested}"));
    assert_eq!(error.api_message(), "prompt is too long");
}

#[test]
fn redacts_the_api_key_in_debug() {
    let (_, sleeper) = recording_sleeper();
    let client = HttpClient::new(client_parts("http://localhost", sleeper)).unwrap();
    assert!(!format!("{client:?}").contains("test-key"));
}

mod through_the_model {
    use super::*;
    use crate::error::Error;
    use crate::models::anthropic::{
        AnthropicClientConfig, AnthropicModel, AnthropicModelConfig, AnthropicModelOptions, Seams, Transport,
    };
    use crate::models::model::{Model, ModelRequest};
    use crate::types::messages::{Message, Role, TextBlock};

    fn model(env: Vec<(&'static str, String)>, options: AnthropicModelOptions) -> AnthropicModel {
        let (_, sleeper) = recording_sleeper();
        let seams = Seams {
            env: Arc::new(move |name| {
                env.iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.clone())
            }),
            sleeper,
            random: Arc::new(|| 0.0),
            warn_once: Arc::new(|_| {}),
            transport: Transport::Http,
        };
        AnthropicModel::with_seams(options, seams).unwrap()
    }

    fn env_for(server: &Server) -> Vec<(&'static str, String)> {
        vec![
            ("ANTHROPIC_API_KEY", "env-key".to_owned()),
            ("ANTHROPIC_BASE_URL", server.url.clone()),
        ]
    }

    fn hi() -> ModelRequest {
        ModelRequest {
            messages: vec![Message::new(Role::User, vec![TextBlock::new("Hi").into()])],
            ..Default::default()
        }
    }

    fn m1() -> AnthropicModelOptions {
        AnthropicModelConfig {
            model_id: Some("m1".to_owned()),
            max_tokens: Some(10),
            ..Default::default()
        }
        .into()
    }

    async fn run(model: &AnthropicModel) -> (Vec<JsonValue>, Option<Error>) {
        let mut stream = model.stream(hi());
        let mut events = Vec::new();
        while let Some(item) = stream.next().await {
            match item {
                Ok(event) => events.push(serde_json::to_value(event).unwrap()),
                Err(error) => return (events, Some(error)),
            }
        }
        (events, None)
    }

    fn text_reply() -> Reply {
        sse(&[
            (
                "message_start",
                json!({ "type": "message_start", "message": { "role": "assistant", "usage": { "input_tokens": 3 } } }),
            ),
            (
                "content_block_start",
                json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } }),
            ),
            (
                "content_block_delta",
                json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "Hello" } }),
            ),
            (
                "content_block_stop",
                json!({ "type": "content_block_stop", "index": 0 }),
            ),
            (
                "message_delta",
                json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 2 } }),
            ),
            ("message_stop", json!({ "type": "message_stop" })),
        ])
    }

    #[tokio::test]
    async fn streams_from_anthropic_base_url_with_the_key_from_anthropic_api_key() {
        let server = serve(vec![Some(text_reply())]).await;
        let model = model(env_for(&server), m1());
        let (events, error) = run(&model).await;
        assert!(error.is_none(), "{error:?}");
        assert_eq!(
            events.last().unwrap(),
            &json!({ "type": "modelMessageStopEvent", "stopReason": "endTurn" })
        );
        let sent = &server.received()[0];
        assert_eq!(sent.header("x-api-key"), Some("env-key"));
        assert_eq!(
            serde_json::from_str::<JsonValue>(&sent.body).unwrap(),
            json!({
                "model": "m1",
                "max_tokens": 10,
                "messages": [{ "role": "user", "content": [{ "type": "text", "text": "Hi" }] }],
                "stream": true,
            })
        );
    }

    #[tokio::test]
    async fn prefers_the_configured_base_url_and_api_key() {
        let server = serve(vec![Some(text_reply())]).await;
        let options = AnthropicModelOptions {
            api_key: Some("option-key".to_owned()),
            client_config: AnthropicClientConfig {
                base_url: Some(server.url.clone()),
                ..Default::default()
            },
            ..m1()
        };
        let env = vec![("ANTHROPIC_BASE_URL", "http://127.0.0.1:9".to_owned())];
        assert!(run(&model(env, options)).await.1.is_none());
        assert_eq!(server.received()[0].header("x-api-key"), Some("option-key"));
    }

    #[tokio::test]
    async fn sends_anthropic_custom_headers() {
        let server = serve(vec![Some(text_reply())]).await;
        let mut env = env_for(&server);
        env.push(("ANTHROPIC_CUSTOM_HEADERS", "x-team: core".to_owned()));
        assert!(run(&model(env, m1())).await.1.is_none());
        assert_eq!(server.received()[0].header("x-team"), Some("core"));
    }

    #[tokio::test]
    async fn maps_a_429_after_the_transport_retries_to_model_throttled_error() {
        let server = serve(vec![Some(reply(429, &rate_limit_body()))]).await;
        let error = run(&model(env_for(&server), m1())).await.1.unwrap();
        assert!(matches!(error, Error::ModelThrottled(_)), "{error:?}");
        assert_eq!(error.to_string(), format!("429 {}", rate_limit_body()));
        assert_eq!(server.received().len(), 3);
    }

    #[tokio::test]
    async fn keeps_an_overloaded_response_a_model_error() {
        let server = serve(vec![Some(reply(529, "overloaded"))]).await;
        let error = run(&model(env_for(&server), m1())).await.1.unwrap();
        assert!(matches!(error, Error::Model(_)), "{error:?}");
        assert_eq!(error.to_string(), "529 overloaded");
    }

    #[tokio::test]
    async fn maps_a_context_overflow_response_to_context_window_overflow_error() {
        let body = json!({ "type": "error", "error": { "type": "invalid_request_error", "message": "prompt is too long: 300000 tokens > 200000 maximum" } });
        let server = serve(vec![Some(reply(400, &body.to_string()))]).await;
        let error = run(&model(env_for(&server), m1())).await.1.unwrap();
        assert!(matches!(error, Error::ContextWindowOverflow(_)), "{error:?}");
    }

    #[tokio::test]
    async fn maps_a_connection_failure_to_model_network_error() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let env = vec![
            ("ANTHROPIC_API_KEY", "env-key".to_owned()),
            ("ANTHROPIC_BASE_URL", url),
        ];
        let error = run(&model(env, m1())).await.1.unwrap();
        assert!(matches!(error, Error::ModelNetwork(_)), "{error:?}");
        assert_eq!(error.to_string(), "Connection error.");
    }

    #[test]
    fn redacts_the_api_key_in_debug() {
        let options = AnthropicModelOptions {
            api_key: Some("secret-value".to_owned()),
            ..m1()
        };
        assert!(!format!("{options:?}").contains("secret-value"));
        let model = model(Vec::new(), options);
        assert!(!format!("{model:?}").contains("secret-value"));
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("secret-value"));
        let config = AnthropicClientConfig {
            default_headers: headers,
            ..Default::default()
        };
        assert!(!format!("{config:?}").contains("secret-value"));
    }
}
