//! Interventions: handlers that gate an agent's lifecycle (`strands-ts/src/interventions/**`).
//!
//! An [`InterventionHandler`] inspects a lifecycle event and returns an action: proceed, deny,
//! guide, transform — or, at the tool-call gate only, [`Confirm`], which raises an interrupt and
//! asks a human. The [`InterventionRegistry`] is a [`Plugin`] that wires handlers into the hook
//! registry at [`hook_order::INTERVENTION_INPUT`] / [`hook_order::INTERVENTION_OUTPUT`], so
//! interventions run last among before-hooks and first among after-hooks.

pub mod vended;

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;

use crate::agent::Agent;
use crate::error::{Error, Result};
use crate::hooks::events::{AfterToolCallEvent, BeforeInvocationEvent, BeforeToolCallEvent, Cancel};
use crate::hooks::hook_order;
use crate::plugins::Plugin;
use crate::types::{InterruptParams, JsonValue};

/// Prefix of the cancel message a denied gate produces.
pub const DENIED_PREFIX: &str = "DENIED: ";
/// Prefix of the cancel message a guided gate produces.
pub const GUIDANCE_PREFIX: &str = "GUIDANCE: ";
/// Prefix of the cancel message a failed confirmation produces.
pub const CONFIRMATION_FAILED_PREFIX: &str = "CONFIRMATION_FAILED: ";

/// Responses that count as approval: `true`, `"y"`, `"yes"` (trimmed, case-insensitive).
pub fn default_evaluate(response: &JsonValue) -> bool {
    match response {
        JsonValue::Bool(b) => *b,
        JsonValue::String(s) => {
            matches!(s.trim().to_lowercase().as_str(), "y" | "yes")
        }
        _ => false,
    }
}

/// Decides whether a response approves the action.
pub type EvaluateFn = Arc<dyn Fn(&JsonValue) -> bool + Send + Sync>;

/// A mutation applied to the event in place.
pub type Transform<E> = Box<dyn FnOnce(&mut E) + Send>;

/// Ask a human a question, then let them approve or reject (`Confirm`).
///
/// Only legal from [`InterventionHandler::before_tool_call`]: the tool-call gate is the only place
/// an intervention may raise an interrupt.
#[non_exhaustive]
pub struct Confirm {
    /// Question shown to the human; also the interrupt's `reason`.
    pub prompt: String,
    /// Why the handler is asking.
    pub reason: Option<String>,
    /// Preemptive answer: the interrupt is recorded but never pauses the agent.
    pub response: Option<JsonValue>,
    /// Approval test; [`default_evaluate`] when `None`.
    pub evaluate: Option<EvaluateFn>,
}

impl Confirm {
    /// A confirmation request with the default approval test.
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            reason: None,
            response: None,
            evaluate: None,
        }
    }

    /// Answer the question up front (no pause).
    #[must_use]
    pub fn with_response(mut self, response: impl Into<JsonValue>) -> Self {
        self.response = Some(response.into());
        self
    }

    /// Use a custom approval test.
    #[must_use]
    pub fn with_evaluate(mut self, evaluate: EvaluateFn) -> Self {
        self.evaluate = Some(evaluate);
        self
    }

    fn approves(&self, response: &JsonValue) -> bool {
        match &self.evaluate {
            Some(evaluate) => evaluate(response),
            None => default_evaluate(response),
        }
    }
}

/// What a handler decides at a gate that cannot raise interrupts.
#[non_exhaustive]
pub enum Action<E> {
    /// Let the event through.
    Proceed,
    /// Block it; the reason becomes the cancel message.
    Deny(String),
    /// Let it through but feed the model this feedback.
    Guide(String),
    /// Mutate the event.
    Transform(Transform<E>),
}

/// What a handler decides at the tool-call gate, the one gate that may raise an interrupt.
#[non_exhaustive]
pub enum ToolCallAction {
    /// Let the tool run.
    Proceed,
    /// Block the tool; the reason becomes the error result.
    Deny(String),
    /// Let it run but feed the model this feedback.
    Guide(String),
    /// Ask a human first (raises an interrupt unless the answer is already known).
    Confirm(Confirm),
    /// Mutate the tool call.
    Transform(Transform<BeforeToolCallEvent>),
}

/// A gate on the agent's lifecycle. Every method defaults to proceeding.
#[async_trait]
pub trait InterventionHandler: Send + Sync {
    /// Unique handler name; also the interrupt name a [`Confirm`] raises under.
    fn name(&self) -> &str;

    /// Gate the whole invocation.
    async fn before_invocation(
        &self,
        _event: &BeforeInvocationEvent,
    ) -> Result<Action<BeforeInvocationEvent>> {
        Ok(Action::Proceed)
    }

    /// Gate one tool call.
    async fn before_tool_call(&self, _event: &BeforeToolCallEvent) -> Result<ToolCallAction> {
        Ok(ToolCallAction::Proceed)
    }

    /// Observe (and optionally rewrite) a tool result.
    async fn after_tool_call(&self, _event: &AfterToolCallEvent) -> Result<Action<AfterToolCallEvent>> {
        Ok(Action::Proceed)
    }
}

/// Runs a set of [`InterventionHandler`]s as hooks, in registration order per gate.
pub struct InterventionRegistry {
    handlers: Vec<Arc<dyn InterventionHandler>>,
}

