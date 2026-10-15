//! Agent wiring for [`ModelRouter`]: per-invocation selection and failure-driven candidate
//! switching (`strands-ts/src/models/routing/router.ts` `initAgent`/`_selectionMiddleware`).

use std::collections::HashSet;
use std::sync::Arc;

use parking_lot::Mutex;

use super::router::{CandidateModel, ModelRouter, RoutingCandidate};
use super::strategy::{RoutingAttempt, RoutingContext};
use crate::agent::{Agent, InvocationState};
use crate::error::{Error, Result};
use crate::hooks::{hook_order, AfterInvocationEvent, AfterModelCallEvent, BeforeInvocationEvent};
use crate::middleware::{wrap_fn, InvokeModelContext, InvokeModelStage};
use crate::models::Model;
#[cfg(test)]
use crate::plugins::Plugin;

const KEY_PREFIX: &str = "strands:modelRouting";

/// Error raised when a router is registered as an ordinary plugin.
pub(crate) const NOT_THE_MODEL_MESSAGE: &str =
    "ModelRouter must be passed through Agent({ model }), not plugins";

struct StateInner {
    candidate: RoutingCandidate,
    model: Arc<dyn Model>,
    attempts: Vec<RoutingAttempt>,
    switched_to: HashSet<RoutingCandidate>,
    switches: usize,
}

/// One invocation's routing decision, held in [`InvocationState`] under this agent/router key.
pub(crate) struct RoutingState(Mutex<StateInner>);

impl RoutingState {
    // Candidate hashing is by `Arc` identity, so interior mutability cannot change a key.
    #[allow(clippy::mutable_key_type)]
    fn opening(candidate: RoutingCandidate, model: Arc<dyn Model>) -> Self {
        // The opening candidate counts as used for this round.
        let switched_to = HashSet::from([candidate.clone()]);
        Self(Mutex::new(StateInner {
            candidate,
            model,
            attempts: Vec::new(),
            switched_to,
            switches: 0,
        }))
    }

    fn model(&self) -> Arc<dyn Model> {
        Arc::clone(&self.0.lock().model)
    }
}

impl ModelRouter {
    /// Register routing middleware and lifecycle hooks on the agent this router is the model for.
    ///
    /// Returns an error when the router was attached as an ordinary plugin instead of being
    /// passed to [`AgentBuilder::model_router`](crate::agent::AgentBuilder::model_router).
    pub(crate) fn register(&self, agent: &Agent) -> Result<()> {
        let router = self.shared_handle(agent)?;
        if !mark(&router.initialized, agent) {
            return Ok(());
        }

        let selecting = Arc::clone(&router);
        agent.middleware().add::<InvokeModelStage>(wrap_fn(
            move |mut context: InvokeModelContext, sink, next| {
                let router = Arc::clone(&selecting);
                async move {
                    context.model = router.select_for_call(&context).await?;
                    next.run(context, &sink).await
                }
            },
        ));

        let advancing = Arc::clone(&router);
        agent
            .hooks()
            .add_callback_boxed_with_order::<AfterModelCallEvent, _>(
                hook_order::MODEL_ROUTING,
                move |event| {
                    let router = Arc::clone(&advancing);
                    Box::pin(async move { router.on_model_result(event).await })
                },
            );

        let clearing = Arc::clone(&router);
        agent
            .hooks()
            .add_sync_callback_with_order::<AfterInvocationEvent, _>(
                hook_order::SDK_LAST,
                move |event: &mut AfterInvocationEvent| {
                    clearing.clear_state(&event.agent, &event.invocation_state);
                    Ok(())
                },
            );
        let starting = Arc::clone(&router);
        agent
            .hooks()
            .add_sync_callback_with_order::<BeforeInvocationEvent, _>(
                hook_order::SDK_FIRST,
                move |event: &mut BeforeInvocationEvent| {
                    starting.clear_state(&event.agent, &event.invocation_state);
                    Ok(())
                },
            );
        Ok(())
    }

    /// The model this invocation is currently routed to, or `None` before the opening selection.
    ///
    /// Mirrors TS `getRoutedModel`.
    pub fn routed_model(&self, agent: &Agent, invocation_state: &InvocationState) -> Option<Arc<dyn Model>> {
        self.state(agent, invocation_state).map(|state| state.model())
    }

