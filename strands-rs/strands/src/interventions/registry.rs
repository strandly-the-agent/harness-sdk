//! Bridges intervention handlers and the hook system (TS `interventions/registry.ts`).

use std::fmt;
use std::sync::Arc;

use super::actions::{
    AfterModelCallAction, AfterToolCallAction, BeforeInvocationAction, BeforeModelCallAction,
    BeforeToolCallAction, Guide, default_evaluate,
};
use super::handler::{InterventionHandler, OnError};
use crate::error::{Error, Result};
use crate::hooks::{
    AfterModelCallEvent, AfterToolCallEvent, BeforeInvocationEvent, BeforeModelCallEvent,
    BeforeToolCallEvent, HookCallbackOptions, HookOrder, HookRegistry,
};
use crate::interrupt::InterruptParams;
use crate::types::json::JsonValue;
use crate::types::messages::{ContentBlock, Message, Role, TextBlock};

/// What applying one action means for the rest of the dispatch.
enum Outcome {
    /// Later handlers still run.
    Continue,
    /// The action short-circuits: later handlers don't run and accumulated guidance is dropped.
    Stop,
    /// Guidance to apply once every handler has run.
    Guide(Guide),
}

/// Runs the registered [`InterventionHandler`]s on the agent's lifecycle events (TS
/// `InterventionRegistry`).
///
/// Handlers run in registration order. A deny, or a confirm the human declines, short-circuits;
/// guidance accumulates across handlers and is applied at the end; a transform is applied at
/// once, so later handlers see the change.
#[derive(Clone)]
pub(crate) struct InterventionRegistry {
    handlers: Arc<[Arc<dyn InterventionHandler>]>,
}

impl InterventionRegistry {
    /// Registers `handlers` on `hooks`: before-events at [`HookOrder::INTERVENTION_INPUT`],
    /// after-events at [`HookOrder::INTERVENTION_OUTPUT`].
    ///
    /// # Errors
    ///
    /// Two handlers share a name.
    pub(crate) fn new(handlers: Vec<Arc<dyn InterventionHandler>>, hooks: &HookRegistry) -> Result<Self> {
        let mut seen: Vec<&str> = Vec::new();
        for handler in &handlers {
            let name = handler.name();
            if seen.contains(&name) {
                return Err(Error::other(format!(
                    "Duplicate intervention handler name: '{name}'"
                )));
            }
            seen.push(name);
        }
        let registry = Self {
            handlers: handlers.into(),
        };
        if !registry.handlers.is_empty() {
            registry.register_hooks(hooks);
        }
        Ok(registry)
    }

    /// The handlers in registration order.
    pub(crate) fn handlers(&self) -> &[Arc<dyn InterventionHandler>] {
        &self.handlers
    }

    fn register_hooks(&self, hooks: &HookRegistry) {
        let input = HookCallbackOptions {
            order: HookOrder::INTERVENTION_INPUT,
        };
        let output = HookCallbackOptions {
            order: HookOrder::INTERVENTION_OUTPUT,
        };
        let this = self.clone();
        hooks.add_async_hook_with::<BeforeInvocationEvent<'static>>(input.clone(), move |event| {
            let this = this.clone();
            Box::pin(async move { this.on_before_invocation(event).await })
        });
        let this = self.clone();
        hooks.add_async_hook_with::<BeforeToolCallEvent>(input.clone(), move |event| {
            let this = this.clone();
            Box::pin(async move { this.on_before_tool_call(event).await })
        });
        let this = self.clone();
        hooks.add_async_hook_with::<AfterToolCallEvent>(output.clone(), move |event| {
            let this = this.clone();
            Box::pin(async move { this.on_after_tool_call(event).await })
        });
        let this = self.clone();
        hooks.add_async_hook_with::<BeforeModelCallEvent<'static>>(input, move |event| {
            let this = this.clone();
            Box::pin(async move { this.on_before_model_call(event).await })
        });
        let this = self.clone();
        hooks.add_async_hook_with::<AfterModelCallEvent<'static>>(output, move |event| {
            let this = this.clone();
            Box::pin(async move { this.on_after_model_call(event).await })
        });
    }
}

