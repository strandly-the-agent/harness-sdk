//! The worker runtime: one agent, one turn at a time, driven by [`Command`]s.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use futures::future::BoxFuture;
use futures::StreamExt;
use parking_lot::Mutex;
use serde_json::{json, Map, Value as JsonValue};
use strands_agents::agent::{AgentStreamEvent, SnapshotField, SnapshotPreset, TakeSnapshotOptions};
use strands_agents::error::{Error, Result};
use strands_agents::hooks::events::{BeforeToolCallEvent, Cancel};
use strands_agents::hooks::hook_order;
use strands_agents::models::streaming::{ContentBlockDelta, ModelStreamEvent};
use strands_agents::plugins::vended::skills::AgentSkills;
use strands_agents::session::Snapshot;
use strands_agents::tools::ToolContext;
use strands_agents::types::{ContentBlock, InterruptResponseContent, Message, StopReason, ToolUse};
use strands_agents::{Agent, InvocationState, InvokeInput};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::protocol::{
    bytes_value, decode_bytes, ConfiguredSession, ContextUsage, RunResult, SessionIdentity, SkillInfo, State,
    TokenUsage, ToolInfo, WorkerMessage,
};

/// Default session directory, relative to the workspace.
pub const DEFAULT_SESSION_DIR: &str = ".agent/sessions";
/// Default memory directory, relative to the workspace.
pub const DEFAULT_MEMORY_DIR: &str = ".agent/memory";

const DENIED: &str = "DENIED: Tool call denied by the user";

/// Everything the factory needs to build (or rebuild) the agent.
#[derive(Debug, Clone, Default)]
pub struct BuildOptions {
    /// Profile overrides with `assignments` already merged in.
    pub overrides: Map<String, JsonValue>,
    /// Explicit model specifier (`overrides.model`, or a later `reset`).
    pub model: Option<String>,
    /// Effort as the CLI spells it: a string, a bool, or `null` for off.
    pub effort: JsonValue,
    /// Session id to use when sessions are on.
    pub session_id: Option<String>,
    /// Session directory to use when sessions are on.
    pub session_dir: Option<String>,
    /// Extra MCP servers.
    pub mcp_servers: Option<Map<String, JsonValue>>,
    /// Extra skill sources.
    pub skill_paths: Vec<String>,
}

impl BuildOptions {
    /// Whether session persistence is enabled by the overrides.
    pub fn sessions_enabled(&self) -> bool {
        !matches!(self.overrides.get("session"), Some(JsonValue::Bool(false)))
    }
}

/// What a factory hands back.
pub struct BuiltAgent {
    /// The agent.
    pub agent: Agent,
    /// The skills plugin the agent carries, if any.
    pub skills: Option<Arc<AgentSkills>>,
    /// Where sessions are written; `None` when persistence is off.
    pub session_dir: Option<PathBuf>,
}

/// Builds an agent from [`BuildOptions`]; the binary uses `strands-harness`, tests a mock model.
pub type AgentFactory = Arc<dyn Fn(BuildOptions) -> BoxFuture<'static, Result<BuiltAgent>> + Send + Sync>;

/// Outgoing messages, serialized by the writer task.
pub type Outbox = mpsc::UnboundedSender<WorkerMessage>;

struct Shared {
    outbox: Outbox,
    /// Hook-originated messages; the turn loop forwards them after the stream events that
    /// preceded them, so the CLI never sees `authorize` before the matching `toolStart`.
    requests: mpsc::UnboundedSender<WorkerMessage>,
    pending: Mutex<HashMap<String, oneshot::Sender<bool>>>,
    workspace: PathBuf,
}

impl Shared {
    fn send(&self, message: WorkerMessage) {
        // A closed outbox means the CLI is gone; nothing left to tell.
        drop(self.outbox.send(message));
    }

    fn event(&self, event: JsonValue) {
        self.send(WorkerMessage::Event { event });
    }

    fn register(&self, id: &str) -> oneshot::Receiver<bool> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id.to_owned(), tx);
        rx
    }

    /// Wait for the CLI's yes/no on `id`; a dropped sender (cancel) reads as "no".
    async fn wait(&self, id: &str, rx: oneshot::Receiver<bool>) -> bool {
        let approved = rx.await.unwrap_or(false);
        self.pending.lock().remove(id);
        approved
    }
}

