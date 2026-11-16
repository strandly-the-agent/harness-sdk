//! Ports `harness-ts/test/openai-cache.test.ts`. TS replaces `Responses.prototype.create`; these
//! cases point `OPENAI_BASE_URL` (read through the env seam) at a localhost server that records
//! each request body and answers with native Responses events.
#![cfg(feature = "openai")]

use std::path::PathBuf;
use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::json;
use strands::memory::{MemoryEntry, MemoryStore, SearchOptions};
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
        None => {
            "HTTP/1.1 500 Unexpected extra OpenAI request\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                .to_owned()
        }
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

const MARKER: &str = "prompt_cache_breakpoint";
const PROJECT_DOC: &str = "Offline cache regression project instructions.";
const MEMORY_ENTRY: &str = "Offline memory: releases ship on Thursdays.";

/// The TS `memoryStore`: read-only, one entry for every search.
struct OfflineMemory;

#[async_trait]
impl MemoryStore for OfflineMemory {
    fn name(&self) -> &str {
        "memory"
    }

    fn writable(&self) -> bool {
        false
    }

    async fn search(&self, _: &str, _: SearchOptions) -> strands::Result<Vec<MemoryEntry>> {
        Ok(vec![MemoryEntry::new(MEMORY_ENTRY)])
    }
}

/// The local environment with `root` as its working directory, where TS calls `process.chdir`.
struct RootedSandbox {
    local: NotASandboxLocalEnvironment,
    root: PathBuf,
}

impl RootedSandbox {
    fn path(&self, path: &str) -> String {
        self.root.join(path).to_string_lossy().into_owned()
    }
}

#[async_trait]
impl Sandbox for RootedSandbox {
    fn execute_streaming<'a>(
        &'a self,
        command: &'a str,
        mut options: ExecuteOptions,
    ) -> BoxStream<'a, strands::Result<ExecutionEvent>> {
        options.cwd.get_or_insert_with(|| self.path(""));
        self.local.execute_streaming(command, options)
    }

    fn execute_code_streaming<'a>(
        &'a self,
        code: &'a str,
        language: &'a str,
        mut options: ExecuteOptions,
    ) -> BoxStream<'a, strands::Result<ExecutionEvent>> {
        options.cwd.get_or_insert_with(|| self.path(""));
        self.local.execute_code_streaming(code, language, options)
    }

    async fn read_file(&self, path: &str) -> strands::Result<Vec<u8>> {
        self.local.read_file(&self.path(path)).await
    }

    async fn write_file(&self, path: &str, content: &[u8]) -> strands::Result<()> {
        self.local.write_file(&self.path(path), content).await
    }

    async fn remove_file(&self, path: &str) -> strands::Result<()> {
        self.local.remove_file(&self.path(path)).await
    }

    async fn list_files(&self, path: &str) -> strands::Result<Vec<FileInfo>> {
        self.local.list_files(&self.path(path)).await
    }
}

fn todos(content: &str) -> JsonValue {
    json!([{ "content": content, "activeForm": content, "status": "in_progress" }])
}

fn cached_text(text: &str) -> JsonValue {
    json!({ "type": "input_text", "text": text, MARKER: { "mode": "explicit" } })
}

/// Asserts `item` is the user message `text` behind a cache checkpoint, followed by one
/// unmarked text part per entry of `temporary`, each containing that entry.
fn assert_user_input(item: &JsonValue, text: Option<&str>, temporary: &[&str]) {
    assert_eq!(item["role"], json!("user"), "{item}");
    let mut parts = item["content"].as_array().unwrap().iter();
    if let Some(text) = text {
        assert_eq!(parts.next(), Some(&cached_text(text)), "{item}");
    }
    assert_eq!(parts.len(), temporary.len(), "{item}");
    for (part, expected) in parts.zip(temporary) {
        let part = part.as_object().unwrap();
        assert_eq!(part.len(), 2, "{item}");
        assert_eq!(part["type"], json!("input_text"));
        assert!(
            part["text"].as_str().unwrap().contains(expected),
            "{expected} not in {item}"
        );
    }
}

