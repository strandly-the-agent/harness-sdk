//! Wire types for the CLI worker protocol (see `docs/CLI_WORKER_PROTOCOL.md`).
//!
//! Commands arrive on stdin, messages leave on fd 3, one JSON object per line. Binary values are
//! wrapped as `{"$bytes": "<base64>"}` in both directions.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as JsonValue};

/// A command from the CLI.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Command {
    /// First line: build the agent.
    Init(InitOptions),
    /// Run one turn.
    Prompt {
        /// A string or a list of content blocks.
        prompt: JsonValue,
    },
    /// Answer to an `authorize` message or a `permission` event.
    #[serde(rename_all = "camelCase")]
    Permission {
        /// The `authorize.id` or interrupt id being answered.
        request_id: String,
        /// Whether the tool call may proceed.
        approved: bool,
    },
    /// Stop the running turn.
    Cancel,
    /// Stop and exit.
    Close,
    /// Rebuild the agent with changed options.
    #[serde(rename_all = "camelCase")]
    Reset {
        /// New model specifier.
        #[serde(default)]
        model: Option<String>,
        /// New effort (`null` means off); absent means keep.
        #[serde(default, deserialize_with = "present")]
        thinking: Option<JsonValue>,
        /// New session id.
        #[serde(default)]
        session_id: Option<String>,
        /// Drop the conversation instead of carrying it over.
        #[serde(default)]
        clear: bool,
    },
    /// Compact the conversation.
    Compact,
    /// Activate a skill by name.
    Skill {
        /// Skill name.
        name: String,
    },
    /// Capture a full snapshot (fork source).
    Snapshot,
    /// Capture the context stash (fork source).
    Stash,
    /// Load a snapshot and stash captured by `snapshot`/`stash` (fork target).
    Seed {
        /// Serialized snapshot.
        snapshot: String,
        /// Serialized stash.
        #[serde(default)]
        stash: Option<String>,
    },
    /// Capture (no `snapshot`) or restore (with `snapshot`) the conversation.
    #[serde(rename_all = "camelCase")]
    Conversation {
        /// camelCase snapshot to restore.
        #[serde(default)]
        snapshot: Option<JsonValue>,
        /// Keep reasoning blocks when restoring.
        #[serde(default)]
        preserve_reasoning: bool,
    },
    /// Record a failed configuration reload in the conversation.
    ReloadFailed {
        /// Why the reload failed.
        message: String,
    },
}

/// Options carried by `init` (the CLI's `PythonOptions`, spread at top level).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitOptions {
    /// Harness profile overrides (`Partial<HarnessAgentConfig>`).
    #[serde(default)]
    pub overrides: Map<String, JsonValue>,
    /// `"<dotted.path>=<json or string>"` assignments merged over `overrides`.
    #[serde(default)]
    pub assignments: Vec<String>,
    /// Emit `authorize` before every tool call.
    #[serde(default)]
    pub interactive: bool,
    /// Session id to resume or create.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Session directory.
    #[serde(default)]
    pub session_dir: Option<String>,
    /// Extra MCP servers, keyed by name.
    #[serde(default)]
    pub mcp_servers: Option<Map<String, JsonValue>>,
    /// Extra skill sources.
    #[serde(default)]
    pub skill_paths: Vec<String>,
    /// Workspace directory (informational; the process cwd is the workspace).
    #[serde(default)]
    pub cwd: Option<String>,
}

/// A message to the CLI.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum WorkerMessage {
    /// The agent is built.
    Ready {
        /// Current agent state.
        state: State,
    },
    /// A chat event during a turn.
    Event {
        /// The event.
        event: JsonValue,
    },
    /// Ask the CLI whether a tool call may run.
    #[serde(rename_all = "camelCase")]
    Authorize {
        /// Request id; echoed back as `permission.requestId`.
        id: String,
        /// Tool name.
        name: String,
        /// Tool input.
        input: JsonValue,
        /// Whether `input.path` resolves inside the workspace.
        path_in_workspace: bool,
        /// Current contents of the file a `write`/`edit` will touch.
        #[serde(skip_serializing_if = "Option::is_none")]
        before: Option<String>,
    },
    /// A command finished.
    Result {
        /// The outcome.
        result: RunResult,
        /// Current agent state.
        state: State,
        /// Whether the agent asked for a configuration reload.
        reload: bool,
    },
    /// A command failed.
    Error {
        /// Human-readable reason.
        message: String,
    },
}