/// The worker's agent and the options it was built from.
pub struct Runtime {
    factory: AgentFactory,
    shared: Arc<Shared>,
    interactive: bool,
    configured_session: ConfiguredSession,
    options: Mutex<BuildOptions>,
    built: Mutex<Arc<BuiltAgent>>,
    cancel: Mutex<CancellationToken>,
    requests: tokio::sync::Mutex<mpsc::UnboundedReceiver<WorkerMessage>>,
    reload_requested: AtomicBool,
}

impl Runtime {
    /// Build the first agent.
    pub async fn open(
        factory: AgentFactory,
        outbox: Outbox,
        options: BuildOptions,
        interactive: bool,
        workspace: PathBuf,
    ) -> Result<Arc<Self>> {
        let (requests, requests_rx) = mpsc::unbounded_channel();
        let shared = Arc::new(Shared {
            outbox,
            requests,
            pending: Mutex::new(HashMap::new()),
            workspace,
        });
        let configured_session = ConfiguredSession {
            session_id: options.session_id.clone(),
            session_dir: options
                .session_dir
                .clone()
                .unwrap_or_else(|| DEFAULT_SESSION_DIR.to_owned()),
        };
        let built = (factory)(options.clone()).await?;
        let runtime = Arc::new(Self {
            factory,
            shared,
            interactive,
            configured_session,
            options: Mutex::new(options),
            built: Mutex::new(Arc::new(built)),
            cancel: Mutex::new(CancellationToken::new()),
            requests: tokio::sync::Mutex::new(requests_rx),
            reload_requested: AtomicBool::new(false),
        });
        runtime.install_hooks();
        Ok(runtime)
    }

    fn agent(&self) -> Agent {
        self.built.lock().agent.clone()
    }

    fn built(&self) -> Arc<BuiltAgent> {
        self.built.lock().clone()
    }

