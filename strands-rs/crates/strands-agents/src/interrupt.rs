//! Interrupts: pausing an invocation to ask a human (`strands-ts/src/interrupt.ts`).
//!
//! An interrupt is control flow, not a failure: it travels as [`Error::Interrupt`] so every layer
//! between the raiser and the agent loop unwinds, and the loop turns it into an
//! [`AgentResult`](crate::agent::AgentResult) with
//! [`StopReason::Interrupt`](crate::types::StopReason::Interrupt). The caller answers by invoking
//! again with [`InvokeInput::InterruptResponses`](crate::agent::InvokeInput::InterruptResponses),
//! and the raiser's `interrupt(..)` call then returns the stored answer instead of raising.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::types::{
    InterruptParams, InterruptResponse, InterruptResponseContent, JsonValue, Message, ToolResultBlock,
};

/// Who raised an interrupt.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum InterruptSource {
    /// `ToolContext::interrupt()`.
    #[serde(rename = "tool")]
    Tool,
    /// An agent-level hook, e.g. `BeforeToolCallEvent::interrupt()`. Default for legacy snapshots.
    #[default]
    #[serde(rename = "hook")]
    Hook,
    /// A middleware stage.
    #[serde(rename = "middleware")]
    Middleware,
    /// A multi-agent hook.
    #[serde(rename = "multiagent-hook")]
    MultiagentHook,
}

/// A request for human input, identified by a namespaced [`id`](Interrupt::id).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Interrupt {
    /// Namespaced id; stable across resumes (see [`tool_interrupt_id`]).
    pub id: String,
    /// Raiser-chosen name, unique within one callback or tool execution.
    pub name: String,
    /// Why input is needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<JsonValue>,
    /// The answer, once resumed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<JsonValue>,
    /// Who raised it.
    #[serde(default)]
    pub source: InterruptSource,
}

impl Interrupt {
    /// An unanswered interrupt.
    pub fn new(id: impl Into<String>, name: impl Into<String>, source: InterruptSource) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            reason: None,
            response: None,
            source,
        }
    }

    /// Attach a reason.
    #[must_use]
    pub fn with_reason(mut self, reason: impl Into<JsonValue>) -> Self {
        self.reason = Some(reason.into());
        self
    }
}

/// Interrupt id for a tool raising through [`ToolContext::interrupt`](crate::tools::ToolContext::interrupt).
///
/// The id shapes are persisted in sessions, so a changed prefix silently breaks resume.
pub fn tool_interrupt_id(tool_use_id: &str, name: &str) -> String {
    format!("tool:{tool_use_id}:{name}")
}

/// Interrupt id for `BeforeToolCallEvent::interrupt`.
pub fn before_tool_call_interrupt_id(tool_use_id: &str, name: &str) -> String {
    format!("hook:beforeToolCall:{tool_use_id}:{name}")
}

/// Interrupt id for `BeforeToolsEvent::interrupt`.
pub fn before_tools_interrupt_id(name: &str) -> String {
    format!("hook:beforeTools:{name}")
}

/// Interrupt id for
/// [`BeforeNodeCallEvent::interrupt`](crate::multiagent::BeforeNodeCallEvent::interrupt).
pub fn before_node_call_interrupt_id(node_id: &str, name: &str) -> String {
    format!("multiagent-hook:beforeNodeCall:{node_id}:{name}")
}

/// A tool batch halted mid-flight: what to replay on resume.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct PendingToolExecution {
    /// The assistant message whose tool uses were being executed.
    pub assistant_message: Message,
    /// Results already produced, keyed by the **model-issued** tool use id.
    pub completed_tool_results: HashMap<String, ToolResultBlock>,
}

/// Every interrupt raised in the current invocation, plus the resume bookkeeping.
///
/// Interrupts keep insertion order: `unanswered().first()` is the oldest open one.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct InterruptState {
    interrupts: Vec<Interrupt>,
    resume_responses: Option<Vec<InterruptResponse>>,
    activated: bool,
    pending_tool_execution: Option<PendingToolExecution>,
    context: HashMap<String, JsonValue>,
}

impl InterruptState {
    /// Empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the agent is waiting for interrupt responses.
    pub fn activated(&self) -> bool {
        self.activated
    }

    /// Mark the agent as interrupted.
    pub fn activate(&mut self) {
        self.activated = true;
    }

    /// Clear everything: interrupts, responses, pending execution and context.
    pub fn deactivate(&mut self) {
        self.interrupts.clear();
        self.resume_responses = None;
        self.pending_tool_execution = None;
        self.context.clear();
        self.activated = false;
    }

