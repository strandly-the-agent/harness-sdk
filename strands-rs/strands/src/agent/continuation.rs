//! Follow-up input that hooks contribute to an invocation (TS `agent/continuation.ts`).
//!
//! An `AfterInvocationEvent` input runs another pass with the input in front of any resume input;
//! a `BeforeModelCallEvent` input is appended before that model call. Each input's callbacks run
//! once: `on_appended` when it reached history, `on_abandoned` otherwise (also when it is dropped).

use std::fmt;

use crate::error::Result;
use crate::types::agent::InvokeArgs;
use crate::types::messages::{ContentBlock, Message, Role, StopReason};

/// TS text when a stream closes before the input reached history.
pub(crate) const STREAM_CLOSED: &str =
    "Agent stream closed before continuation input was incorporated into agent history";
/// TS text when a pass ran without incorporating the input.
pub(crate) const NOT_INCORPORATED: &str = "Continuation was not incorporated into agent history";
/// TS text when a `BeforeModelCallEvent` cancel drops the input.
pub(crate) const CANCELLED_BY_MODEL_HOOK: &str = "Continuation abandoned by BeforeModelCallEvent";
const INCOMPLETE: &str = "Continuation input must contain a complete message sequence";

type Callback = Box<dyn FnOnce() + Send + Sync>;
type AbandonCallback = Box<dyn FnOnce(&str) + Send + Sync>;

/// One input contribution (TS `ContinuationInput`).
pub(crate) struct ContinuationInput {
    args: Option<InvokeArgs>,
    on_appended: Option<Callback>,
    on_abandoned: Option<AbandonCallback>,
}

impl ContinuationInput {
    pub(crate) fn new(args: impl Into<InvokeArgs>) -> Self {
        Self {
            args: Some(args.into()),
            on_appended: None,
            on_abandoned: None,
        }
    }

    pub(crate) fn on_appended(mut self, callback: impl FnOnce() + Send + Sync + 'static) -> Self {
        self.on_appended = Some(Box::new(callback));
        self
    }

    #[cfg(test)]
    pub(crate) fn on_abandoned(mut self, callback: impl FnOnce(&str) + Send + Sync + 'static) -> Self {
        self.on_abandoned = Some(Box::new(callback));
        self
    }

    fn appended(mut self) {
        self.on_abandoned = None;
        if let Some(callback) = self.on_appended.take() {
            callback();
        }
    }

    fn abandon(mut self, reason: &str) {
        self.on_appended = None;
        if let Some(callback) = self.on_abandoned.take() {
            callback(reason);
        }
    }
}

impl Drop for ContinuationInput {
    fn drop(&mut self) {
        self.on_appended = None;
        if let Some(callback) = self.on_abandoned.take() {
            callback(STREAM_CLOSED);
        }
    }
}

impl fmt::Debug for ContinuationInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContinuationInput")
            .field("args", &self.args)
            .finish_non_exhaustive()
    }
}

/// Inputs whose messages form complete sequences, ready to be appended.
#[derive(Debug)]
pub(crate) struct Prepared {
    inputs: Vec<ContinuationInput>,
    pub(crate) messages: Vec<Message>,
}

impl Prepared {
    /// Runs the `on_appended` callbacks (TS `markAppended`).
    pub(crate) fn mark_appended(self) {
        for input in self.inputs {
            input.appended();
        }
    }

    /// Runs the `on_abandoned` callbacks.
    pub(crate) fn abandon(self, reason: &str) {
        abandon(self.inputs, reason);
    }
}

/// Runs the `on_abandoned` callbacks of `inputs`.
pub(crate) fn abandon(inputs: Vec<ContinuationInput>, reason: &str) {
    for input in inputs {
        input.abandon(reason);
    }
}

/// Normalizes the inputs into messages (TS `prepare`). `stop_reason` is the pass's stop reason
/// for `AfterInvocationEvent` inputs: an interrupt defers them to the next pass that finishes,
/// any other stop than `endTurn` or `stopSequence` prepares nothing.
pub(crate) fn prepare(
    inputs: Vec<ContinuationInput>,
    deferred: Option<&mut Vec<ContinuationInput>>,
    stop_reason: Option<&StopReason>,
    normalize: impl Fn(InvokeArgs) -> Result<Vec<Message>>,
) -> Option<Prepared> {
    if stop_reason == Some(&StopReason::Interrupt) {
        if let Some(deferred) = deferred {
            deferred.extend(inputs);
        }
        return None;
    }
    if stop_reason.is_some_and(|reason| !matches!(reason, StopReason::EndTurn | StopReason::StopSequence)) {
        return None;
    }
    let mut all = deferred.map(std::mem::take).unwrap_or_default();
    all.extend(inputs);
    let mut accepted = Vec::new();
    let mut messages = Vec::new();
    for mut input in all {
        let normalized = input.args.take().map(&normalize).transpose();
        match normalized {
            Ok(Some(normalized)) if is_complete_message_input(&normalized) => {
                messages.extend(normalized);
                accepted.push(input);
            }
            Ok(_) => input.abandon(INCOMPLETE),
            Err(error) => input.abandon(&error.to_string()),
        }
    }
    (!accepted.is_empty()).then_some(Prepared {
        inputs: accepted,
        messages,
    })
}

/// The TS abandon text for a pass that stopped with `stop_reason`.
pub(crate) fn stopped_after(stop_reason: &StopReason) -> String {
    format!("Continuation abandoned after {}", stop_reason.as_str())
}

/// Whether `args` is an empty list (TS `Array.isArray(args) && args.length === 0`).
pub(crate) fn is_empty_list(args: &InvokeArgs) -> bool {
    match args {
        InvokeArgs::Content(content) => content.is_empty(),
        InvokeArgs::Messages(messages) => messages.is_empty(),
        InvokeArgs::InterruptResponses(responses) => responses.is_empty(),
        _ => false,
    }
}

/// Messages ending on a user turn, with every tool use answered in the next user message.
fn is_complete_message_input(messages: &[Message]) -> bool {
    if messages.last().is_none_or(|last| last.role() != Role::User) {
        return false;
    }
    let mut pending: Vec<&str> = Vec::new();
    for message in messages {
        if !pending.is_empty() && message.role() != Role::User {
            return false;
        }
        let mut next: Vec<&str> = Vec::new();
        for block in message.content() {
            match block {
                ContentBlock::ToolUse(tool_use) => {
                    if message.role() != Role::Assistant || next.contains(&tool_use.tool_use_id()) {
                        return false;
                    }
                    next.push(tool_use.tool_use_id());
                }
                ContentBlock::ToolResult(result) => {
                    let Some(index) = pending.iter().position(|id| *id == result.tool_use_id()) else {
                        return false;
                    };
                    if message.role() != Role::User {
                        return false;
                    }
                    pending.swap_remove(index);
                }
                _ => {}
            }
        }
        if message.role() == Role::User && !pending.is_empty() {
            return false;
        }
        pending = next;
    }
    pending.is_empty()
}
