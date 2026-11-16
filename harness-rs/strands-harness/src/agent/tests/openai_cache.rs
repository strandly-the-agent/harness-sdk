//! Ports `harness-ts/test/openai-cache.test.ts`. TS replaces `Responses.prototype.create`; these
//! cases point `OPENAI_BASE_URL` (read through the env seam) at a localhost server that records
//! each request body and answers with native Responses events.
#![cfg(feature = "openai")]

use std::sync::Mutex;

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::*;

/// What the model answers a request with.
#[derive(Clone)]
pub(super) enum Reply {
    Text(&'static str),
    ToolCall {
        name: &'static str,
        arguments: JsonValue,
    },
}

impl Reply {
    /// The server-sent events of response `id`, as TS `responseStream` yields them.
    fn events(&self, id: &str) -> String {
        let mut events = vec![json!({ "type": "response.created", "response": { "id": id } })];
        match self {
            Self::Text(text) => events.push(json!({ "type": "response.output_text.delta", "delta": text })),
            Self::ToolCall { name, arguments } => {
                events.push(json!({
                    "type": "response.output_item.added",
                    "item": { "type": "function_call", "id": "item_call", "call_id": "call_1", "name": name },
                }));
                events.push(json!({
                    "type": "response.function_call_arguments.done",
                    "item_id": "item_call",
                    "arguments": arguments.to_string(),
                }));
            }
        }
        events.push(json!({ "type": "response.completed", "response": { "id": id } }));
        events
            .iter()
            .map(|event| format!("event: {}\ndata: {event}\n\n", event["type"].as_str().unwrap()))
            .collect()
    }
}

/// A localhost stand-in for the OpenAI Responses endpoint.
pub(super) struct ResponsesServer {
    pub(super) url: String,
    requests: Arc<Mutex<Vec<JsonValue>>>,
}

impl ResponsesServer {
    /// Answers request `n` with `replies[n]`; a request past the script gets a 500.
    pub(super) async fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&requests);
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                tokio::spawn(answer(socket, replies.clone(), Arc::clone(&log)));
            }
        });
        Self { url, requests }
    }

    /// The request bodies received so far.
    pub(super) fn requests(&self) -> Vec<JsonValue> {
        self.requests.lock().unwrap().clone()
    }

    /// An environment with a dummy OpenAI key and this server as the endpoint, plus `vars`.
    pub(super) fn env(&self, vars: &'static [(&'static str, &'static str)]) -> EnvLookup {
        let url = self.url.clone();
        Arc::new(move |name| match name {
            "OPENAI_API_KEY" => Some("offline-test-key".to_owned()),
            "OPENAI_BASE_URL" => Some(url.clone()),
            _ => vars
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned()),
        })
    }
}

async fn answer(mut socket: TcpStream, replies: Vec<Reply>, log: Arc<Mutex<Vec<JsonValue>>>) {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];
    let body_start = loop {
        let read = socket.read(&mut chunk).await.unwrap_or(0);
        if read == 0 {
            return;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let head = String::from_utf8_lossy(&buffer[..body_start]).to_lowercase();
    let length: usize = head
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0);
    while buffer.len() < body_start + length {
        let read = socket.read(&mut chunk).await.unwrap_or(0);
        if read == 0 {
            return;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let index = {
        let mut log = log.lock().unwrap();
        log.push(serde_json::from_slice(&buffer[body_start..]).unwrap_or(JsonValue::Null));
        log.len()
    };
    let response = match replies.get(index - 1) {
        Some(reply) => {
            let body = reply.events(&format!("response_{index}"));
            format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
        }
        None => "HTTP/1.1 500 Unexpected extra OpenAI request\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
            .to_owned(),
    };
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;
}

/// The harness options both TS cases start from: nothing but the model and the session.
fn bare_options(model: &str) -> HarnessAgentOptions {
    let mut options = HarnessAgentOptions {
        model: Some(ModelSource::Id(model.to_owned())),
        builtin_tools: names(&[]),
        builtin_plugins: Some(Vec::new()),
        memory: MemoryOption::Off,
        skills: SkillsOption::Off,
        session: SessionOption::Off,
        context_manager: Some(ContextManagerStrategy::Off),
        background_tasks: Some(false.into()),
        interventions: Some("off".into()),
        ..Default::default()
    };
    options.agent_config.printer = Some(false);
    options
}

/// The `openai/` rows of the TS table. The `bedrock-mantle/` rows are not run: that provider
/// derives its endpoint from the region and mints its token from the AWS credential chain, so no
/// seam points it at a localhost server.
#[tokio::test]
async fn sends_the_session_cache_key_for_when_caching_is() {
    for (model, cache_points) in [("openai/gpt-6-sol", true), ("openai/gpt-4.1", false)] {
        for caching in [true, false] {
            let server = ResponsesServer::start(vec![Reply::Text("Done")]).await;
            let dir = tempfile::tempdir().unwrap();
            let mut options = bare_options(model);
            options.caching = caching.into();
            options.session = SessionOption::Config(SessionConfig {
                id: Some("cache-key-test".to_owned()),
                dir: Some(dir.path().to_string_lossy().into_owned()),
            });
            let seams = Seams {
                base_dir: Some(dir.path().to_owned()),
                env: server.env(&[]),
            };
            let mut agent = create_harness_with(options, seams).await.unwrap();
            agent.invoke("Hello").await.unwrap();

            let requests = server.requests();
            assert_eq!(requests.len(), 1, "{model} caching={caching}");
            let request = requests[0].as_object().unwrap();
            let input = request["input"].to_string();
            if caching {
                assert_eq!(request["prompt_cache_key"], json!("strands-cache-key-test"));
                assert_eq!(request.contains_key("prompt_cache_options"), cache_points, "{model}");
                assert_eq!(input.contains("prompt_cache_breakpoint"), cache_points, "{model}");
            } else {
                assert!(!request.contains_key("prompt_cache_key"), "{model}");
                assert!(!request.contains_key("prompt_cache_options"), "{model}");
                assert!(!input.contains("prompt_cache_breakpoint"), "{model}");
            }
        }
    }
}