    /// All interrupts, in the order they were raised.
    pub fn list(&self) -> &[Interrupt] {
        &self.interrupts
    }

    /// Interrupts still waiting for a response.
    pub fn unanswered(&self) -> Vec<Interrupt> {
        self.interrupts
            .iter()
            .filter(|i| i.response.is_none())
            .cloned()
            .collect()
    }

    /// Per-invocation scratch data (e.g. an interrupted sub-agent turn).
    pub fn context(&mut self) -> &mut HashMap<String, JsonValue> {
        &mut self.context
    }

    /// The interrupt for `id`, creating and storing it when absent.
    ///
    /// An existing entry is returned **as-is**: a fresh preemptive `response` is ignored, so a
    /// re-raised interrupt can never answer itself.
    pub fn get_or_create(
        &mut self,
        id: &str,
        params: &InterruptParams,
        source: InterruptSource,
    ) -> Interrupt {
        if let Some(existing) = self.interrupts.iter().find(|i| i.id == id) {
            return existing.clone();
        }
        let interrupt = Interrupt {
            id: id.to_owned(),
            name: params.name.clone(),
            reason: params.reason.clone(),
            response: params.response.clone(),
            source,
        };
        self.interrupts.push(interrupt.clone());
        interrupt
    }

    /// Store an interrupt raised elsewhere, **dropping** any response it carries.
    pub fn register(&mut self, interrupt: &Interrupt) -> Interrupt {
        let params = InterruptParams {
            name: interrupt.name.clone(),
            reason: interrupt.reason.clone(),
            response: None,
        };
        self.get_or_create(&interrupt.id, &params, interrupt.source)
    }

    /// Apply responses to the stored interrupts.
    ///
    /// A no-op when the agent is not interrupted (a stray response array is ignored). An unknown
    /// id is an error.
    pub fn resume(&mut self, responses: Vec<InterruptResponse>) -> Result<()> {
        if !self.activated {
            return Ok(());
        }
        for response in &responses {
            let Some(interrupt) = self.interrupts.iter_mut().find(|i| i.id == response.interrupt_id) else {
                return Err(Error::other(format!(
                    "interrupt_id=<{}> | no interrupt found",
                    response.interrupt_id
                )));
            };
            interrupt.response = Some(response.response.clone());
        }
        self.resume_responses = Some(responses);
        Ok(())
    }

    /// The tool batch to replay, if one was interrupted.
    pub fn pending_tool_execution(&self) -> Option<&PendingToolExecution> {
        self.pending_tool_execution.as_ref()
    }

    /// Record the tool batch to replay on resume.
    pub fn set_pending_tool_execution(&mut self, pending: PendingToolExecution) {
        self.pending_tool_execution = Some(pending);
    }

    /// Forget the pending tool batch.
    pub fn clear_pending_tool_execution(&mut self) {
        self.pending_tool_execution = None;
    }

    /// Wire form (`InterruptStateData`): `interrupts` is an id-keyed map.
    pub fn to_json(&self) -> JsonValue {
        let interrupts: serde_json::Map<String, JsonValue> = self
            .interrupts
            .iter()
            .map(|i| {
                (
                    i.id.clone(),
                    serde_json::to_value(i).expect("Interrupt is serializable"),
                )
            })
            .collect();
        let mut out = serde_json::Map::new();
        out.insert("interrupts".to_owned(), JsonValue::Object(interrupts));
        out.insert("activated".to_owned(), JsonValue::Bool(self.activated));
        if let Some(responses) = &self.resume_responses {
            let blocks: Vec<JsonValue> = responses
                .iter()
                .map(|r| serde_json::json!({ "interruptResponse": r }))
                .collect();
            out.insert("resumeResponses".to_owned(), JsonValue::Array(blocks));
        }
        if let Some(pending) = &self.pending_tool_execution {
            out.insert(
                "pendingToolExecution".to_owned(),
                serde_json::to_value(pending).expect("PendingToolExecution is serializable"),
            );
        }
        if !self.context.is_empty() {
            out.insert(
                "context".to_owned(),
                serde_json::to_value(&self.context).expect("context is serializable"),
            );
        }
        JsonValue::Object(out)
    }