/// The conversation without injected blocks, as role and content.
fn durable_history_of(agent: &Agent) -> Vec<JsonValue> {
    agent
        .messages()
        .iter()
        .map(|message| {
            let message = serde_json::to_value(message).unwrap();
            let content: Vec<&JsonValue> = message["content"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|block| block.get("injected").is_none())
                .collect();
            json!({ "role": message["role"], "content": content })
        })
        .collect()
}

/// The `openai/gpt-6-sol` row of the TS table; the `bedrock-mantle/` row is not run (see
/// `sends_the_session_cache_key_for_when_caching_is`). TS moves a fake clock one day per turn
/// and looks for the stale date; the environment plugin reads the real clock, so this case counts
/// the `<environment>` blocks of each request instead.
#[tokio::test]
async fn keeps_durable_checkpoints_ahead_of_changing_todos_environment_and_tool_continuation_injections_on() {
    let (first, second, tool, third) = (
        "Inspect initial cache prefix",
        "Inspect updated cache prefix",
        "Verify tool continuation",
        "Review final cache prefix",
    );
    let server = ResponsesServer::start(vec![
        Reply::Text("First answer"),
        Reply::ToolCall {
            name: "todo_write",
            arguments: json!({ "todos": todos(tool) }),
        },
        Reply::Text("Second answer"),
        Reply::Text("Third answer"),
    ])
    .await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), PROJECT_DOC).unwrap();

    let mut options = bare_options("openai/gpt-6-sol");
    options.builtin_plugins = Some(BUILTIN_PLUGIN_NAMES.to_vec());
    options.memory = MemoryConfig {
        dir: None,
        stores: Some(vec![Arc::new(OfflineMemory)]),
    }
    .into();
    options.agent_config.system_prompt = Some("Keep this system prompt stable.".into());
    options.agent_config.sandbox = Some(SandboxSource::new(RootedSandbox {
        local: NotASandboxLocalEnvironment::new(),
        root: dir.path().to_owned(),
    }));
    let seams = Seams {
        base_dir: Some(dir.path().to_owned()),
        env: server.env(&[]),
    };
    // No `caching` option: the default opts a model that takes cache points in.
    let mut agent = create_harness_with(options, seams).await.unwrap();
    assert_eq!(agent.model().model_id().as_deref(), Some("gpt-6-sol"));

    let environments = |request: &JsonValue| request["input"].to_string().matches("<environment>").count();
    let assistant = |text: &str| json!({ "role": "assistant", "content": text });

    agent.app_state().set("todos", todos(first)).unwrap();
    agent.invoke("First question").await.unwrap();
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let input = requests[0]["input"].as_array().unwrap();
    assert_eq!(input.len(), 1);
    assert_user_input(
        &input[0],
        Some("First question"),
        &[first, "Date: ", MEMORY_ENTRY],
    );
    let text = requests[0]["input"].to_string();
    assert!(text.contains(PROJECT_DOC), "{text}");
    assert!(text.contains("Working directory: "), "{text}");
    let first_history = durable_history_of(&agent);
    assert_eq!(
        first_history,
        [
            json!({ "role": "user", "content": [{ "text": "First question" }] }),
            json!({ "role": "assistant", "content": [{ "text": "First answer" }] }),
        ]
    );

    agent.app_state().set("todos", todos(second)).unwrap();
    agent.invoke("Second question").await.unwrap();
    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    let input = requests[1]["input"].as_array().unwrap();
    assert_eq!(input.len(), 3);
    assert_user_input(&input[0], Some("First question"), &[]);
    assert_eq!(input[1], assistant("First answer"));
    assert_user_input(
        &input[2],
        Some("Second question"),
        &[second, "Date: ", MEMORY_ENTRY],
    );
    let tool_call = json!({
        "type": "function_call",
        "call_id": "call_1",
        "name": "todo_write",
        "arguments": json!({ "todos": todos(tool) }).to_string(),
    });
    let tool_output = json!({
        "type": "function_call_output",
        "call_id": "call_1",
        "output": [cached_text(&format!("1 todos remaining\n  [in_progress] {tool}"))],
    });
    let input = requests[2]["input"].as_array().unwrap();
    assert_eq!(input.len(), 6);
    assert_user_input(&input[0], Some("First question"), &[]);
    assert_eq!(input[1], assistant("First answer"));
    assert_user_input(&input[2], Some("Second question"), &[]);
    assert_eq!(input[3], tool_call);
    assert_eq!(input[4], tool_output);
    assert_user_input(&input[5], None, &[tool]);
    assert_eq!(
        agent.app_state().get::<JsonValue>("todos").unwrap(),
        Some(todos(tool))
    );
    // Environment is user-turn only; the every-turn todo injector has refreshed after todo_write.
    assert_eq!(environments(&requests[1]), 1);
    assert_eq!(environments(&requests[2]), 0);
    for request in &requests[1..] {
        assert!(!request["input"].to_string().contains(first));
    }
    assert!(!requests[2]["input"].to_string().contains(second));

    agent.app_state().set("todos", todos(third)).unwrap();
    agent.invoke("Third question").await.unwrap();
    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    let input = requests[3]["input"].as_array().unwrap();
    assert_eq!(input.len(), 7);
    assert_user_input(&input[0], Some("First question"), &[]);
    assert_eq!(input[1], assistant("First answer"));
    assert_user_input(&input[2], Some("Second question"), &[]);
    assert_eq!(input[3], tool_call);
    assert_eq!(input[4], tool_output);
    assert_eq!(input[5], assistant("Second answer"));
    assert_user_input(
        &input[6],
        Some("Third question"),
        &[third, "Date: ", MEMORY_ENTRY],
    );
    let latest = requests[3]["input"].to_string();
    assert!(!latest.contains(first));
    assert!(!latest.contains(second));
    assert_eq!(environments(&requests[3]), 1);

    let injected_roles: Vec<JsonValue> = agent
        .messages()
        .iter()
        .map(|message| serde_json::to_value(message).unwrap())
        .filter(|message| {
            message["content"]
                .as_array()
                .unwrap()
                .iter()
                .any(|block| block.get("injected").is_some())
        })
        .map(|message| message["role"].clone())
        .collect();
    assert!(!injected_roles.is_empty());
    assert!(
        injected_roles.iter().all(|role| role == "user"),
        "{injected_roles:?}"
    );
    let history = durable_history_of(&agent);
    assert_eq!(history[..first_history.len()], first_history);
    assert_eq!(history.len(), 8);
    let durable = JsonValue::Array(history).to_string();
    for temporary in [
        "<system-reminder>",
        "<environment>",
        "<memory>",
        MEMORY_ENTRY,
        PROJECT_DOC,
        MARKER,
        // The tool's arguments and result are durable; externally seeded todo reminders are not.
        first,
        second,
        third,
    ] {
        assert!(!durable.contains(temporary), "{temporary} in {durable}");
    }
    for request in &requests {
        assert_eq!(request["model"], json!("gpt-6-sol"));
        assert_eq!(request["stream"], json!(true));
        assert_eq!(request["store"], json!(false));
        assert_eq!(request["instructions"], json!("Keep this system prompt stable."));
        assert_eq!(request["prompt_cache_options"], json!({ "mode": "explicit" }));
        assert!(request.get("previous_response_id").is_none());
    }
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
                assert_eq!(
                    request.contains_key("prompt_cache_options"),
                    cache_points,
                    "{model}"
                );
                assert_eq!(input.contains("prompt_cache_breakpoint"), cache_points, "{model}");
            } else {
                assert!(!request.contains_key("prompt_cache_key"), "{model}");
                assert!(!request.contains_key("prompt_cache_options"), "{model}");
                assert!(!input.contains("prompt_cache_breakpoint"), "{model}");
            }
        }
    }
}
