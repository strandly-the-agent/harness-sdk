//! The intervention handler trait (TS `interventions/handler.ts`).

use async_trait::async_trait;

use super::actions::{
    AfterModelCallAction, AfterToolCallAction, BeforeInvocationAction, BeforeModelCallAction,
    BeforeToolCallAction, proceed,
};
use crate::error::Result;
use crate::hooks::{
    AfterModelCallEvent, AfterToolCallEvent, BeforeInvocationEvent, BeforeModelCallEvent, BeforeToolCallEvent,
};

/// What the registry does when a handler method fails (TS `OnError`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum OnError {
    /// Fail the invocation with the handler's error: a broken policy check blocks execution.
    #[default]
    Throw,
    /// Continue as if the handler had proceeded. Fail-open: a broken handler stops enforcing
    /// its policy.
    Proceed,
    /// Treat the failure as a deny (fail-closed).
    Deny,
}

/// A policy evaluated at the agent's lifecycle gates (TS `InterventionHandler`).
///
/// Implement the gates the policy cares about; the others proceed.
///
/// ```
/// use strands::hooks::BeforeToolCallEvent;
/// use strands::interventions::{BeforeToolCallAction, InterventionHandler, deny, proceed};
///
/// struct NoShell;
///
/// #[strands::async_trait]
/// impl InterventionHandler for NoShell {
///     fn name(&self) -> &str {
///         "no-shell"
///     }
///
///     async fn before_tool_call(&self, event: &mut BeforeToolCallEvent) -> strands::Result<BeforeToolCallAction> {
///         if event.tool_use().name() == "shell" {
///             return Ok(deny("shell is disabled").into());
///         }
///         Ok(proceed().into())
///     }
/// }
/// ```
#[async_trait]
pub trait InterventionHandler: Send + Sync + 'static {
    /// The handler's name, unique within an agent.
    fn name(&self) -> &str;

    /// What to do when one of this handler's methods fails.
    fn on_error(&self) -> OnError {
        OnError::Throw
    }

    /// Runs before an invocation starts.
    async fn before_invocation(
        &self,
        _event: &mut BeforeInvocationEvent<'_>,
    ) -> Result<BeforeInvocationAction> {
        Ok(proceed().into())
    }

    /// Runs before a tool call.
    async fn before_tool_call(&self, _event: &mut BeforeToolCallEvent) -> Result<BeforeToolCallAction> {
        Ok(proceed().into())
    }

    /// Runs after a tool call.
    async fn after_tool_call(&self, _event: &mut AfterToolCallEvent) -> Result<AfterToolCallAction> {
        Ok(proceed().into())
    }

    /// Runs before a model call.
    async fn before_model_call(
        &self,
        _event: &mut BeforeModelCallEvent<'_>,
    ) -> Result<BeforeModelCallAction> {
        Ok(proceed().into())
    }

    /// Runs after a model call.
    async fn after_model_call(&self, _event: &mut AfterModelCallEvent<'_>) -> Result<AfterModelCallAction> {
        Ok(proceed().into())
    }
}

impl std::fmt::Debug for dyn InterventionHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InterventionHandler")
            .field("name", &self.name())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
