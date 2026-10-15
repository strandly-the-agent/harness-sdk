//! A2A protocol wire types (`@a2a-js/sdk` 0.3.x `AgentCard`, `Message`, `Task`, … and the
//! JSON-RPC 2.0 envelope).
//!
//! Hand-written serde structs rather than a third-party crate; see the A2A section of
//! `docs/DESIGN.md` for the dependency decision. Field names and casing are taken verbatim from
//! the `@a2a-js/sdk` type declarations, so a Rust server and a TS/Python client agree on the wire.

use serde::{Deserialize, Serialize};

use crate::types::JsonValue;

/// Protocol version advertised in the agent card.
pub const PROTOCOL_VERSION: &str = "0.3.0";

/// Well-known path the agent card is served from.
pub const AGENT_CARD_PATH: &str = "/.well-known/agent-card.json";

/// Deprecated pre-0.3 agent card path, still served as an alias.
pub const PREV_AGENT_CARD_PATH: &str = "/.well-known/agent.json";

/// `message/send`: run the agent and return the final `Task` or `Message`.
pub const METHOD_MESSAGE_SEND: &str = "message/send";

/// `message/stream`: run the agent and stream its events as server-sent events.
pub const METHOD_MESSAGE_STREAM: &str = "message/stream";

/// `tasks/get`: fetch a stored task.
pub const METHOD_TASKS_GET: &str = "tasks/get";

/// `tasks/cancel`: cancel a running task.
pub const METHOD_TASKS_CANCEL: &str = "tasks/cancel";

/// Free-form JSON metadata carried by most protocol types.
pub type Metadata = serde_json::Map<String, JsonValue>;

/// Who sent a [`Message`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// The client.
    #[default]
    User,
    /// The served agent.
    Agent,
}

/// One piece of a [`Message`] or [`Artifact`], discriminated by `kind`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
#[non_exhaustive]
pub enum Part {
    /// Plain text.
    Text {
        /// The text.
        text: String,
        /// Part metadata.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<Metadata>,
    },
    /// A file, inline or by URI.
    File {
        /// The file.
        file: FileContent,
        /// Part metadata.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<Metadata>,
    },
    /// Structured JSON data.
    Data {
        /// The data.
        data: Metadata,
        /// Part metadata.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<Metadata>,
    },
}

impl Part {
    /// A text part.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            metadata: None,
        }
    }

    /// The text, when this is a text part.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text, .. } => Some(text),
            _ => None,
        }
    }
}

/// A file part's payload: base64 `bytes` or a `uri`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum FileContent {
    /// Inline base64-encoded bytes.
    Bytes {
        /// Base64-encoded content.
        bytes: String,
        /// MIME type.
        #[serde(default, rename = "mimeType", skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        /// File name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// A URI the content can be fetched from.
    Uri {
        /// The URI.
        uri: String,
        /// MIME type.
        #[serde(default, rename = "mimeType", skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        /// File name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
}

/// A single turn exchanged with an agent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename = "message", rename_all = "camelCase")]
#[non_exhaustive]
pub struct Message {
    /// Unique id of this message.
    pub message_id: String,
    /// Who sent it.
    pub role: Role,
    /// Its content.
    pub parts: Vec<Part>,
    /// The conversation context this message belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
    /// The task this message belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// Tasks the recipient should take into account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_task_ids: Option<Vec<String>>,
    /// Message metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
}

impl Message {
    /// A message of `parts` with a freshly generated id.
    pub fn new(role: Role, parts: Vec<Part>) -> Self {
        Self {
            message_id: uuid::Uuid::new_v4().to_string(),
            role,
            parts,
            context_id: None,
            task_id: None,
            reference_task_ids: None,
            metadata: None,
        }
    }

    /// A user message of a single text part.
    pub fn user(text: impl Into<String>) -> Self {
        Self::new(Role::User, vec![Part::text(text)])
    }

    /// The text parts joined by newlines; non-text parts are skipped.
    pub fn text(&self) -> String {
        text_from_parts(&self.parts)
    }
}

/// Lifecycle state of a [`Task`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum TaskState {
    /// Accepted, not started.
    #[default]
    Submitted,
    /// Running.
    Working,
    /// Waiting for more client input.
    InputRequired,
    /// Finished successfully.
    Completed,
    /// Cancelled by the client.
    Canceled,
    /// Finished with an error.
    Failed,
    /// Refused by the agent.
    Rejected,
    /// Waiting for the client to authenticate.
    AuthRequired,
    /// State could not be determined.
    Unknown,
}