impl fmt::Debug for InterventionRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.handlers.iter().map(|handler| handler.name()).collect();
        f.debug_struct("InterventionRegistry")
            .field("handlers", &names)
            .finish()
    }
}

/// Generates one dispatch method (TS `_dispatch` with the event's `apply` closure).
///
/// `interrupts` counts the interrupts collected on the event: a handler that raises one pauses
/// the agent, which in TS throws past the remaining handlers.
macro_rules! dispatch {
    (
        $fn_name:ident, $method:ident, $label:literal, $event:ty,
        interrupts: $interrupts:expr, apply: $apply:path, deny: $deny:expr, guide: $guide:expr
    ) => {
        async fn $fn_name(&self, event: &mut $event) {
            tracing::debug!(
                "event=<{}> | dispatching to {} handler(s)",
                $label,
                self.handlers.len()
            );
            let interrupts: fn(&$event) -> usize = $interrupts;
            let deny: fn(&mut $event, &str, String) -> Outcome = $deny;
            let guide: fn(&mut $event, String) = $guide;
            let mut guides: Vec<String> = Vec::new();
            for handler in self.handlers.iter() {
                let name = handler.name();
                tracing::debug!("handler=<{name}>, event=<{}> | evaluating", $label);
                let raised = interrupts(event);
                let result = handler.$method(event).await;
                if interrupts(event) > raised {
                    return;
                }
                let outcome = match result {
                    Ok(action) => {
                        tracing::debug!(
                            "handler=<{name}>, event=<{}> | returned {}",
                            $label,
                            action.type_name()
                        );
                        $apply(event, action, name)
                    }
                    Err(error) => match handler.on_error() {
                        OnError::Throw => {
                            event.fail(error);
                            return;
                        }
                        OnError::Deny => {
                            tracing::debug!("handler=<{name}>, event=<{}> | returned deny", $label);
                            deny(event, name, format!("Handler threw: {error}"))
                        }
                        OnError::Proceed => Outcome::Continue,
                    },
                };
                match outcome {
                    Outcome::Continue => {}
                    Outcome::Stop => {
                        tracing::debug!("handler=<{name}>, event=<{}> | short-circuited", $label);
                        return;
                    }
                    Outcome::Guide(action) => guides.push(format!("[{name}] {}", action.feedback)),
                }
            }
            if !guides.is_empty() {
                tracing::debug!(
                    "event=<{}> | applying accumulated guide from {} handler(s)",
                    $label,
                    guides.len()
                );
                guide(event, guides.join("\n"));
            }
        }
    };
}

fn user_text(text: String) -> Message {
    Message::new(Role::User, vec![ContentBlock::Text(TextBlock::new(text))])
}

fn apply_before_invocation(
    event: &mut BeforeInvocationEvent<'_>,
    action: BeforeInvocationAction,
    _name: &str,
) -> Outcome {
    match action {
        BeforeInvocationAction::Deny(action) => {
            event.set_cancel(format!("DENIED: {}", action.reason));
            Outcome::Stop
        }
        BeforeInvocationAction::Guide(action) => Outcome::Guide(action),
        BeforeInvocationAction::Transform(action) => {
            (action.apply)(event);
            Outcome::Continue
        }
        BeforeInvocationAction::Proceed(_) => Outcome::Continue,
    }
}

fn apply_before_tool_call(
    event: &mut BeforeToolCallEvent,
    action: BeforeToolCallAction,
    name: &str,
) -> Outcome {
    match action {
        BeforeToolCallAction::Deny(action) => {
            event.set_cancel(format!("DENIED: {}", action.reason));
            Outcome::Stop
        }
        BeforeToolCallAction::Confirm(action) => {
            // A supplied response answers the interrupt at once; without one the interrupt is
            // recorded on the event and the agent pauses.
            let params = InterruptParams {
                name: name.to_owned(),
                reason: Some(JsonValue::String(action.prompt.clone())),
                response: action.response,
            };
            let Some(response) = event.interrupt(params) else {
                return Outcome::Stop;
            };
            let approved = match &action.evaluate {
                Some(evaluate) => evaluate(&response),
                None => default_evaluate(&response),
            };
            if approved {
                Outcome::Continue
            } else {
                event.set_cancel(format!("CONFIRMATION_FAILED: {}", action.prompt));
                Outcome::Stop
            }
        }
        BeforeToolCallAction::Guide(action) => Outcome::Guide(action),
        BeforeToolCallAction::Transform(action) => {
            (action.apply)(event);
            Outcome::Continue
        }
        BeforeToolCallAction::Proceed(_) => Outcome::Continue,
    }
}