    /// Apply this invocation's selection to one model call, opening one on the first call.
    async fn select_for_call(&self, context: &InvokeModelContext) -> Result<Arc<dyn Model>> {
        if let Some(state) = self.state(&context.agent, &context.invocation_state) {
            return Ok(state.model());
        }
        let routing = self.routing_context(
            context.messages.clone(),
            context.system_prompt.clone(),
            context.tool_specs.clone(),
            context.invocation_state.clone(),
            Vec::new(),
        );
        let (candidate, model) = self.open(&routing).await?;
        context.invocation_state.insert(
            self.state_key(&context.agent),
            RoutingState::opening(candidate, Arc::clone(&model)),
        );
        Ok(model)
    }

    /// Choose and resolve the candidate to start on; a decline serves the default model.
    async fn open(&self, context: &RoutingContext) -> Result<(RoutingCandidate, Arc<dyn Model>)> {
        let Some(candidate) = self.ask(context).await? else {
            let fallback = self.candidates()[0].clone();
            let model = self.default_model();
            tracing::info!(
                strategy = self.strategy().name(),
                candidate = fallback.label(),
                model = label(&model),
                "strategy declined the opening choice, using the default model"
            );
            return Ok((fallback, model));
        };
        let model = self.resolve(&candidate, context).await?;
        tracing::info!(
            strategy = self.strategy().name(),
            candidate = candidate.label(),
            model = label(&model),
            "candidate selected"
        );
        Ok((candidate, model))
    }

    /// Ask the strategy for a candidate and reject an answer outside the configured set.
    async fn ask(&self, context: &RoutingContext) -> Result<Option<RoutingCandidate>> {
        let Some(candidate) = self.strategy().select(context).await? else {
            return Ok(None);
        };
        if !context.candidates.contains(&candidate) {
            return Err(Error::other(
                "strategy.select must return a candidate from context.candidates",
            ));
        }
        Ok(Some(candidate))
    }

    /// Resolve a candidate to a concrete model, asking a nested router for one selection.
    async fn resolve(
        &self,
        candidate: &RoutingCandidate,
        context: &RoutingContext,
    ) -> Result<Arc<dyn Model>> {
        match candidate.model() {
            CandidateModel::Model(model) => Ok(Arc::clone(model)),
            CandidateModel::Router(nested) => {
                let inner = nested.routing_context(
                    context.messages.clone(),
                    context.system_prompt.clone(),
                    context.tool_specs.clone(),
                    context.invocation_state.clone(),
                    Vec::new(),
                );
                match nested.ask(&inner).await? {
                    Some(chosen) => Box::pin(nested.resolve(&chosen, &inner)).await,
                    None => Ok(nested.default_model()),
                }
            }
        }
    }

    /// Record the outcome and, after an unclaimed failure, apply the strategy's next choice.
    async fn on_model_result(&self, event: &mut AfterModelCallEvent) -> Result<()> {
        let Some(state) = self.state(&event.agent, &event.invocation_state) else {
            return Ok(());
        };
        if matches!(event.error.as_deref(), Some(Error::Cancelled)) {
            return Ok(());
        }
        if event.stop_data.is_some() {
            #[allow(clippy::mutable_key_type)]
            let mut inner = state.0.lock();
            let candidate = inner.candidate.clone();
            inner.attempts.push(RoutingAttempt::success(candidate.clone()));
            // The candidate that succeeded opens the next round, counting as used.
            inner.switched_to = HashSet::from([candidate]);
            return Ok(());
        }
        if event.retry {
            return Ok(());
        }
        let Some(error) = event.error.clone() else {
            return Ok(());
        };

        {
            let mut inner = state.0.lock();
            let candidate = inner.candidate.clone();
            inner.attempts.push(RoutingAttempt::failure(candidate, error));
            if let Some(cap) = self.max_switches() {
                if inner.switches >= cap {
                    tracing::warn!(
                        max_switches = cap,
                        "switch cap reached, leaving the error to surface"
                    );
                    return Ok(());
                }
            }
        }

        if self.advance(event, &state).await {
            event.retry = true;
        }
        Ok(())
    }