impl TaskState {
    /// Whether no further work can change this state.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Canceled | Self::Failed | Self::Rejected
        )
    }

    /// The wire string, as it appears in JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Submitted => "submitted",
            Self::Working => "working",
            Self::InputRequired => "input-required",
            Self::Completed => "completed",
            Self::Canceled => "canceled",
            Self::Failed => "failed",
            Self::Rejected => "rejected",
            Self::AuthRequired => "auth-required",
            Self::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for TaskState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A [`Task`]'s state plus the message that explains it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct TaskStatus {
    /// The state.
    pub state: TaskState,
    /// A message describing the state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Message>,
    /// RFC 3339 timestamp of the transition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

impl TaskStatus {
    /// A status in `state` with no message.
    pub fn new(state: TaskState) -> Self {
        Self {
            state,
            message: None,
            timestamp: None,
        }
    }
}

/// An output the agent produced while working on a [`Task`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Artifact {
    /// Unique id of this artifact.
    pub artifact_id: String,
    /// Its content.
    pub parts: Vec<Part>,
    /// Human-readable name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Artifact metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
}

impl Artifact {
    /// An artifact of `parts` under `artifact_id`.
    pub fn new(artifact_id: impl Into<String>, parts: Vec<Part>) -> Self {
        Self {
            artifact_id: artifact_id.into(),
            parts,
            name: None,
            description: None,
            metadata: None,
        }
    }
}

/// A unit of work the agent is doing for a client.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename = "task", rename_all = "camelCase")]
#[non_exhaustive]
pub struct Task {
    /// Unique id of this task.
    pub id: String,
    /// The conversation context it belongs to.
    pub context_id: String,
    /// Its current status.
    pub status: TaskStatus,
    /// Outputs produced so far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<Vec<Artifact>>,
    /// Messages exchanged on this task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<Vec<Message>>,
    /// Task metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
}

impl Task {
    /// A task in `status` with no artifacts or history.
    pub fn new(id: impl Into<String>, context_id: impl Into<String>, status: TaskStatus) -> Self {
        Self {
            id: id.into(),
            context_id: context_id.into(),
            status,
            artifacts: None,
            history: None,
            metadata: None,
        }
    }
}

/// A [`Task`]'s status changed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename = "status-update", rename_all = "camelCase")]
#[non_exhaustive]
pub struct TaskStatusUpdateEvent {
    /// The task.
    pub task_id: String,
    /// Its context.
    pub context_id: String,
    /// The new status.
    pub status: TaskStatus,
    /// Whether this is the last event of the stream.
    #[serde(rename = "final")]
    pub final_: bool,
    /// Event metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
}

/// A [`Task`] produced or extended an artifact.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename = "artifact-update", rename_all = "camelCase")]
#[non_exhaustive]
pub struct TaskArtifactUpdateEvent {
    /// The task.
    pub task_id: String,
    /// Its context.
    pub context_id: String,
    /// The artifact chunk.
    pub artifact: Artifact,
    /// Whether to append to the artifact of the same id rather than replace it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub append: Option<bool>,
    /// Whether this is the artifact's last chunk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_chunk: Option<bool>,
    /// Event metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
}

/// Parameters of `message/send` and `message/stream`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct MessageSendParams {
    /// The message to send.
    pub message: Message,
    /// How the caller wants the response delivered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<MessageSendConfiguration>,
    /// Request metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
}

impl MessageSendParams {
    /// Params sending `message` with no configuration.
    pub fn new(message: Message) -> Self {
        Self {
            message,
            configuration: None,
            metadata: None,
        }
    }
}

/// Response-delivery options of [`MessageSendParams`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct MessageSendConfiguration {
    /// MIME types the client accepts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_output_modes: Option<Vec<String>>,
    /// Whether to wait for the task to finish before responding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocking: Option<bool>,
    /// How many history messages to include on the returned task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_length: Option<u32>,
}

