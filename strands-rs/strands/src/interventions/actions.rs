//! Intervention actions (TS `interventions/actions.ts`).
//!
//! Each lifecycle gate has its own action enum holding only the actions that take effect there,
//! so a handler can't return an action its gate would ignore.

use std::fmt;
use std::sync::Arc;

use crate::hooks::registry::SyncHookFn;
use crate::hooks::{
    AfterModelCallEvent, AfterToolCallEvent, BeforeInvocationEvent, BeforeModelCallEvent,
    BeforeToolCallEvent, HookEvent, IntoSyncHook,
};
use crate::models::model::js_trim;
use crate::types::json::JsonValue;

/// The default `evaluate` of [`Confirm`] (TS `defaultEvaluate`): `true`, or `y`/`yes` in any
/// case with surrounding whitespace ignored.
pub(crate) fn default_evaluate(response: &JsonValue) -> bool {
    match response {
        JsonValue::Bool(approved) => *approved,
        JsonValue::String(text) => matches!(js_trim(&text.to_lowercase()), "y" | "yes"),
        _ => false,
    }
}

/// Lets the operation continue unchanged (TS `Proceed`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct Proceed {
    /// Debugging metadata; not shown to the model.
    pub reason: Option<String>,
}

impl Proceed {
    /// Sets the debugging reason.
    #[must_use]
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }
}

/// Blocks the operation (TS `Deny`); the reason is shown to the model.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Deny {
    /// Why the operation was blocked.
    pub reason: String,
}

/// Steers the model with feedback (TS `Guide`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Guide {
    /// The guidance shown to the model.
    pub feedback: String,
    /// Debugging metadata; not shown to the model.
    pub reason: Option<String>,
}

impl Guide {
    /// Sets the debugging reason.
    #[must_use]
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }
}

/// Decides whether a human's response to a [`Confirm`] approves the tool call.
pub type Evaluate = Arc<dyn Fn(&JsonValue) -> bool + Send + Sync>;

/// Asks a human to approve a tool call (TS `Confirm`).
#[derive(Clone)]
#[non_exhaustive]
pub struct Confirm {
    /// The message shown to the human; not shown to the model.
    pub prompt: String,
    /// Debugging metadata; not shown to the model.
    pub reason: Option<String>,
    /// A response collected ahead of time, so the agent doesn't pause.
    pub response: Option<JsonValue>,
    /// Decides whether the response approves. When unset, `true` and `y`/`yes` (any case,
    /// surrounding whitespace ignored) approve.
    pub evaluate: Option<Evaluate>,
}

impl Confirm {
    /// Sets the debugging reason.
    #[must_use]
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// Supplies the response up front, so the agent doesn't pause.
    #[must_use]
    pub fn with_response(mut self, response: impl Into<JsonValue>) -> Self {
        self.response = Some(response.into());
        self
    }

    /// Replaces the default check (`true`, `y` or `yes` approves).
    #[must_use]
    pub fn with_evaluate(mut self, evaluate: impl Fn(&JsonValue) -> bool + Send + Sync + 'static) -> Self {
        self.evaluate = Some(Arc::new(evaluate));
        self
    }
}

impl fmt::Debug for Confirm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Confirm")
            .field("prompt", &self.prompt)
            .field("reason", &self.reason)
            .field("response", &self.response)
            .finish_non_exhaustive()
    }
}

/// Changes the event in place (TS `Transform`); the closure runs when the registry applies the
/// action, so later handlers see the change.
pub struct Transform<M: HookEvent> {
    pub(crate) apply: SyncHookFn<M>,
    /// Debugging metadata; not shown to the model.
    pub reason: Option<String>,
}

impl<M: HookEvent> Transform<M> {
    /// Sets the debugging reason.
    #[must_use]
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }
}

impl<M: HookEvent> fmt::Debug for Transform<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Transform")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

/// [`Proceed`] with no reason (TS `proceed`).
pub fn proceed() -> Proceed {
    Proceed::default()
}

/// [`Deny`] with `reason` (TS `deny`).
pub fn deny(reason: impl Into<String>) -> Deny {
    Deny {
        reason: reason.into(),
    }
}

/// [`Guide`] with `feedback` (TS `guide`).
pub fn guide(feedback: impl Into<String>) -> Guide {
    Guide {
        feedback: feedback.into(),
        reason: None,
    }
}

/// [`Confirm`] with `prompt` and the default check: `true`, `y` or `yes` approves (TS `confirm`).
pub fn confirm(prompt: impl Into<String>) -> Confirm {
    Confirm {
        prompt: prompt.into(),
        reason: None,
        response: None,
        evaluate: None,
    }
}

/// [`Transform`] running `apply` on the event (TS `transform`); the event type comes from the
/// closure's argument type.
///
/// ```
/// use strands::hooks::BeforeToolCallEvent;
/// use strands::interventions::{BeforeToolCallAction, transform};
///
/// let action: BeforeToolCallAction =
///     transform(|event: &mut BeforeToolCallEvent| event.clear_cancel()).into();
/// ```
pub fn transform<M: HookEvent, F: IntoSyncHook<M>>(apply: F) -> Transform<M> {
    Transform {
        apply: apply.into_hook_fn(),
        reason: None,
    }
}

macro_rules! gate_actions {
    ($( $(#[$doc:meta])* $name:ident for $marker:ty { $( $variant:ident ),* } )*) => {
        $(
            $(#[$doc])*
            #[derive(Debug)]
            #[non_exhaustive]
            pub enum $name {
                $(
                    #[doc = concat!("[`", stringify!($variant), "`]")]
                    $variant(gate_actions!(@ty $variant, $marker)),
                )*
            }

            impl $name {
                /// The TS `type` of the action.
                pub(crate) fn type_name(&self) -> &'static str {
                    match self {
                        $( Self::$variant(_) => gate_actions!(@name $variant), )*
                    }
                }
            }

            $(
                impl From<gate_actions!(@ty $variant, $marker)> for $name {
                    fn from(action: gate_actions!(@ty $variant, $marker)) -> Self {
                        Self::$variant(action)
                    }
                }
            )*
        )*
    };
    (@name Proceed) => { "proceed" };
    (@name Deny) => { "deny" };
    (@name Guide) => { "guide" };
    (@name Confirm) => { "confirm" };
    (@name Transform) => { "transform" };
    (@ty Transform, $marker:ty) => { Transform<$marker> };
    (@ty $variant:ident, $marker:ty) => { $variant };
}

gate_actions! {
    /// What `before_invocation` may answer.
    BeforeInvocationAction for BeforeInvocationEvent<'static> { Proceed, Deny, Guide, Transform }
    /// What `before_tool_call` may answer.
    BeforeToolCallAction for BeforeToolCallEvent { Proceed, Deny, Guide, Confirm, Transform }
    /// What `after_tool_call` may answer.
    AfterToolCallAction for AfterToolCallEvent { Proceed, Transform }
    /// What `before_model_call` may answer.
    BeforeModelCallAction for BeforeModelCallEvent<'static> { Proceed, Deny, Guide, Transform }
    /// What `after_model_call` may answer.
    AfterModelCallAction for AfterModelCallEvent<'static> { Proceed, Guide, Transform }
}