    /// Rebuild state from the wire form produced by [`InterruptState::to_json`].
    ///
    /// # Errors
    /// Fails when `data` is not an object, or a field does not match its wire shape.
    pub fn from_json(data: &JsonValue) -> Result<Self> {
        let Some(object) = data.as_object() else {
            return Err(Error::other("interrupt state must be a JSON object"));
        };
        let mut state = Self::new();
        state.activated = object
            .get("activated")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        if let Some(interrupts) = object.get("interrupts") {
            let by_id: serde_json::Map<String, JsonValue> = serde_json::from_value(interrupts.clone())?;
            state.interrupts = by_id
                .into_iter()
                .map(|(_, interrupt)| serde_json::from_value(interrupt))
                .collect::<serde_json::Result<Vec<Interrupt>>>()?;
        }
        if let Some(responses) = object.get("resumeResponses") {
            let blocks: Vec<InterruptResponseContent> = serde_json::from_value(responses.clone())?;
            state.resume_responses = Some(blocks.into_iter().map(|b| b.interrupt_response).collect());
        }
        if let Some(pending) = object.get("pendingToolExecution") {
            state.pending_tool_execution = Some(serde_json::from_value(pending.clone())?);
        }
        if let Some(context) = object.get("context") {
            state.context = serde_json::from_value(context.clone())?;
        }
        Ok(state)
    }
}

/// Shared handle to an agent's interrupt state (the loop and raisers both hold one).
pub(crate) type SharedInterruptState = Arc<Mutex<InterruptState>>;

/// Read-only view of an agent's interrupt state, from [`Agent::interrupt_state`].
///
/// [`Agent::interrupt_state`]: crate::Agent::interrupt_state
#[derive(Clone, Debug)]
pub struct InterruptStateHandle(SharedInterruptState);

impl InterruptStateHandle {
    pub(crate) fn new(state: SharedInterruptState) -> Self {
        Self(state)
    }

    /// Whether the agent is waiting for interrupt responses.
    pub fn is_activated(&self) -> bool {
        self.0.lock().activated()
    }

    /// Every interrupt raised in the current invocation, in insertion order.
    pub fn interrupts(&self) -> Vec<Interrupt> {
        self.0.lock().list().to_vec()
    }

    /// Interrupts that are still unanswered; the oldest open one is first.
    pub fn unanswered(&self) -> Vec<Interrupt> {
        self.0.lock().unanswered()
    }

    /// The tool batch that was paused mid-execution, if any.
    pub fn pending(&self) -> Option<PendingToolExecution> {
        self.0.lock().pending_tool_execution().cloned()
    }

    /// A copy of the whole state.
    pub fn snapshot(&self) -> InterruptState {
        self.0.lock().clone()
    }
}

/// A raise closure: returns the stored response, or `Err(Error::Interrupt)` to halt.
///
/// Callers may override an agent's default closure to re-scope ids (agent-as-tool).
pub type InterruptFn = Arc<dyn Fn(InterruptParams) -> Result<JsonValue> + Send + Sync>;

/// Raise or resolve the interrupt `id`: the stored response when answered, otherwise
/// `Err(Error::Interrupt)`.
pub(crate) fn raise(
    state: &SharedInterruptState,
    id: &str,
    params: InterruptParams,
    source: InterruptSource,
) -> Result<JsonValue> {
    let interrupt = state.lock().get_or_create(id, &params, source);
    match interrupt.response.clone() {
        Some(response) => Ok(response),
        None => Err(Error::Interrupt(vec![interrupt])),
    }
}