/// Parameters of `tasks/get`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct TaskQueryParams {
    /// The task id.
    pub id: String,
    /// How many history messages to include (all when absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_length: Option<u32>,
    /// Request metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
}

impl TaskQueryParams {
    /// Query for `id` with full history.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            history_length: None,
            metadata: None,
        }
    }
}

/// Parameters of `tasks/cancel`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct TaskIdParams {
    /// The task id.
    pub id: String,
    /// Request metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
}

impl TaskIdParams {
    /// Params naming `id`.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            metadata: None,
        }
    }
}

/// What a client learns about an agent before talking to it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct AgentCard {
    /// Human-readable name.
    pub name: String,
    /// What the agent does.
    pub description: String,
    /// Base URL of its JSON-RPC endpoint.
    pub url: String,
    /// The agent's own version string.
    pub version: String,
    /// A2A protocol version it speaks.
    pub protocol_version: String,
    /// What it supports.
    pub capabilities: AgentCapabilities,
    /// MIME types every skill accepts by default.
    pub default_input_modes: Vec<String>,
    /// MIME types every skill produces by default.
    pub default_output_modes: Vec<String>,
    /// The skills it advertises.
    pub skills: Vec<AgentSkill>,
    /// Who runs it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<AgentProvider>,
    /// Link to its documentation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documentation_url: Option<String>,
    /// Link to its icon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
}

impl AgentCard {
    /// A card for a streaming JSON-RPC agent with `text/plain` in and out and no skills.
    pub fn new(name: impl Into<String>, description: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            url: url.into(),
            version: "0.0.1".to_owned(),
            protocol_version: PROTOCOL_VERSION.to_owned(),
            capabilities: AgentCapabilities {
                streaming: Some(true),
                ..AgentCapabilities::default()
            },
            default_input_modes: vec!["text/plain".to_owned()],
            default_output_modes: vec!["text/plain".to_owned()],
            skills: Vec::new(),
            provider: None,
            documentation_url: None,
            icon_url: None,
        }
    }
}

/// Optional protocol features an agent supports.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct AgentCapabilities {
    /// Whether `message/stream` is available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub streaming: Option<bool>,
    /// Whether push notification configs are accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push_notifications: Option<bool>,
    /// Whether task status history is retained.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_transition_history: Option<bool>,
}

/// One capability an agent advertises on its card.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct AgentSkill {
    /// Unique id of the skill.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// What the skill does.
    pub description: String,
    /// Tags a client can match on.
    pub tags: Vec<String>,
    /// Example prompts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub examples: Option<Vec<String>>,
    /// MIME types this skill accepts, overriding the card default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_modes: Option<Vec<String>>,
    /// MIME types this skill produces, overriding the card default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_modes: Option<Vec<String>>,
}

impl AgentSkill {
    /// A skill with no examples and no per-skill modes.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
        tags: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            tags: tags.into_iter().map(Into::into).collect(),
            examples: None,
            input_modes: None,
            output_modes: None,
        }
    }
}

/// The organization running an agent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AgentProvider {
    /// Organization name.
    pub organization: String,
    /// Organization URL.
    pub url: String,
}

/// An event a `message/stream` subscription delivers, discriminated by `kind`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum A2AEventData {
    /// A complete message (the agent answered without creating a task).
    Message(Message),
    /// A task snapshot.
    Task(Task),
    /// A task's status changed.
    StatusUpdate(TaskStatusUpdateEvent),
    /// A task produced or extended an artifact.
    ArtifactUpdate(TaskArtifactUpdateEvent),
}

