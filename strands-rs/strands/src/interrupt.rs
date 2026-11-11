//! Interrupts: pausing the agent for a caller's answer (TS `interrupt.ts`).

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Map;

use crate::error::{Result, SessionError};
use crate::types::interrupt::{InterruptParams, InterruptResponseContent};
use crate::types::json::{JsonValue, null_as_default, present};
use crate::types::messages::{ContentBlock, Message, Role, ToolResultBlock};
use crate::types::open_enum::open_enum;

open_enum! {
    /// What raised an interrupt (TS `InterruptSource`). Defaults to `Hook`, as in TS.
    #[derive(Default)]
    pub enum InterruptSource / UnknownInterruptSource {
        /// A tool, through its context.
        Tool = "tool",
        /// A hook callback.
        #[default]
        Hook = "hook",
        /// Middleware.
        Middleware = "middleware",
        /// A multi-agent orchestrator hook.
        MultiagentHook = "multiagent-hook",
    }
}

/// A request for a caller's answer that pauses the agent (TS `Interrupt`).
///
/// A present `null` response counts as an answer; only a missing one leaves the interrupt open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Interrupt {
    /// Unique id, for example `tool:<toolUseId>:<name>`.
    pub id: String,
    /// The name given when it was raised.
    pub name: String,
    /// Why it was raised.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub reason: Option<JsonValue>,
    /// The caller's answer, once given.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub response: Option<JsonValue>,
    /// What raised it.
    #[serde(default, deserialize_with = "null_as_default")]
    pub source: InterruptSource,
}

impl Interrupt {
    /// An unanswered interrupt raised by a hook.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            reason: None,
            response: None,
            source: InterruptSource::Hook,
        }
    }

    /// Sets the reason.
    #[must_use]
    pub fn with_reason(mut self, reason: JsonValue) -> Self {
        self.reason = Some(reason);
        self
    }

    /// Sets the response.
    #[must_use]
    pub fn with_response(mut self, response: JsonValue) -> Self {
        self.response = Some(response);
        self
    }

    /// Sets the source.
    #[must_use]
    pub fn with_source(mut self, source: InterruptSource) -> Self {
        self.source = source;
        self
    }
}

/// Tool calls paused by an interrupt, kept as persisted so a newer SDK's data survives a save.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingToolExecution {
    pub(crate) assistant_message_data: JsonValue,
    pub(crate) completed_tool_results: JsonValue,
}

/// A [`PendingToolExecution`] parsed for resuming.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PendingExecution {
    pub(crate) assistant_message: Message,
    pub(crate) completed_tool_results: IndexMap<String, ToolResultBlock>,
}

/// The agent's interrupt bookkeeping, persisted in snapshots (TS `InterruptState`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "used by the agent loop and snapshots; Rust 1.96 doesn't flag the struct, so `expect` would fail there"
    )
)]
pub(crate) struct InterruptState {
    pub(crate) interrupts: IndexMap<String, Interrupt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) resume_responses: Option<Vec<InterruptResponseContent>>,
    pub(crate) activated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) pending_tool_execution: Option<PendingToolExecution>,
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub(crate) context: Map<String, JsonValue>,
}

#[cfg_attr(not(test), expect(dead_code, reason = "used by the agent loop and snapshots"))]
impl InterruptState {
    /// The pending execution, parsed, when activated.
    pub(crate) fn get_pending_execution(&self) -> Result<Option<PendingExecution>> {
        let Some(pending) = self.pending_tool_execution.as_ref().filter(|_| self.activated) else {
            return Ok(None);
        };
        let assistant_message = Message::deserialize(&pending.assistant_message_data)?;
        let completed_tool_results =
            IndexMap::<String, ToolResultWrapper>::deserialize(&pending.completed_tool_results)?
                .into_iter()
                .map(|(id, wrapper)| (id, wrapper.tool_result))
                .collect();
        Ok(Some(PendingExecution {
            assistant_message,
            completed_tool_results,
        }))
    }

    pub(crate) fn set_pending_tool_execution(&mut self, pending: PendingToolExecution) {
        self.pending_tool_execution = Some(pending);
    }

    pub(crate) fn clear_pending_tool_execution(&mut self) {
        self.pending_tool_execution = None;
    }

    pub(crate) fn get_interrupts_list(&self) -> Vec<Interrupt> {
        self.interrupts.values().cloned().collect()
    }

    pub(crate) fn get_unanswered_interrupts(&self) -> Vec<Interrupt> {
        self.interrupts
            .values()
            .filter(|i| i.response.is_none())
            .cloned()
            .collect()
    }

    pub(crate) fn get_unanswered_interrupt(&self) -> Option<&Interrupt> {
        self.interrupts.values().find(|i| i.response.is_none())
    }

    pub(crate) fn activate(&mut self) {
        self.activated = true;
    }

    pub(crate) fn deactivate(&mut self) {
        *self = Self::default();
    }

    /// Records the caller's answers; does nothing unless activated.
    ///
    /// Fails with the TS text when an answer names an unknown interrupt.
    pub(crate) fn resume(
        &mut self,
        responses: Vec<InterruptResponseContent>,
    ) -> std::result::Result<(), String> {
        if !self.activated {
            return Ok(());
        }
        for content in &responses {
            let id = &content.interrupt_response.interrupt_id;
            let interrupt = self
                .interrupts
                .get_mut(id)
                .ok_or_else(|| format!("interrupt_id=<{id}> | no interrupt found"))?;
            interrupt.response = Some(content.interrupt_response.response.clone());
        }
        self.resume_responses = Some(responses);
        Ok(())
    }