    fn install_hooks(&self) {
        if !self.interactive {
            return;
        }
        let shared = self.shared.clone();
        self.agent().hooks().add_callback_boxed_with_order(
            hook_order::INTERVENTION_INPUT,
            move |event: &mut BeforeToolCallEvent| -> BoxFuture<'_, Result<()>> {
                let shared = shared.clone();
                Box::pin(async move {
                    if event.tool_use.name == "strands_config" {
                        return Ok(());
                    }
                    let id = uuid::Uuid::new_v4().to_string();
                    let input = event.tool_use.input.clone();
                    let name = event.tool_use.name.clone();
                    let path = input.get("path").and_then(JsonValue::as_str).map(PathBuf::from);
                    let message = WorkerMessage::Authorize {
                        id: id.clone(),
                        path_in_workspace: path
                            .as_deref()
                            .is_some_and(|p| path_in_workspace(&shared.workspace, p)),
                        before: path
                            .as_deref()
                            .and_then(|p| before_text(&shared.workspace, &name, p)),
                        name,
                        input,
                    };
                    let rx = shared.register(&id);
                    drop(shared.requests.send(message));
                    if !shared.wait(&id, rx).await {
                        event.cancel = Cancel::Reason(DENIED.to_owned());
                    }
                    Ok(())
                })
            },
        );
    }

    /// Resolve a pending `authorize` / `permission` request; unknown ids are ignored.
    pub fn permission(&self, request_id: &str, approved: bool) {
        if let Some(tx) = self.shared.pending.lock().remove(request_id) {
            let _ = tx.send(approved);
        }
    }

    /// Stop the running turn (if any) and deny everything it is waiting on.
    pub fn cancel(&self) {
        self.shared.pending.lock().clear();
        self.cancel.lock().cancel();
        self.agent().cancel();
    }

    /// Whether the agent asked for a configuration reload during the last turn.
    pub fn take_reload_requested(&self) -> bool {
        self.reload_requested.swap(false, Ordering::SeqCst)
    }

    /// Run one turn; the reply is a `result` or an `error`.
    pub async fn turn(&self, prompt: JsonValue) -> WorkerMessage {
        let token = CancellationToken::new();
        *self.cancel.lock() = token.clone();
        self.reload_requested.store(false, Ordering::SeqCst);
        let mut input = match parse_prompt(prompt) {
            Ok(input) => input,
            Err(error) => return self.error(error),
        };
        loop {
            match self.run_once(input, &token).await {
                Ok(Turn::Done(result)) => return self.result(result),
                Ok(Turn::Interrupted(responses)) => {
                    if responses.is_empty() {
                        return self.result(RunResult::stop("cancelled"));
                    }
                    input = InvokeInput::InterruptResponses(responses);
                }
                Err(_) if token.is_cancelled() => return self.result(RunResult::stop("cancelled")),
                Err(error) => return self.error(error),
            }
        }
    }

    async fn run_once(&self, input: InvokeInput, token: &CancellationToken) -> Result<Turn> {
        let agent = self.agent();
        let mut stream = agent.stream(input);
        let mut requests = self.requests.lock().await;
        let mut seen_results = std::collections::HashSet::new();
        loop {
            let event = tokio::select! {
                biased;
                event = stream.next() => match event {
                    Some(event) => event?,
                    None => break,
                },
                Some(request) = requests.recv() => {
                    self.shared.send(request);
                    continue;
                }
            };
            match event {
                AgentStreamEvent::ModelStreamUpdate(update) => match update.event {
                    ModelStreamEvent::ContentBlockDelta {
                        delta: ContentBlockDelta::Text { text },
                    } => self.shared.event(json!({ "type": "textDelta", "text": text })),
                    ModelStreamEvent::ContentBlockDelta {
                        delta: ContentBlockDelta::ReasoningContent { text: Some(text), .. },
                    } if !text.is_empty() => self
                        .shared
                        .event(json!({ "type": "reasoningDelta", "text": text })),
                    _ => {}
                },
                AgentStreamEvent::ContentBlock(block) => match &block.content_block {
                    ContentBlock::ToolUse(tool_use) => self.shared.event(json!({
                        "type": "toolStart",
                        "toolUseId": tool_use.tool_use_id,
                        "name": tool_use.name,
                        "input": tool_use.input,
                    })),
                    ContentBlock::Image(_) | ContentBlock::Document(_) => {
                        let value = serde_json::to_value(&block.content_block)?;
                        self.shared
                            .event(json!({ "type": "media", "content": display_content(value) }));
                    }
                    _ => {}
                },
                AgentStreamEvent::ToolResult(event) => {
                    if seen_results.insert(event.result.tool_use_id.clone()) {
                        let value = serde_json::to_value(&event.result)?;
                        self.shared.event(json!({
                            "type": "toolResult",
                            "toolUseId": event.result.tool_use_id,
                            "status": value.get("status").cloned().unwrap_or(JsonValue::Null),
                            "content": display_contents(value.get("content")),
                        }));
                    }
                }
                AgentStreamEvent::AgentResult(event) => {
                    let result = event.result;
                    if token.is_cancelled() || result.stop_reason == StopReason::Cancelled {
                        return Ok(Turn::Done(RunResult::stop("cancelled")));
                    }
                    if result.stop_reason == StopReason::Interrupt {
                        let mut responses = Vec::new();
                        for interrupt in &result.interrupts {
                            if token.is_cancelled() {
                                return Ok(Turn::Interrupted(Vec::new()));
                            }
                            let rx = self.shared.register(&interrupt.id);
                            self.shared.event(permission_event(
                                &interrupt.id,
                                &interrupt.name,
                                interrupt.reason.clone(),
                            ));
                            let approved = self.shared.wait(&interrupt.id, rx).await;
                            if token.is_cancelled() {
                                return Ok(Turn::Interrupted(Vec::new()));
                            }
                            responses.push(InterruptResponseContent::new(interrupt.id.clone(), approved));
                        }
                        return Ok(Turn::Interrupted(responses));
                    }
                    let metrics = &result.metrics;
                    let usage = &metrics.accumulated_usage;
                    return Ok(Turn::Done(RunResult {
                        stop_reason: result.stop_reason.as_str().to_owned(),
                        final_text: Some(result.text()),
                        usage: Some(TokenUsage {
                            input_tokens: usage.input_tokens,
                            output_tokens: usage.output_tokens,
                            total_tokens: usage.total_tokens,
                            cache_read_input_tokens: usage.cache_read_input_tokens.unwrap_or(0),
                            cache_write_input_tokens: usage.cache_write_input_tokens.unwrap_or(0),
                        }),
                        context: Some(ContextUsage {
                            current_tokens: metrics.latest_context_size,
                            projected_tokens: metrics.projected_context_size,
                            context_window: agent.model().context_window_limit(),
                        }),
                    }));
                }
                _ => {}
            }
        }
        Err(Error::other("The agent stream ended without a result"))
    }

    /// Rebuild the agent, carrying the conversation over unless `clear`.
    pub async fn reset(
        &self,
        model: Option<String>,
        thinking: Option<JsonValue>,
        session_id: Option<String>,
        clear: bool,
    ) -> Result<()> {
        let snapshot = (!clear).then(|| {
            self.agent()
                .take_snapshot(TakeSnapshotOptions::default().preset(SnapshotPreset::Session))
        });
        let options = {
            let mut options = self.options.lock();
            if let Some(model) = model {
                options
                    .overrides
                    .insert("model".to_owned(), JsonValue::String(model.clone()));
                options.model = Some(model);
            }
            if let Some(thinking) = thinking {
                options.overrides.insert("effort".to_owned(), thinking.clone());
                options.effort = thinking;
            }
            if let Some(session_id) = session_id {
                if options.sessions_enabled() {
                    options.session_id = Some(session_id);
                }
            }
            options.clone()
        };
        let built = (self.factory)(options).await?;
        if let Some(snapshot) = snapshot {
            built.agent.load_snapshot(&snapshot)?;
        }
        *self.built.lock() = Arc::new(built);
        self.install_hooks();
        Ok(())
    }

    /// Activate a skill by name.
    pub async fn activate_skill(&self, name: &str) -> Result<()> {
        let built = self.built();
        let Some(skills) = &built.skills else {
            return Err(Error::other("This agent has no skills"));
        };
        if !skills.available_skills().iter().any(|skill| skill.name == name) {
            return Err(Error::other(format!("Unknown skill '{name}'")));
        }
        let agent = built.agent.clone();
        let Some(tool) = agent.tools().into_iter().find(|tool| tool.name() == "skills") else {
            return Err(Error::other("This agent has no skills tool"));
        };
        let tool_use = ToolUse {
            name: "skills".to_owned(),
            tool_use_id: uuid::Uuid::new_v4().to_string(),
            input: json!({ "skill_name": name }),
        };
        let ctx = ToolContext::new(tool_use, agent, InvocationState::new(), CancellationToken::new());
        tool.invoke(ctx).await?;
        Ok(())
    }

    /// Serialized session snapshot (fork source).
    pub fn snapshot_json(&self) -> Result<String> {
        let snapshot = self
            .agent()
            .take_snapshot(TakeSnapshotOptions::default().preset(SnapshotPreset::Session));
        Ok(serde_json::to_string(&snapshot)?)
    }

    /// Load a snapshot produced by [`Runtime::snapshot_json`] (fork target).
    pub fn seed(&self, snapshot: &str) -> Result<()> {
        let snapshot: Snapshot = serde_json::from_str(snapshot)?;
        self.agent().load_snapshot(&snapshot)
    }

    /// Serialized conversation snapshot (`messages` + `state`) for the CLI's conversation store.
    pub fn conversation_json(&self) -> Result<String> {
        let snapshot = self.agent().take_snapshot(
            TakeSnapshotOptions::default()
                .preset(SnapshotPreset::Session)
                .exclude(SnapshotField::SystemPrompt)
                .exclude(SnapshotField::ModelState)
                .exclude(SnapshotField::Interrupts),
        );
        Ok(serde_json::to_string(&snapshot)?)
    }

    /// Restore a conversation captured by [`Runtime::conversation_json`] (or by another runtime).
    pub fn restore_conversation(&self, snapshot: JsonValue, preserve_reasoning: bool) -> Result<()> {
        let agent = self.agent();
        let mut data = match decode_bytes(snapshot).get("data").cloned() {
            Some(JsonValue::Object(data)) => data,
            _ => return Err(Error::other("The conversation snapshot has no data")),
        };
        if !preserve_reasoning {
            if let Some(JsonValue::Array(messages)) = data.get_mut("messages") {
                messages.retain_mut(|message| {
                    let Some(JsonValue::Array(content)) = message.get_mut("content") else {
                        return true;
                    };
                    content.retain(|block| block.get("reasoning").is_none());
                    !content.is_empty()
                });
            }
        }
        let mut template =
            agent.take_snapshot(TakeSnapshotOptions::default().preset(SnapshotPreset::Session));
        template.data.retain(|key, _| key != "messages" && key != "state");
        template.data.extend(
            data.into_iter()
                .filter(|(key, _)| key == "messages" || key == "state"),
        );
        template.app_data = Map::new();
        agent.load_snapshot(&template)
    }

    /// Append a user message recording a failed configuration reload.
    pub fn record_reload_failure(&self, message: &str) {
        let text = format!(
            "The configuration change could not be applied and the previous configuration is still active.\nReason: {message}\n"
        );
        self.agent()
            .with_messages(|messages| messages.push(Message::user(text)));
    }

    /// Compact the conversation. Not supported yet: the Rust SDK has no summarizing manager.
    pub fn compact(&self) -> Result<Option<ContextUsage>> {
        if self.agent().messages().len() <= 2 {
            return Ok(None);
        }
        Err(Error::other("/compact is not supported by the Rust runtime yet."))
    }

    fn result(&self, result: RunResult) -> WorkerMessage {
        WorkerMessage::Result {
            result,
            state: self.state(),
            reload: self.take_reload_requested(),
        }
    }

    fn error(&self, error: Error) -> WorkerMessage {
        WorkerMessage::Error {
            message: error.to_string(),
        }
    }

    /// The CLI's view of the agent.
    pub fn state(&self) -> State {
        let built = self.built();
        let agent = &built.agent;
        let options = self.options.lock().clone();
        let workspace = &self.shared.workspace;
        let session_directory = built
            .session_dir
            .clone()
            .unwrap_or_else(|| workspace.join(&self.configured_session.session_dir));
        let session_directory = absolute(workspace, &session_directory);
        let memory_dir = options
            .overrides
            .get("memory")
            .and_then(|memory| memory.get("dir"))
            .and_then(JsonValue::as_str)
            .unwrap_or(DEFAULT_MEMORY_DIR);
        let session = built.session_dir.as_ref().map(|_| SessionIdentity {
            session_id: agent.session_id(),
            session_directory: session_directory.clone(),
            workspace: workspace.display().to_string(),
            configured: self.configured_session.clone(),
        });
        let skills = built
            .skills
            .as_ref()
            .map(|plugin| {
                let active = plugin.activated_skills(agent);
                plugin
                    .available_skills()
                    .into_iter()
                    .map(|skill| SkillInfo {
                        active: active.contains(&skill.name),
                        name: skill.name,
                        description: skill.description,
                        instructions: skill.instructions,
                    })
                    .collect()
            })
            .unwrap_or_default();
        State {
            name: agent.name().to_owned(),
            reconstructable: true,
            description: agent.description().unwrap_or_default().to_owned(),
            model: options
                .model
                .clone()
                .unwrap_or_else(|| strands_harness::defaults::DEFAULT_MODEL.to_owned()),
            model_specifier: options.model.clone(),
            thinking: options.effort.clone(),
            session_id: session.as_ref().map(|s| s.session_id.clone()),
            session_directory: session_directory.clone(),
            session,
            messages: display_messages(&agent.messages()),
            tools: agent
                .tools()
                .iter()
                .map(|tool| ToolInfo {
                    name: tool.name().to_owned(),
                    description: tool.description().to_owned(),
                })
                .collect(),
            skills,
            private_paths: vec![session_directory, absolute(workspace, Path::new(memory_dir))],
        }
    }
}