impl A2AEventData {
    /// The `kind` discriminator, as it appears on the wire.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Message(_) => "message",
            Self::Task(_) => "task",
            Self::StatusUpdate(_) => "status-update",
            Self::ArtifactUpdate(_) => "artifact-update",
        }
    }

    /// Whether this event terminates the response (TS `_isCompleteEvent`).
    pub fn is_complete(&self) -> bool {
        match self {
            Self::Message(_) | Self::Task(_) => true,
            Self::ArtifactUpdate(e) => e.last_chunk == Some(true),
            Self::StatusUpdate(e) => e.status.state == TaskState::Completed,
        }
    }

    /// The text this event carries (TS `_extractTextFromEvent`).
    pub fn text(&self) -> String {
        match self {
            Self::Message(m) => text_from_parts(&m.parts),
            Self::Task(t) => {
                let parts: Vec<Part> = t
                    .artifacts
                    .iter()
                    .flatten()
                    .flat_map(|a| a.parts.iter().cloned())
                    .collect();
                let from_artifacts = text_from_parts(&parts);
                if from_artifacts.is_empty() {
                    t.status
                        .message
                        .as_ref()
                        .map(|m| text_from_parts(&m.parts))
                        .unwrap_or_default()
                } else {
                    from_artifacts
                }
            }
            Self::ArtifactUpdate(e) => text_from_parts(&e.artifact.parts),
            Self::StatusUpdate(e) => e
                .status
                .message
                .as_ref()
                .map(|m| text_from_parts(&m.parts))
                .unwrap_or_default(),
        }
    }

    /// The task status this event reports, when it carries one.
    pub fn status(&self) -> Option<&TaskStatus> {
        match self {
            Self::Task(t) => Some(&t.status),
            Self::StatusUpdate(e) => Some(&e.status),
            _ => None,
        }
    }
}

/// Text parts joined by newlines; non-text parts are skipped (TS `_textFromParts`).
pub fn text_from_parts(parts: &[Part]) -> String {
    parts
        .iter()
        .filter_map(Part::as_text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// JSON-RPC 2.0 error codes used by A2A.
pub mod error_codes {
    /// Malformed JSON.
    pub const PARSE_ERROR: i32 = -32700;
    /// Not a valid JSON-RPC request.
    pub const INVALID_REQUEST: i32 = -32600;
    /// Unknown method.
    pub const METHOD_NOT_FOUND: i32 = -32601;
    /// Params did not match the method.
    pub const INVALID_PARAMS: i32 = -32602;
    /// The server failed.
    pub const INTERNAL_ERROR: i32 = -32603;
    /// No task with that id.
    pub const TASK_NOT_FOUND: i32 = -32001;
    /// The task is in a state that cannot be cancelled.
    pub const TASK_NOT_CANCELABLE: i32 = -32002;
    /// Push notifications are not supported.
    pub const PUSH_NOTIFICATION_NOT_SUPPORTED: i32 = -32003;
    /// The operation is not supported by this agent.
    pub const UNSUPPORTED_OPERATION: i32 = -32004;
    /// The requested content type is not supported.
    pub const CONTENT_TYPE_NOT_SUPPORTED: i32 = -32005;
    /// The agent produced an invalid response.
    pub const INVALID_AGENT_RESPONSE: i32 = -32006;
}

/// A JSON-RPC 2.0 request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct JsonRpcRequest {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// Caller-chosen id, echoed on the response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<JsonValue>,
    /// The method name.
    pub method: String,
    /// The method's parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<JsonValue>,
}

impl JsonRpcRequest {
    /// A request calling `method` with `params` under a freshly generated id.
    pub fn new(method: impl Into<String>, params: impl Serialize) -> Result<Self, serde_json::Error> {
        Ok(Self {
            jsonrpc: "2.0".to_owned(),
            id: Some(JsonValue::String(uuid::Uuid::new_v4().to_string())),
            method: method.into(),
            params: Some(serde_json::to_value(params)?),
        })
    }
}

/// A JSON-RPC 2.0 error object.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[error("{message} (code {code})")]
#[non_exhaustive]
pub struct JsonRpcError {
    /// The error code; see [`error_codes`].
    pub code: i32,
    /// Human-readable message.
    pub message: String,
    /// Extra data about the error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<JsonValue>,
}

impl JsonRpcError {
    /// An error with `code` and `message` and no data.
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// `-32700` malformed JSON.
    pub fn parse_error(message: impl Into<String>) -> Self {
        Self::new(error_codes::PARSE_ERROR, message)
    }

    /// `-32600` not a valid JSON-RPC request.
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(error_codes::INVALID_REQUEST, message)
    }

    /// `-32601` unknown method.
    pub fn method_not_found(method: &str) -> Self {
        Self::new(
            error_codes::METHOD_NOT_FOUND,
            format!("Method not found: {method}"),
        )
    }

    /// `-32602` params did not match the method.
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(error_codes::INVALID_PARAMS, message)
    }

    /// `-32603` the server failed.
    pub fn internal_error(message: impl Into<String>) -> Self {
        Self::new(error_codes::INTERNAL_ERROR, message)
    }

    /// `-32001` no task with that id.
    pub fn task_not_found(id: &str) -> Self {
        Self::new(error_codes::TASK_NOT_FOUND, format!("Task not found: {id}"))
    }

    /// `-32002` the task cannot be cancelled.
    pub fn task_not_cancelable(id: &str) -> Self {
        Self::new(
            error_codes::TASK_NOT_CANCELABLE,
            format!("Task cannot be canceled: {id}"),
        )
    }

    /// `-32004` the operation is not supported by this agent.
    pub fn unsupported_operation(message: impl Into<String>) -> Self {
        Self::new(error_codes::UNSUPPORTED_OPERATION, message)
    }
}