impl InterventionRegistry {
    /// A registry over `handlers`; duplicate names are rejected.
    pub fn new(handlers: impl IntoIterator<Item = Arc<dyn InterventionHandler>>) -> Result<Self> {
        let handlers: Vec<_> = handlers.into_iter().collect();
        let mut seen = HashSet::new();
        for handler in &handlers {
            if !seen.insert(handler.name().to_owned()) {
                return Err(Error::Plugin(format!(
                    "Duplicate intervention handler name: '{}'",
                    handler.name()
                )));
            }
        }
        Ok(Self { handlers })
    }
}

/// Guides are accumulated across handlers and applied once, tagged with the handler name.
fn merge_guides(guides: &[(String, String)]) -> String {
    guides
        .iter()
        .map(|(handler, feedback)| format!("[{handler}] {feedback}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[async_trait]
impl Plugin for InterventionRegistry {
    fn name(&self) -> &str {
        "strands:interventions"
    }

    async fn init_agent(&self, agent: &Agent) -> Result<()> {
        let handlers = self.handlers.clone();
        let _ = agent.hooks().add_callback_with_order::<BeforeInvocationEvent>(
            hook_order::INTERVENTION_INPUT,
            Arc::new(move |event: &mut BeforeInvocationEvent| {
                let handlers = handlers.clone();
                Box::pin(async move {
                    let mut guides = Vec::new();
                    for handler in &handlers {
                        match handler.before_invocation(event).await? {
                            Action::Proceed => {}
                            Action::Deny(reason) => {
                                event.cancel = Cancel::Reason(format!("{DENIED_PREFIX}{reason}"));
                                return Ok(());
                            }
                            Action::Guide(feedback) => guides.push((handler.name().to_owned(), feedback)),
                            Action::Transform(apply) => apply(event),
                        }
                    }
                    if !guides.is_empty() {
                        event.cancel = Cancel::Reason(format!("{GUIDANCE_PREFIX}{}", merge_guides(&guides)));
                    }
                    Ok(())
                })
            }),
        );

        let handlers = self.handlers.clone();
        let _ = agent.hooks().add_callback_with_order::<BeforeToolCallEvent>(
            hook_order::INTERVENTION_INPUT,
            Arc::new(move |event: &mut BeforeToolCallEvent| {
                let handlers = handlers.clone();
                Box::pin(async move {
                    let mut guides = Vec::new();
                    for handler in &handlers {
                        match handler.before_tool_call(event).await? {
                            ToolCallAction::Proceed => {}
                            ToolCallAction::Deny(reason) => {
                                event.cancel = Cancel::Reason(format!("{DENIED_PREFIX}{reason}"));
                                return Ok(());
                            }
                            ToolCallAction::Guide(feedback) => {
                                guides.push((handler.name().to_owned(), feedback))
                            }
                            ToolCallAction::Transform(apply) => apply(event),
                            ToolCallAction::Confirm(confirm) => {
                                let mut params =
                                    InterruptParams::new(handler.name()).with_reason(confirm.prompt.clone());
                                params.response = confirm.response.clone();
                                // Propagates `Error::Interrupt` on the first pass; on resume the
                                // human's answer comes back here.
                                let response: JsonValue = event.interrupt(params)?;
                                if !confirm.approves(&response) {
                                    event.cancel = Cancel::Reason(format!(
                                        "{CONFIRMATION_FAILED_PREFIX}{}",
                                        confirm.prompt
                                    ));
                                    return Ok(());
                                }
                            }
                        }
                    }
                    if !guides.is_empty() {
                        event.cancel = Cancel::Reason(format!("{GUIDANCE_PREFIX}{}", merge_guides(&guides)));
                    }
                    Ok(())
                })
            }),
        );

        let handlers = self.handlers.clone();
        let _ = agent.hooks().add_callback_with_order::<AfterToolCallEvent>(
            hook_order::INTERVENTION_OUTPUT,
            Arc::new(move |event: &mut AfterToolCallEvent| {
                let handlers = handlers.clone();
                Box::pin(async move {
                    for handler in &handlers {
                        // Only `transform` has an effect after a tool call.
                        if let Action::Transform(apply) = handler.after_tool_call(event).await? {
                            apply(event);
                        }
                    }
                    Ok(())
                })
            }),
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Named(&'static str);

    #[async_trait]
    impl InterventionHandler for Named {
        fn name(&self) -> &str {
            self.0
        }
    }

    #[test]
    fn duplicate_handler_names_are_rejected() {
        let handlers: Vec<Arc<dyn InterventionHandler>> = vec![Arc::new(Named("a")), Arc::new(Named("a"))];
        let err = match InterventionRegistry::new(handlers) {
            Err(err) => err,
            Ok(_) => panic!("expected a duplicate-name error"),
        };
        assert_eq!(err.to_string(), "Duplicate intervention handler name: 'a'");
    }

    #[test]
    fn default_evaluate_accepts_true_y_and_yes_only() {
        for approved in [
            serde_json::json!(true),
            serde_json::json!("y"),
            serde_json::json!("YES"),
            serde_json::json!(" yes "),
        ] {
            assert!(default_evaluate(&approved), "{approved} should approve");
        }
        for rejected in [
            serde_json::json!(false),
            serde_json::json!("n"),
            serde_json::json!("sure"),
            serde_json::json!(1),
        ] {
            assert!(!default_evaluate(&rejected), "{rejected} should reject");
        }
    }

    #[test]
    fn guides_are_merged_with_handler_names() {
        let merged = merge_guides(&[
            ("a".to_owned(), "first".to_owned()),
            ("b".to_owned(), "second".to_owned()),
        ]);
        assert_eq!(merged, "[a] first\n[b] second");
    }
}