enum Turn {
    Done(RunResult),
    Interrupted(Vec<InterruptResponseContent>),
}

fn parse_prompt(prompt: JsonValue) -> Result<InvokeInput> {
    match prompt {
        JsonValue::String(text) => Ok(InvokeInput::Text(text)),
        JsonValue::Array(blocks) => {
            let blocks: Vec<ContentBlock> = serde_json::from_value(decode_bytes(JsonValue::Array(blocks)))?;
            Ok(InvokeInput::Blocks(blocks))
        }
        _ => Err(Error::other(
            "prompt must be a string or a list of content blocks",
        )),
    }
}

fn permission_event(id: &str, name: &str, reason: Option<JsonValue>) -> JsonValue {
    json!({
        "type": "permission",
        "request": {
            "id": id,
            "toolName": name,
            "input": reason.unwrap_or(JsonValue::Null),
            "options": [
                { "id": "allow", "label": "Allow", "kind": "allow_once" },
                { "id": "deny", "label": "Deny", "kind": "reject_once" },
            ],
        },
    })
}

fn absolute(workspace: &Path, path: &Path) -> String {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    };
    joined.display().to_string()
}

fn path_in_workspace(workspace: &Path, path: &Path) -> bool {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    };
    let candidate = candidate.canonicalize().unwrap_or(candidate);
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    candidate.starts_with(workspace)
}

