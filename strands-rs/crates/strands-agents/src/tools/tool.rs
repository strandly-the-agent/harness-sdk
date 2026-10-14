//! The `Tool` trait and tool execution context (`strands-ts/src/tools/tool.ts`).

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::agent::{Agent, InvocationState};
use crate::error::{Error, Result};
use crate::interrupt::{self, InterruptFn, InterruptSource};
use crate::types::{InterruptParams, JsonValue, ToolResultBlock, ToolSpec, ToolUse};

/// Progress data emitted by a tool while it runs (`ToolStreamEvent` in TS).
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(tag = "type", rename = "toolStreamEvent")]
#[derive(Default)]
#[non_exhaustive]
pub struct ToolStreamEvent {
    /// Arbitrary progress payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<JsonValue>,
}

impl ToolStreamEvent {
    /// A progress event carrying `data`.
    pub fn new(data: impl Into<Option<JsonValue>>) -> Self {
        Self { data: data.into() }
    }
}

/// Everything a tool receives when invoked.
#[derive(Clone)]
#[non_exhaustive]
pub struct ToolContext {
    /// The invocation request.
    pub tool_use: ToolUse,
    /// The invoking agent.
    pub agent: Agent,
    /// Per-invocation shared state.
    pub invocation_state: InvocationState,
    /// Cancellation for this tool call.
    pub cancel: CancellationToken,
    progress: Option<mpsc::Sender<JsonValue>>,
    interrupt: Option<InterruptFn>,
}

impl ToolContext {
    /// Context without a progress channel (progress events are dropped).
    pub fn new(
        tool_use: ToolUse,
        agent: Agent,
        invocation_state: InvocationState,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            tool_use,
            agent,
            invocation_state,
            cancel,
            progress: None,
            interrupt: None,
        }
    }

    /// Override the raise closure (used to re-scope a sub-agent's interrupt ids).
    #[must_use]
    pub fn with_interrupt(mut self, interrupt: InterruptFn) -> Self {
        self.interrupt = Some(interrupt);
        self
    }

    /// Ask the caller for input: returns the answer on resume, otherwise raises
    /// [`Error::Interrupt`] to halt the invocation.
    ///
    /// Propagate the error (`?`) — catching it turns a pause into a tool failure.
    pub fn interrupt<T: serde::de::DeserializeOwned>(&self, params: InterruptParams) -> Result<T> {
        let response = match &self.interrupt {
            Some(raise) => raise(params)?,
            None => {
                let id = interrupt::tool_interrupt_id(&self.tool_use.tool_use_id, &params.name);
                interrupt::raise(&self.agent.interrupt_state_shared(), &id, params, InterruptSource::Tool)?
            }
        };
        serde_json::from_value(response).map_err(|e| Error::other(e.to_string()))
    }

    /// Attach a progress channel; emitted values surface as `ToolStreamUpdate` events.
    pub fn with_progress(mut self, sender: mpsc::Sender<JsonValue>) -> Self {
        self.progress = Some(sender);
        self
    }

    /// Emit a progress value. No-op when nobody is listening.
    pub async fn progress(&self, data: impl Into<JsonValue>) {
        if let Some(tx) = &self.progress {
            let _ = tx.send(data.into()).await;
        }
    }

    /// Tool input as `T`.
    pub fn input<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_value(self.tool_use.input.clone()).map_err(Error::from)
    }
}

impl std::fmt::Debug for ToolContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolContext")
            .field("tool_use", &self.tool_use)
            .finish_non_exhaustive()
    }
}

/// A tool the model can call. Tools are shared as `Arc<dyn Tool>`.
///
/// `invoke` returns `Ok` for every ordinary outcome — tool failures are an `Ok` result with
/// `status: error`. `Err` is reserved for exceptional control flow (cancellation).
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name (1–64 chars, `[a-zA-Z0-9_-]`).
    fn name(&self) -> &str;
    /// Description shown to the model.
    fn description(&self) -> &str;
    /// Full spec sent to the model.
    fn spec(&self) -> &ToolSpec;
    /// Run the tool.
    async fn invoke(&self, ctx: ToolContext) -> Result<ToolResultBlock>;
}

/// Shared tool handle.
pub type ToolRef = Arc<dyn Tool>;

/// `status: error` result with content `Error: <message>` (`createErrorResult` in TS).
pub fn create_error_result(error: impl std::fmt::Display, tool_use_id: &str) -> ToolResultBlock {
    ToolResultBlock::error(tool_use_id, format!("Error: {error}"))
}

/// 1..=64 chars of `[a-zA-Z0-9_-]`.
pub fn is_valid_tool_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Test-only context backed by [`crate::agent::test_agent`].
#[cfg(test)]
pub(crate) fn test_context(name: &str, tool_use_id: &str, input: JsonValue) -> ToolContext {
    ToolContext::new(
        ToolUse {
            name: name.to_owned(),
            tool_use_id: tool_use_id.to_owned(),
            input,
        },
        crate::agent::test_agent(),
        InvocationState::new(),
        CancellationToken::new(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_name_rules() {
        assert!(is_valid_tool_name("a"));
        assert!(is_valid_tool_name("my-tool_1"));
        assert!(!is_valid_tool_name(""));
        assert!(!is_valid_tool_name(&"a".repeat(65)));
        assert!(!is_valid_tool_name("bad name"));
        assert!(!is_valid_tool_name("ünï"));
    }

    #[test]
    fn error_result_has_prefix() {
        let r = create_error_result("boom", "t1");
        assert_eq!(r.text(), "Error: boom");
        assert_eq!(r.status, crate::types::ToolResultStatus::Error);
    }

    #[test]
    fn tool_stream_event_omits_absent_data() {
        assert_eq!(
            serde_json::to_value(ToolStreamEvent { data: None }).unwrap(),
            serde_json::json!({"type": "toolStreamEvent"})
        );
    }
}

#[cfg(test)]
mod name_and_event_tests {
    use super::*;

    #[test]
    fn accepts_valid_tool_names() {
        for name in [
            "simple",
            "with_underscore",
            "with-hyphen",
            "Mixed-Case_123",
            "a",
            &"a".repeat(64),
        ] {
            assert!(is_valid_tool_name(name), "{name} should be valid");
        }
    }

    #[test]
    fn rejects_invalid_tool_names() {
        for name in [
            "",
            &"a".repeat(65),
            "has space",
            "has.dot",
            "has/slash",
            "has:colon",
            "emoji🚀",
        ] {
            assert!(!is_valid_tool_name(name), "{name} should be invalid");
        }
    }

    #[test]
    fn creates_tool_stream_event_with_data() {
        let event = ToolStreamEvent {
            data: Some(serde_json::json!("progress")),
        };
        assert_eq!(event.data, Some(serde_json::json!("progress")));
    }

    #[test]
    fn creates_tool_stream_event_without_data() {
        assert_eq!(ToolStreamEvent { data: None }.data, None);
    }

    #[test]
    fn creates_tool_stream_event_with_structured_data() {
        let data = serde_json::json!({ "progress": 0.5, "message": "halfway" });
        let event = ToolStreamEvent {
            data: Some(data.clone()),
        };
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            serde_json::json!({ "type": "toolStreamEvent", "data": data })
        );
    }
}
