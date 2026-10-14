//! Continuation inputs: messages queued mid-invocation and prepended to the next model input
//! (`strands-ts/src/agent/continuation.ts`).

use parking_lot::Mutex;

use crate::types::{ContentBlock, Message, Role};

/// Callback invoked once a continuation input is incorporated into the conversation.
type AppendedFn = Box<dyn FnOnce() + Send + 'static>;
/// Callback invoked once a continuation input is dropped without being incorporated.
type AbandonedFn = Box<dyn FnOnce(String) + Send + 'static>;

/// One queued continuation: the messages plus optional outcome callbacks.
///
/// When both callbacks are supplied exactly one of them runs, exactly once.
#[must_use]
pub struct ContinuationInput {
    messages: Vec<Message>,
    on_appended: Option<AppendedFn>,
    on_abandoned: Option<AbandonedFn>,
}

impl ContinuationInput {
    /// Queue `messages`; they must form a complete message sequence
    /// (see [`is_complete_message_input`]).
    pub fn new(messages: Vec<Message>) -> Self {
        Self {
            messages,
            on_appended: None,
            on_abandoned: None,
        }
    }

    /// Run `f` after the messages are appended to the conversation.
    pub fn on_appended(mut self, f: impl FnOnce() + Send + 'static) -> Self {
        self.on_appended = Some(Box::new(f));
        self
    }

    /// Run `f` with a reason when the input is abandoned instead of appended.
    pub fn on_abandoned(mut self, f: impl FnOnce(String) + Send + 'static) -> Self {
        self.on_abandoned = Some(Box::new(f));
        self
    }

    /// The queued messages.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    fn notify_appended(self) {
        if let Some(f) = self.on_appended {
            f();
        }
    }

    fn notify_abandoned(self, reason: &str) {
        if let Some(f) = self.on_abandoned {
            f(reason.to_owned());
        }
    }
}

impl std::fmt::Debug for ContinuationInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContinuationInput")
            .field("messages", &self.messages)
            .finish_non_exhaustive()
    }
}

/// Queue of continuation inputs for one agent (TS `continuations`).
///
/// [`Continuations::prepare`] hands out the messages to prepend and holds the inputs until either
/// [`Continuations::mark_appended`] or [`Continuations::abandon`] resolves them.
#[derive(Default)]
pub struct Continuations {
    queued: Mutex<Vec<ContinuationInput>>,
    prepared: Mutex<Vec<ContinuationInput>>,
}

impl Continuations {
    /// Queue an input for the next invocation.
    pub fn add_input(&self, input: ContinuationInput) {
        self.queued.lock().push(input);
    }

    /// Whether anything is queued.
    pub fn has_input(&self) -> bool {
        !self.queued.lock().is_empty()
    }

    /// Take the queued inputs and return the messages to prepend, dropping (and abandoning) any
    /// input that is not a complete message sequence. `None` when nothing valid is queued.
    pub fn prepare(&self) -> Option<Vec<Message>> {
        let inputs = std::mem::take(&mut *self.queued.lock());
        let mut messages = Vec::new();
        let mut accepted = Vec::new();
        for input in inputs {
            if is_complete_message_input(input.messages()) {
                messages.extend(input.messages().iter().cloned());
                accepted.push(input);
            } else {
                tracing::warn!("continuation input must contain a complete message sequence");
                input.notify_abandoned("Continuation input must contain a complete message sequence");
            }
        }
        if accepted.is_empty() {
            return None;
        }
        self.prepared.lock().extend(accepted);
        Some(messages)
    }

    /// Resolve the prepared inputs as appended, running their `on_appended` callbacks.
    pub fn mark_appended(&self) {
        for input in std::mem::take(&mut *self.prepared.lock()) {
            input.notify_appended();
        }
    }

    /// Resolve every prepared *and* queued input as abandoned with `reason`.
    pub fn abandon(&self, reason: &str) {
        let inputs = std::mem::take(&mut *self.prepared.lock());
        let queued = std::mem::take(&mut *self.queued.lock());
        for input in inputs.into_iter().chain(queued) {
            input.notify_abandoned(reason);
        }
    }
}

impl std::fmt::Debug for Continuations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Continuations")
            .field("queued", &self.queued.lock().len())
            .field("prepared", &self.prepared.lock().len())
            .finish()
    }
}