/// `ChatRunResult` on the wire.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunResult {
    /// Why the turn (or command) stopped.
    pub stop_reason: String,
    /// Final assistant text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_text: Option<String>,
    /// Accumulated token usage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    /// Context-window usage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextUsage>,
}

impl RunResult {
    /// A bare result with just a stop reason.
    pub fn stop(reason: &str) -> Self {
        Self {
            stop_reason: reason.to_owned(),
            ..Self::default()
        }
    }
}

/// `TokenUsage` on the wire.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    /// Prompt tokens.
    pub input_tokens: u64,
    /// Completion tokens.
    pub output_tokens: u64,
    /// Total tokens.
    pub total_tokens: u64,
    /// Tokens read from the prompt cache.
    pub cache_read_input_tokens: u64,
    /// Tokens written to the prompt cache.
    pub cache_write_input_tokens: u64,
}

/// `ChatContextUsage` on the wire.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsage {
    /// Tokens in the last prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_tokens: Option<u64>,
    /// Projected tokens for the next prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projected_tokens: Option<u64>,
    /// The model's context window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

/// The CLI's view of the agent (`PythonState`).
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    /// Agent name.
    pub name: String,
    /// Always `true`: the worker can rebuild the agent from its options.
    pub reconstructable: bool,
    /// Agent description.
    pub description: String,
    /// Model id in use.
    pub model: String,
    /// The model specifier the CLI configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_specifier: Option<String>,
    /// Effort setting.
    pub thinking: JsonValue,
    /// Session id, when sessions are on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Session directory.
    pub session_directory: String,
    /// Session identity, when sessions are on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionIdentity>,
    /// Display-safe conversation (text / toolUse / toolResult blocks only).
    pub messages: Vec<JsonValue>,
    /// Registered tools.
    pub tools: Vec<ToolInfo>,
    /// Available skills.
    pub skills: Vec<SkillInfo>,
    /// Directories the agent writes private data to.
    pub private_paths: Vec<String>,
}

/// `ChatConversation['session']` on the wire.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionIdentity {
    /// Session id.
    pub session_id: String,
    /// Session directory.
    pub session_directory: String,
    /// Workspace directory.
    pub workspace: String,
    /// What the CLI configured.
    pub configured: ConfiguredSession,
}

/// The session settings the CLI asked for.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfiguredSession {
    /// Requested session id, if any.
    pub session_id: Option<String>,
    /// Requested session directory.
    pub session_dir: String,
}

/// One registered tool.
#[derive(Debug, Clone, Serialize)]
pub struct ToolInfo {
    /// Tool name.
    pub name: String,
    /// Tool description.
    pub description: String,
}

/// One available skill.
#[derive(Debug, Clone, Serialize)]
pub struct SkillInfo {
    /// Skill name.
    pub name: String,
    /// Skill description.
    pub description: String,
    /// Skill instructions.
    pub instructions: String,
    /// Whether the skill is active.
    pub active: bool,
}

/// `Some` for any present value, including `null`, so callers can tell "null" from "absent".
fn present<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<JsonValue>, D::Error> {
    JsonValue::deserialize(deserializer).map(Some)
}

/// Replace `{"$bytes": "<base64>"}` wrappers with the bare base64 string the SDK types expect.
pub fn decode_bytes(value: JsonValue) -> JsonValue {
    match value {
        JsonValue::Object(map) => {
            if map.len() == 1 {
                if let Some(JsonValue::String(encoded)) = map.get("$bytes") {
                    return JsonValue::String(encoded.clone());
                }
            }
            JsonValue::Object(map.into_iter().map(|(k, v)| (k, decode_bytes(v))).collect())
        }
        JsonValue::Array(items) => JsonValue::Array(items.into_iter().map(decode_bytes).collect()),
        other => other,
    }
}

/// Wrap a base64 string as `{"$bytes": ...}` for the CLI.
pub fn bytes_value(encoded: &str) -> JsonValue {
    serde_json::json!({ "$bytes": encoded })
}