/// A JSON-RPC 2.0 response: exactly one of `result` or `error` is present.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct JsonRpcResponse {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// The id of the request being answered.
    pub id: Option<JsonValue>,
    /// The result, on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<JsonValue>,
    /// The error, on failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    /// A success response carrying `result`.
    pub fn success(id: Option<JsonValue>, result: JsonValue) -> Self {
        Self {
            jsonrpc: "2.0".to_owned(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// A failure response carrying `error`.
    pub fn error(id: Option<JsonValue>, error: JsonRpcError) -> Self {
        Self {
            jsonrpc: "2.0".to_owned(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn message_serializes_with_its_kind_discriminator() {
        let mut message = Message::user("hi");
        message.message_id = "m1".to_owned();
        assert_eq!(
            serde_json::to_value(&message).unwrap(),
            json!({
                "kind": "message",
                "messageId": "m1",
                "role": "user",
                "parts": [{ "kind": "text", "text": "hi" }],
            })
        );
    }

    #[test]
    fn task_serializes_with_camel_case_fields_and_its_kind() {
        let task = Task::new("t1", "c1", TaskStatus::new(TaskState::Working));
        assert_eq!(
            serde_json::to_value(&task).unwrap(),
            json!({ "kind": "task", "id": "t1", "contextId": "c1", "status": { "state": "working" } })
        );
    }

    #[test]
    fn update_events_serialize_with_kebab_case_kinds() {
        let status = TaskStatusUpdateEvent {
            task_id: "t1".to_owned(),
            context_id: "c1".to_owned(),
            status: TaskStatus::new(TaskState::Completed),
            final_: true,
            metadata: None,
        };
        assert_eq!(
            serde_json::to_value(&status).unwrap(),
            json!({
                "kind": "status-update",
                "taskId": "t1",
                "contextId": "c1",
                "status": { "state": "completed" },
                "final": true,
            })
        );

        let artifact = TaskArtifactUpdateEvent {
            task_id: "t1".to_owned(),
            context_id: "c1".to_owned(),
            artifact: Artifact::new("a1", vec![Part::text("chunk")]),
            append: Some(false),
            last_chunk: Some(true),
            metadata: None,
        };
        assert_eq!(
            serde_json::to_value(&artifact).unwrap(),
            json!({
                "kind": "artifact-update",
                "taskId": "t1",
                "contextId": "c1",
                "artifact": { "artifactId": "a1", "parts": [{ "kind": "text", "text": "chunk" }] },
                "append": false,
                "lastChunk": true,
            })
        );
    }

    #[test]
    fn every_event_kind_round_trips_through_the_union() {
        let events = vec![
            A2AEventData::Message(Message::user("hi")),
            A2AEventData::Task(Task::new("t", "c", TaskStatus::new(TaskState::Submitted))),
            A2AEventData::StatusUpdate(TaskStatusUpdateEvent {
                task_id: "t".to_owned(),
                context_id: "c".to_owned(),
                status: TaskStatus::new(TaskState::Working),
                final_: false,
                metadata: None,
            }),
            A2AEventData::ArtifactUpdate(TaskArtifactUpdateEvent {
                task_id: "t".to_owned(),
                context_id: "c".to_owned(),
                artifact: Artifact::new("a", vec![Part::text("x")]),
                append: None,
                last_chunk: None,
                metadata: None,
            }),
        ];
        for event in events {
            let json = serde_json::to_value(&event).unwrap();
            assert_eq!(json["kind"], event.kind());
            let back: A2AEventData = serde_json::from_value(json).unwrap();
            assert_eq!(back, event);
        }
    }

    #[test]
    fn file_parts_round_trip_both_bytes_and_uri_payloads() {
        let bytes = Part::File {
            file: FileContent::Bytes {
                bytes: "AAA=".to_owned(),
                mime_type: Some("image/png".to_owned()),
                name: None,
            },
            metadata: None,
        };
        assert_eq!(
            serde_json::to_value(&bytes).unwrap(),
            json!({ "kind": "file", "file": { "bytes": "AAA=", "mimeType": "image/png" } })
        );
        assert_eq!(
            serde_json::from_value::<Part>(serde_json::to_value(&bytes).unwrap()).unwrap(),
            bytes
        );

        let uri = Part::File {
            file: FileContent::Uri {
                uri: "https://example.com/a.pdf".to_owned(),
                mime_type: None,
                name: Some("a.pdf".to_owned()),
            },
            metadata: None,
        };
        assert_eq!(
            serde_json::to_value(&uri).unwrap(),
            json!({ "kind": "file", "file": { "uri": "https://example.com/a.pdf", "name": "a.pdf" } })
        );
        assert_eq!(
            serde_json::from_value::<Part>(serde_json::to_value(&uri).unwrap()).unwrap(),
            uri
        );
    }

    #[test]
    fn task_states_use_the_protocol_spelling() {
        assert_eq!(
            serde_json::to_value(TaskState::InputRequired).unwrap(),
            json!("input-required")
        );
        assert_eq!(
            serde_json::from_value::<TaskState>(json!("auth-required")).unwrap(),
            TaskState::AuthRequired
        );
        assert!(TaskState::Completed.is_terminal());
        assert!(!TaskState::Working.is_terminal());
    }

    #[test]
    fn agent_card_defaults_to_streaming_json_rpc_over_text_plain() {
        let card = AgentCard::new("a", "does things", "http://127.0.0.1:9000/");
        assert_eq!(
            serde_json::to_value(&card).unwrap(),
            json!({
                "name": "a",
                "description": "does things",
                "url": "http://127.0.0.1:9000/",
                "version": "0.0.1",
                "protocolVersion": "0.3.0",
                "capabilities": { "streaming": true },
                "defaultInputModes": ["text/plain"],
                "defaultOutputModes": ["text/plain"],
                "skills": [],
            })
        );
    }

    #[test]
    fn a_json_rpc_response_carries_result_xor_error() {
        let ok = JsonRpcResponse::success(Some(json!(1)), json!({ "a": 1 }));
        assert_eq!(
            serde_json::to_value(&ok).unwrap(),
            json!({ "jsonrpc": "2.0", "id": 1, "result": { "a": 1 } })
        );
        let err = JsonRpcResponse::error(Some(json!("x")), JsonRpcError::task_not_found("t1"));
        assert_eq!(
            serde_json::to_value(&err).unwrap(),
            json!({
                "jsonrpc": "2.0",
                "id": "x",
                "error": { "code": -32001, "message": "Task not found: t1" },
            })
        );
    }

    #[test]
    fn event_text_prefers_artifacts_then_falls_back_to_the_status_message() {
        let mut task = Task::new("t", "c", TaskStatus::new(TaskState::Completed));
        task.status.message = Some(Message::new(Role::Agent, vec![Part::text("status")]));
        assert_eq!(A2AEventData::Task(task.clone()).text(), "status");

        task.artifacts = Some(vec![Artifact::new("a", vec![Part::text("answer")])]);
        assert_eq!(A2AEventData::Task(task).text(), "answer");
    }
}