/// Whether `messages` form a self-contained conversation suffix: it ends on a user turn and every
/// tool use is answered by a tool result in the next message (TS `isCompleteMessageInput`).
pub fn is_complete_message_input(messages: &[Message]) -> bool {
    if messages.last().map(|m| m.role) != Some(Role::User) {
        return false;
    }
    let mut pending: Vec<String> = Vec::new();
    for message in messages {
        if !pending.is_empty() && message.role != Role::User {
            return false;
        }
        let mut next_pending: Vec<String> = Vec::new();
        for block in &message.content {
            match block {
                ContentBlock::ToolUse(tool_use) => {
                    if message.role != Role::Assistant || next_pending.contains(&tool_use.tool_use_id) {
                        return false;
                    }
                    next_pending.push(tool_use.tool_use_id.clone());
                }
                ContentBlock::ToolResult(result) => {
                    if message.role != Role::User {
                        return false;
                    }
                    match pending.iter().position(|id| *id == result.tool_use_id) {
                        Some(index) => {
                            pending.remove(index);
                        }
                        None => return false,
                    }
                }
                _ => {}
            }
        }
        if message.role == Role::User && !pending.is_empty() {
            return false;
        }
        pending = next_pending;
    }
    pending.is_empty()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::*;
    use crate::types::{ToolResultBlock, ToolResultContent, ToolUseBlock};

    fn tool_use_message(id: &str) -> Message {
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse(ToolUseBlock::new(
                "t",
                id,
                serde_json::json!({}),
            ))],
        )
    }

    fn tool_result_message(id: &str) -> Message {
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult(ToolResultBlock::success(
                id,
                vec![ToolResultContent::Text("ok".to_owned())],
            ))],
        )
    }

    #[test]
    fn complete_input_requires_a_trailing_user_turn() {
        assert!(is_complete_message_input(&[Message::user("hi")]));
        assert!(!is_complete_message_input(&[]));
        assert!(!is_complete_message_input(&[Message::assistant("hi")]));
    }

    #[test]
    fn complete_input_pairs_tool_uses_with_results() {
        assert!(is_complete_message_input(&[
            tool_use_message("a"),
            tool_result_message("a"),
        ]));
        // unanswered tool use
        assert!(!is_complete_message_input(&[
            tool_use_message("a"),
            Message::user("hi")
        ]));
        // result without an outstanding use
        assert!(!is_complete_message_input(&[tool_result_message("a")]));
    }

    #[test]
    fn prepare_then_mark_appended_runs_the_callback_once() {
        let continuations = Continuations::default();
        let appended = Arc::new(AtomicUsize::new(0));
        let flag = appended.clone();
        continuations.add_input(
            ContinuationInput::new(vec![Message::user("more")]).on_appended(move || {
                flag.fetch_add(1, Ordering::SeqCst);
            }),
        );

        let messages = continuations.prepare().expect("prepared");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text(), "more");
        continuations.mark_appended();
        continuations.mark_appended();
        assert_eq!(appended.load(Ordering::SeqCst), 1);
        assert!(continuations.prepare().is_none());
    }

    #[test]
    fn invalid_input_is_abandoned_and_never_prepared() {
        let continuations = Continuations::default();
        let reason = Arc::new(Mutex::new(None));
        let sink = reason.clone();
        continuations.add_input(
            ContinuationInput::new(vec![Message::assistant("not a user turn")])
                .on_abandoned(move |r| *sink.lock() = Some(r)),
        );

        assert!(continuations.prepare().is_none());
        assert_eq!(
            reason.lock().as_deref(),
            Some("Continuation input must contain a complete message sequence")
        );
    }

    #[test]
    fn abandon_resolves_queued_and_prepared_inputs() {
        let continuations = Continuations::default();
        let abandoned = Arc::new(AtomicUsize::new(0));
        for _ in 0..2 {
            let flag = abandoned.clone();
            continuations.add_input(ContinuationInput::new(vec![Message::user("x")]).on_abandoned(
                move |_| {
                    flag.fetch_add(1, Ordering::SeqCst);
                },
            ));
        }
        continuations.prepare().expect("prepared");
        let flag = abandoned.clone();
        continuations.add_input(
            ContinuationInput::new(vec![Message::user("y")]).on_abandoned(move |_| {
                flag.fetch_add(1, Ordering::SeqCst);
            }),
        );

        continuations.abandon("cancelled");
        assert_eq!(abandoned.load(Ordering::SeqCst), 3);
    }
}