    /// The interrupt with `id`, created from the arguments if it doesn't exist yet.
    pub(crate) fn get_or_create_interrupt(
        &mut self,
        id: &str,
        name: &str,
        reason: Option<JsonValue>,
        response: Option<JsonValue>,
        source: Option<InterruptSource>,
    ) -> &mut Interrupt {
        self.interrupts.entry(id.to_owned()).or_insert_with(|| Interrupt {
            id: id.to_owned(),
            name: name.to_owned(),
            reason,
            response,
            source: source.unwrap_or_default(),
        })
    }

    pub(crate) fn register_interrupt(&mut self, interrupt: &Interrupt) -> &mut Interrupt {
        self.get_or_create_interrupt(
            &interrupt.id,
            &interrupt.name,
            interrupt.reason.clone(),
            None,
            Some(interrupt.source.clone()),
        )
    }

    /// Raises interrupt `id`: its response when answered (or answered up front), otherwise the
    /// interrupt to pause on (TS `interruptFromAgent`).
    pub(crate) fn interrupt(
        &mut self,
        id: &str,
        params: InterruptParams,
        source: InterruptSource,
    ) -> std::result::Result<JsonValue, Box<Interrupt>> {
        let interrupt =
            self.get_or_create_interrupt(id, &params.name, params.reason, params.response, Some(source));
        interrupt
            .response
            .clone()
            .ok_or_else(|| Box::new(interrupt.clone()))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolResultWrapper {
    tool_result: ToolResultBlock,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InterruptStateData {
    interrupts: IndexMap<String, Interrupt>,
    #[serde(default)]
    resume_responses: Option<Vec<InterruptResponseContent>>,
    activated: bool,
    #[serde(default)]
    pending_tool_execution: Option<JsonValue>,
    #[serde(default)]
    context: Option<Map<String, JsonValue>>,
}

impl InterruptState {
    /// Restores persisted state, validating the pending tool execution (TS `InterruptState.fromJSON`).
    ///
    /// Invalid pending data fails with a [`SessionError`] naming the field.
    #[cfg_attr(not(test), expect(dead_code, reason = "used by snapshots"))]
    pub(crate) fn from_json(data: JsonValue) -> Result<Self> {
        let data = InterruptStateData::deserialize(data)?;
        let pending_tool_execution = match data.pending_tool_execution {
            Some(pending) if data.activated && is_truthy(&pending) => {
                Some(validate_pending_tool_execution(pending_from_value(pending))?)
            }
            _ => None,
        };
        Ok(Self {
            interrupts: data.interrupts,
            resume_responses: data.resume_responses,
            activated: data.activated,
            pending_tool_execution,
            context: data.context.unwrap_or_default(),
        })
    }
}

/// JavaScript truthiness, for TS's `if (data.pendingToolExecution)`.
fn is_truthy(value: &JsonValue) -> bool {
    match value {
        JsonValue::Null => false,
        JsonValue::Bool(b) => *b,
        JsonValue::Number(n) => n.as_f64().is_some_and(|n| n != 0.0),
        JsonValue::String(s) => !s.is_empty(),
        JsonValue::Array(_) | JsonValue::Object(_) => true,
    }
}

/// Reads the two pending fields, with `null` for a missing one so validation reports it with the TS text.
fn pending_from_value(value: JsonValue) -> PendingToolExecution {
    let mut fields = match value {
        JsonValue::Object(map) => map,
        _ => Map::new(),
    };
    PendingToolExecution {
        assistant_message_data: fields.remove("assistantMessageData").unwrap_or_default(),
        completed_tool_results: fields.remove("completedToolResults").unwrap_or_default(),
    }
}

fn restore_error(field: &str, source: serde_json::Error) -> SessionError {
    SessionError::new(format!(
        "{field} | failed to parse restored pending tool execution"
    ))
    .with_source(source)
}

fn validate_pending_tool_execution(
    pending: PendingToolExecution,
) -> std::result::Result<PendingToolExecution, SessionError> {
    let message = Message::deserialize(&pending.assistant_message_data)
        .map_err(|e| restore_error("field=<pendingToolExecution.assistantMessageData>", e))?;
    if message.role != Role::Assistant {
        return Err(SessionError::new(
            "field=<pendingToolExecution.assistantMessageData.role> | pending tool execution message must be an assistant message",
        ));
    }
    let tool_use_ids: Vec<&str> = message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse(tool_use) => Some(tool_use.tool_use_id.as_str()),
            _ => None,
        })
        .collect();
    if tool_use_ids.is_empty() {
        return Err(SessionError::new(
            "field=<pendingToolExecution.assistantMessageData.content> | pending tool execution message has no tool uses",
        ));
    }
    let Some(results) = pending.completed_tool_results.as_object() else {
        return Err(SessionError::new(
            "field=<pendingToolExecution.completedToolResults> | completed tool results must be an object",
        ));
    };
    for (tool_use_id, result) in results {
        let field = format!("field=<pendingToolExecution.completedToolResults>, tool_use_id=<{tool_use_id}>");
        if !tool_use_ids.contains(&tool_use_id.as_str()) {
            return Err(SessionError::new(format!(
                "{field} | does not match a tool use in the pending assistant message"
            )));
        }
        ToolResultWrapper::deserialize(result).map_err(|e| restore_error(&field, e))?;
    }
    Ok(pending)
}

#[cfg(test)]
mod tests;
