//! The tool-call ledger context provider
//! (`strands-ts/src/vended-interventions/steering/providers/tool-ledger.ts`).

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::Serialize;

use crate::agent::Agent;
use crate::error::Result;
use crate::hooks::events::{AfterToolCallEvent, BeforeToolCallEvent};
use crate::types::{JsonValue, ToolResultStatus, ToolUse};

use super::context::{SteeringContextData, SteeringContextProvider};

/// Default provider name.
pub const DEFAULT_TOOL_LEDGER_NAME: &str = "strands:steering:toolLedger";
/// Default number of ledger entries retained.
pub const DEFAULT_MAX_ENTRIES: usize = 100;

/// Execution state of a ledger entry: `pending` while in flight, then the tool's own status.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum LedgerStatus {
    Pending,
    Success,
    Error,
}

impl From<ToolResultStatus> for LedgerStatus {
    fn from(status: ToolResultStatus) -> Self {
        match status {
            ToolResultStatus::Success => LedgerStatus::Success,
            ToolResultStatus::Error => LedgerStatus::Error,
        }
    }
}

/// A single entry in the tool call ledger. Field order is the serialization order TS produces.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LedgerToolCall {
    start_time: u64,
    id: String,
    name: String,
    args: JsonValue,
    status: LedgerStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    end_time: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<JsonValue>,
    /// `None` while pending; `Some(None)` (JSON `null`) once a call finished without an error.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Option<String>>,
}

/// Context provider that tracks tool call history within a single invocation.
///
/// Records every tool invocation with its input, timing and success/failure status, so a steering
/// handler can spot patterns (repeated failures, excessive retries). Once the ledger exceeds
/// [`max_entries`](Self::max_entries), the oldest entries are dropped.
///
/// ```
/// use strands_agents::interventions::vended::steering::{LlmSteeringHandler, ToolLedgerProvider};
///
/// let handler = LlmSteeringHandler::new("Guide the agent away from repeated failures.")
///     .context_providers([ToolLedgerProvider::new().max_entries(20).shared()]);
/// ```
#[derive(Clone)]
pub struct ToolLedgerProvider {
    name: String,
    max_entries: usize,
    calls: Arc<Mutex<Vec<LedgerToolCall>>>,
}

impl ToolLedgerProvider {
    /// Ledger retaining [`DEFAULT_MAX_ENTRIES`] calls.
    pub fn new() -> Self {
        Self {
            name: DEFAULT_TOOL_LEDGER_NAME.to_owned(),
            max_entries: DEFAULT_MAX_ENTRIES,
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Override the provider name (needed when attaching more than one ledger).
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Maximum number of tool calls to retain; older entries are dropped.
    #[must_use]
    pub fn max_entries(mut self, max_entries: usize) -> Self {
        self.max_entries = max_entries;
        self
    }

    /// This provider as a shared handle, ready to hand to a handler.
    pub fn shared(self) -> Arc<dyn SteeringContextProvider> {
        Arc::new(self)
    }

    fn record_pending(&self, tool_use: &ToolUse) {
        let mut calls = self.calls.lock();
        calls.push(LedgerToolCall {
            start_time: now_ms(),
            id: tool_use.tool_use_id.clone(),
            name: tool_use.name.clone(),
            args: tool_use.input.clone(),
            status: LedgerStatus::Pending,
            end_time: None,
            result: None,
            error: None,
        });
        if calls.len() > self.max_entries {
            let excess = calls.len() - self.max_entries;
            calls.drain(..excess);
        }
    }

    fn record_completion(&self, event: &AfterToolCallEvent) {
        let mut calls = self.calls.lock();
        let Some(call) = calls
            .iter_mut()
            .rev()
            .find(|call| call.id == event.tool_use.tool_use_id)
        else {
            return;
        };
        call.end_time = Some(now_ms());
        call.status = event.result.status.into();
        call.result = serde_json::to_value(&event.result.content).ok();
        call.error = Some(event.error.as_ref().map(ToString::to_string));
    }
}

impl Default for ToolLedgerProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SteeringContextProvider for ToolLedgerProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn observe_agent(&self, agent: &Agent) -> Result<()> {
        let provider = self.clone();
        agent
            .hooks()
            .add_sync_callback(move |event: &mut BeforeToolCallEvent| {
                provider.record_pending(&event.tool_use);
                Ok(())
            });

        let provider = self.clone();
        agent
            .hooks()
            .add_sync_callback(move |event: &mut AfterToolCallEvent| {
                provider.record_completion(event);
                Ok(())
            });
        Ok(())
    }

    fn context(&self) -> SteeringContextData {
        let calls = serde_json::to_value(&*self.calls.lock()).unwrap_or(JsonValue::Null);
        SteeringContextData::new("toolLedger").with("calls", calls)
    }
}

/// Milliseconds since the Unix epoch (TS uses an ISO string; see `docs/matrices/steering.md`).
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::Error;
    use crate::hooks::events::Cancel;
    use crate::testing::{MockMessageModel, MockResponse};
    use crate::types::{Message, ToolResultBlock, ToolResultContent};