    /// Switch to the strategy's next candidate, returning whether the call should be retried.
    ///
    /// A strategy that errors, declines, or names a candidate the round already used leaves the
    /// model's error to surface. An unresolvable candidate burns its slot and the strategy is
    /// asked once more, so the round stays bounded by the candidate count.
    async fn advance(&self, event: &AfterModelCallEvent, state: &RoutingState) -> bool {
        loop {
            let context = self.routing_context(
                event.agent.messages(),
                event.agent.system_prompt(),
                event.agent.tools().iter().map(|t| t.spec().clone()).collect(),
                event.invocation_state.clone(),
                state.0.lock().attempts.clone(),
            );

            let candidate = match self.ask(&context).await {
                Ok(Some(candidate)) => candidate,
                Ok(None) => return false,
                Err(error) => {
                    tracing::warn!(
                        strategy = self.strategy().name(),
                        %error,
                        "routing failed, leaving the error to surface"
                    );
                    return false;
                }
            };
            if state.0.lock().switched_to.contains(&candidate) {
                tracing::info!(
                    candidate = candidate.label(),
                    "already used this round, leaving the error to surface"
                );
                return false;
            }

            let model = match self.resolve(&candidate, &context).await {
                Ok(model) => model,
                Err(error) => {
                    tracing::warn!(
                        candidate = candidate.label(),
                        %error,
                        "candidate could not be resolved, asking again"
                    );
                    let mut inner = state.0.lock();
                    inner
                        .attempts
                        .push(RoutingAttempt::failure(candidate.clone(), Arc::new(error)));
                    inner.switched_to.insert(candidate);
                    continue;
                }
            };

            let mut inner = state.0.lock();
            tracing::info!(
                from_candidate = inner.candidate.label(),
                to_candidate = candidate.label(),
                model = label(&model),
                "model call failed, switching candidate"
            );
            inner.candidate = candidate.clone();
            inner.model = model;
            inner.switched_to.insert(candidate);
            inner.switches += 1;
            return true;
        }
    }

    fn routing_context(
        &self,
        messages: Vec<crate::types::Message>,
        system_prompt: Option<crate::types::SystemPrompt>,
        tool_specs: Vec<crate::types::ToolSpec>,
        invocation_state: InvocationState,
        attempts: Vec<RoutingAttempt>,
    ) -> RoutingContext {
        RoutingContext {
            messages,
            system_prompt,
            tool_specs,
            candidates: self.candidates().to_vec(),
            invocation_state,
            attempts,
        }
    }

    fn state(&self, agent: &Agent, invocation_state: &InvocationState) -> Option<Arc<RoutingState>> {
        invocation_state.get::<RoutingState>(&self.state_key(agent))
    }

    fn clear_state(&self, agent: &Agent, invocation_state: &InvocationState) {
        invocation_state.remove(&self.state_key(agent));
    }

    /// Scope routing state to one agent/router pair: one invocation state can serve several
    /// agents, and one router several agents, so neither identity alone is a sufficient key.
    fn state_key(&self, agent: &Agent) -> String {
        format!(
            "{KEY_PREFIX}:{:x}:{:x}",
            Arc::as_ptr(&agent.inner) as *const () as usize,
            self as *const Self as usize
        )
    }
}

/// Record `agent` in `slot`, returning whether it was not already there.
pub(crate) fn mark(slot: &Mutex<Vec<std::sync::Weak<crate::agent::AgentInner>>>, agent: &Agent) -> bool {
    let mut slot = slot.lock();
    slot.retain(|weak| weak.strong_count() > 0);
    let target = Arc::as_ptr(&agent.inner);
    if slot.iter().any(|weak| weak.as_ptr() == target) {
        return false;
    }
    slot.push(Arc::downgrade(&agent.inner));
    true
}

/// Whether `agent` is recorded in `slot`.
pub(crate) fn marked(slot: &Mutex<Vec<std::sync::Weak<crate::agent::AgentInner>>>, agent: &Agent) -> bool {
    let mut slot = slot.lock();
    slot.retain(|weak| weak.strong_count() > 0);
    let target = Arc::as_ptr(&agent.inner);
    slot.iter().any(|weak| weak.as_ptr() == target)
}

fn label(model: &Arc<dyn Model>) -> String {
    model.model_id().unwrap_or_else(|| "Model".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::routing::test_support::model;
    use crate::testing::empty_model;

    #[tokio::test]
    async fn rejects_a_router_registered_as_an_ordinary_plugin() {
        let router = Arc::new(ModelRouter::new(vec![model()]).unwrap());
        let agent = crate::agent::AgentBuilder::new()
            .model(empty_model())
            .plugin(Arc::clone(&router) as Arc<dyn Plugin>)
            .build()
            .unwrap();

        let error = agent.initialize().await.unwrap_err();

        assert!(
            error.to_string().contains(NOT_THE_MODEL_MESSAGE),
            "unexpected error: {error}"
        );
    }
}