fn before_text(workspace: &Path, tool: &str, path: &Path) -> Option<String> {
    if tool != "write" && tool != "edit" {
        return None;
    }
    let full = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    };
    match std::fs::read_to_string(full) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && tool == "write" => Some(String::new()),
        Err(_) => None,
    }
}

/// Externally tagged SDK block → `ChatToolResultContent` / `ChatMediaContent`.
fn display_content(block: JsonValue) -> JsonValue {
    let JsonValue::Object(mut map) = block else {
        return json!({ "type": "text", "text": block.to_string() });
    };
    if let Some(text) = map.remove("text") {
        return json!({ "type": "text", "text": text });
    }
    if let Some(value) = map.remove("json") {
        return json!({ "type": "json", "value": value });
    }
    for kind in ["image", "document"] {
        if let Some(JsonValue::Object(mut media)) = map.remove(kind) {
            let source = media.remove("source").and_then(|source| match source {
                JsonValue::Object(source) => source.into_iter().next(),
                _ => None,
            });
            let mut out = Map::new();
            out.insert("type".to_owned(), JsonValue::String(kind.to_owned()));
            out.extend(media);
            if let Some((key, value)) = source {
                let value = match (key.as_str(), value) {
                    ("bytes", JsonValue::String(encoded)) => bytes_value(&encoded),
                    (_, value) => value,
                };
                out.insert("source".to_owned(), json!({ "type": key, key: value }));
            }
            return JsonValue::Object(out);
        }
    }
    json!({ "type": "text", "text": JsonValue::Object(map).to_string() })
}

fn display_contents(content: Option<&JsonValue>) -> Vec<JsonValue> {
    match content {
        Some(JsonValue::Array(items)) => items.iter().cloned().map(display_content).collect(),
        _ => Vec::new(),
    }
}

/// Messages reduced to text / toolUse / toolResult blocks, media replaced by `[media]`.
fn display_messages(messages: &[Message]) -> Vec<JsonValue> {
    messages
        .iter()
        .filter_map(|message| {
            let content: Vec<JsonValue> = message
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text(_) | ContentBlock::ToolUse(_) => serde_json::to_value(block).ok(),
                    ContentBlock::ToolResult(result) => {
                        let mut value = serde_json::to_value(result).ok()?;
                        if let Some(JsonValue::Array(items)) = value.get_mut("content") {
                            for item in items.iter_mut() {
                                let keep = matches!(item, JsonValue::Object(m) if m.contains_key("text") || m.contains_key("json"));
                                if !keep {
                                    *item = json!({ "text": "[media]" });
                                }
                            }
                        }
                        Some(json!({ "toolResult": value }))
                    }
                    _ => None,
                })
                .collect();
            (!content.is_empty()).then(|| json!({ "role": message.role, "content": content }))
        })
        .collect()
}