    async fn agent_observed_by(provider: &ToolLedgerProvider) -> Agent {
        let model = MockMessageModel::shared(vec![MockResponse::message(Message::assistant("hi"))]);
        let agent = Agent::builder().model(model).build().unwrap();
        provider.observe_agent(&agent).await.unwrap();
        agent
    }

    fn tool_use(id: &str) -> ToolUse {
        ToolUse {
            name: "searchWeb".to_owned(),
            tool_use_id: id.to_owned(),
            input: json!({ "q": "hi" }),
        }
    }

    fn before(agent: &Agent, tool_use: ToolUse) -> BeforeToolCallEvent {
        BeforeToolCallEvent {
            agent: agent.clone(),
            tool_use,
            tool: None,
            invocation_state: Default::default(),
            cancel: Cancel::No,
            selected_tool: None,
        }
    }

    fn after(agent: &Agent, status: ToolResultStatus, error: Option<Error>) -> AfterToolCallEvent {
        let tool_use = tool_use("tu-1");
        AfterToolCallEvent {
            agent: agent.clone(),
            result: ToolResultBlock {
                tool_use_id: tool_use.tool_use_id.clone(),
                status,
                content: vec![ToolResultContent::Text("result text".to_owned())],
                error: None,
            },
            tool_use,
            tool: None,
            error: error.map(Arc::new),
            invocation_state: Default::default(),
            retry: false,
        }
    }

    fn calls_of(provider: &ToolLedgerProvider) -> Vec<JsonValue> {
        provider.context().fields["calls"]
            .as_array()
            .expect("calls is an array")
            .clone()
    }

    #[tokio::test]
    async fn records_pending_entry_on_before_tool_call() {
        let provider = ToolLedgerProvider::new();
        let agent = agent_observed_by(&provider).await;

        assert_eq!(provider.context().type_name, "toolLedger");
        assert_eq!(calls_of(&provider), Vec::<JsonValue>::new());

        let mut event = before(&agent, tool_use("tu-1"));
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        let calls = calls_of(&provider);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["id"], json!("tu-1"));
        assert_eq!(calls[0]["name"], json!("searchWeb"));
        assert_eq!(calls[0]["args"], json!({ "q": "hi" }));
        assert_eq!(calls[0]["status"], json!("pending"));
        assert!(calls[0].get("endTime").is_none());
    }

    #[tokio::test]
    async fn flips_pending_to_success_after_after_tool_call() {
        let provider = ToolLedgerProvider::new();
        let agent = agent_observed_by(&provider).await;

        let mut event = before(&agent, tool_use("tu-1"));
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();
        let mut event = after(&agent, ToolResultStatus::Success, None);
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        let calls = calls_of(&provider);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["status"], json!("success"));
        assert_eq!(calls[0]["error"], JsonValue::Null);
        assert_eq!(calls[0]["result"], json!([{ "text": "result text" }]));
        assert!(calls[0]["endTime"].is_u64());
    }

    #[tokio::test]
    async fn records_error_status_and_message() {
        let provider = ToolLedgerProvider::new();
        let agent = agent_observed_by(&provider).await;

        let mut event = before(&agent, tool_use("tu-1"));
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();
        let mut event = after(&agent, ToolResultStatus::Error, Some(Error::other("boom")));
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        let calls = calls_of(&provider);
        assert_eq!(calls[0]["status"], json!("error"));
        assert_eq!(calls[0]["error"], json!("boom"));
        assert!(calls[0]["endTime"].is_u64());
    }

    #[tokio::test]
    async fn drops_oldest_entries_when_ledger_exceeds_max_entries() {
        let provider = ToolLedgerProvider::new().max_entries(2);
        let agent = agent_observed_by(&provider).await;

        for id in ["a", "b", "c"] {
            let mut event = before(&agent, tool_use(id));
            agent.hooks().invoke_callbacks(&mut event).await.unwrap();
        }

        let ids: Vec<_> = calls_of(&provider)
            .iter()
            .map(|c| c["id"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(ids, vec!["b", "c"]);
    }
}