fn apply_after_tool_call(
    event: &mut AfterToolCallEvent,
    action: AfterToolCallAction,
    _name: &str,
) -> Outcome {
    match action {
        AfterToolCallAction::Transform(action) => {
            (action.apply)(event);
            Outcome::Continue
        }
        AfterToolCallAction::Proceed(_) => Outcome::Continue,
    }
}

fn apply_before_model_call(
    event: &mut BeforeModelCallEvent<'_>,
    action: BeforeModelCallAction,
    _name: &str,
) -> Outcome {
    match action {
        BeforeModelCallAction::Deny(action) => {
            event.set_cancel(format!("DENIED: {}", action.reason));
            Outcome::Stop
        }
        BeforeModelCallAction::Guide(action) => Outcome::Guide(action),
        BeforeModelCallAction::Transform(action) => {
            (action.apply)(event);
            Outcome::Continue
        }
        BeforeModelCallAction::Proceed(_) => Outcome::Continue,
    }
}

fn apply_after_model_call(
    event: &mut AfterModelCallEvent<'_>,
    action: AfterModelCallAction,
    _name: &str,
) -> Outcome {
    match action {
        AfterModelCallAction::Guide(action) => Outcome::Guide(action),
        AfterModelCallAction::Transform(action) => {
            (action.apply)(event);
            Outcome::Continue
        }
        AfterModelCallAction::Proceed(_) => Outcome::Continue,
    }
}

impl InterventionRegistry {
    dispatch!(
        on_before_invocation, before_invocation, "beforeInvocation", BeforeInvocationEvent<'_>,
        interrupts: |_event| 0,
        apply: apply_before_invocation,
        deny: |event, _name, reason| {
            event.set_cancel(format!("DENIED: {reason}"));
            Outcome::Stop
        },
        guide: |event, feedback| event.set_cancel(format!("GUIDANCE: {feedback}"))
    );

    dispatch!(
        on_before_tool_call, before_tool_call, "beforeToolCall", BeforeToolCallEvent,
        interrupts: |event| event.collected_interrupts().len(),
        apply: apply_before_tool_call,
        deny: |event, _name, reason| {
            event.set_cancel(format!("DENIED: {reason}"));
            Outcome::Stop
        },
        guide: |event, feedback| event.set_cancel(format!("GUIDANCE: {feedback}"))
    );

    dispatch!(
        on_after_tool_call, after_tool_call, "afterToolCall", AfterToolCallEvent,
        interrupts: |_event| 0,
        apply: apply_after_tool_call,
        deny: |_event, name, _reason| {
            tracing::warn!("handler=<{name}>, event=<afterToolCall> | deny has no effect");
            Outcome::Continue
        },
        guide: |_event, _feedback| {}
    );

    // Guidance on a model call is appended to the messages directly, bypassing
    // `MessageAddedEvent` and the conversation manager, as in TS.
    dispatch!(
        on_before_model_call, before_model_call, "beforeModelCall", BeforeModelCallEvent<'_>,
        interrupts: |_event| 0,
        apply: apply_before_model_call,
        deny: |event, _name, reason| {
            event.set_cancel(format!("DENIED: {reason}"));
            Outcome::Stop
        },
        guide: |event, feedback| event.agent_mut().messages_mut().push(user_text(feedback))
    );

    dispatch!(
        on_after_model_call, after_model_call, "afterModelCall", AfterModelCallEvent<'_>,
        interrupts: |_event| 0,
        apply: apply_after_model_call,
        deny: |_event, name, _reason| {
            tracing::warn!("handler=<{name}>, event=<afterModelCall> | deny has no effect");
            Outcome::Continue
        },
        guide: |event, feedback| {
            event.set_retry(true);
            event.agent_mut().messages_mut().push(user_text(feedback));
        }
    );
}

#[cfg(test)]
mod tests;