/// A raise closure that namespaces ids with `id_prefix` (`{id_prefix}:{params.name}`).
pub fn interrupt_fn(
    state: SharedInterruptState,
    id_prefix: impl Into<String>,
    source: InterruptSource,
) -> InterruptFn {
    let id_prefix = id_prefix.into();
    Arc::new(move |params: InterruptParams| {
        let id = format!("{id_prefix}:{}", params.name);
        raise(&state, &id, params, source)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle() -> SharedInterruptState {
        Arc::new(Mutex::new(InterruptState::new()))
    }

    #[test]
    fn raise_halts_then_returns_the_response_after_resume() {
        let state = handle();
        let err = raise(
            &state,
            "tool:1:approve",
            InterruptParams::new("approve").with_reason("ok?"),
            InterruptSource::Tool,
        )
        .unwrap_err();
        let Error::Interrupt(interrupts) = &err else {
            panic!("expected interrupt, got {err:?}");
        };
        assert_eq!(interrupts.len(), 1);
        assert_eq!(err.to_string(), "Interrupt raised: approve");

        state.lock().activate();
        state
            .lock()
            .resume(vec![InterruptResponse::new(
                "tool:1:approve",
                serde_json::json!(true),
            )])
            .unwrap();
        let response = raise(
            &state,
            "tool:1:approve",
            InterruptParams::new("approve"),
            InterruptSource::Tool,
        )
        .unwrap();
        assert_eq!(response, serde_json::json!(true));
    }

    #[test]
    fn preemptive_response_does_not_halt() {
        let state = handle();
        let response = raise(
            &state,
            "tool:1:approve",
            InterruptParams::new("approve").with_response(serde_json::json!("yes")),
            InterruptSource::Tool,
        )
        .unwrap();
        assert_eq!(response, serde_json::json!("yes"));
    }

    #[test]
    fn register_drops_the_response_so_it_never_answers_itself() {
        let mut state = InterruptState::new();
        let mut answered = Interrupt::new("hook:beforeTools:x", "x", InterruptSource::Hook);
        answered.response = Some(serde_json::json!("sneaky"));
        assert_eq!(state.register(&answered).response, None);
        assert_eq!(state.unanswered().len(), 1);
    }

    #[test]
    fn resume_is_a_noop_when_not_activated_and_errors_on_unknown_ids() {
        let mut state = InterruptState::new();
        state
            .resume(vec![InterruptResponse::new("nope", serde_json::json!(1))])
            .unwrap();

        state.activate();
        let err = state
            .resume(vec![InterruptResponse::new("nope", serde_json::json!(1))])
            .unwrap_err();
        assert_eq!(err.to_string(), "interrupt_id=<nope> | no interrupt found");
    }

    #[test]
    fn deactivate_clears_everything() {
        let mut state = InterruptState::new();
        state.activate();
        state.get_or_create("a", &InterruptParams::new("a"), InterruptSource::Hook);
        state.context().insert("k".to_owned(), serde_json::json!(1));
        state.deactivate();
        assert!(!state.activated());
        assert!(state.list().is_empty());
        assert!(state.pending_tool_execution().is_none());
    }

    #[test]
    fn json_round_trip_preserves_everything_including_the_pending_batch() {
        let mut state = InterruptState::new();
        state.activate();
        state.get_or_create(
            "tool:call-1:confirm",
            &InterruptParams::new("confirm").with_reason("ok?"),
            InterruptSource::Tool,
        );
        state.get_or_create(
            "tool:call-2:confirm",
            &InterruptParams::new("confirm"),
            InterruptSource::Tool,
        );
        state
            .resume(vec![InterruptResponse::new(
                "tool:call-1:confirm",
                serde_json::json!("yes"),
            )])
            .unwrap();
        state.set_pending_tool_execution(PendingToolExecution {
            assistant_message: Message::assistant("calling"),
            completed_tool_results: HashMap::new(),
        });
        state.context().insert("k".to_owned(), serde_json::json!(1));

        let restored = InterruptState::from_json(&state.to_json()).unwrap();

        assert!(restored.activated());
        // Insertion order survives the id-keyed wire map.
        assert_eq!(
            restored.list().iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            vec!["tool:call-1:confirm", "tool:call-2:confirm"]
        );
        assert_eq!(restored.list()[0].response, Some(serde_json::json!("yes")));
        assert_eq!(restored.list()[0].source, InterruptSource::Tool);
        assert_eq!(restored.unanswered().len(), 1);
        assert_eq!(
            restored
                .pending_tool_execution()
                .unwrap()
                .assistant_message
                .text(),
            "calling"
        );
        assert_eq!(restored.to_json(), state.to_json());
    }

    #[test]
    fn from_json_defaults_an_empty_object_and_rejects_a_non_object() {
        let empty = InterruptState::from_json(&serde_json::json!({})).unwrap();
        assert!(!empty.activated());
        assert!(empty.list().is_empty());

        let err = InterruptState::from_json(&serde_json::json!("nope")).unwrap_err();
        assert_eq!(err.to_string(), "interrupt state must be a JSON object");
    }

    #[test]
    fn id_namespaces_are_verbatim() {
        assert_eq!(tool_interrupt_id("tu1", "approve"), "tool:tu1:approve");
        assert_eq!(
            before_tool_call_interrupt_id("tu1", "approve"),
            "hook:beforeToolCall:tu1:approve"
        );
        assert_eq!(before_tools_interrupt_id("approve"), "hook:beforeTools:approve");
    }

    #[test]
    fn multiple_interrupts_format_with_a_count() {
        let err = Error::Interrupt(vec![
            Interrupt::new("a", "first", InterruptSource::Hook),
            Interrupt::new("b", "second", InterruptSource::Hook),
        ]);
        assert_eq!(err.to_string(), "2 interrupts raised: first, second");
    }
}
