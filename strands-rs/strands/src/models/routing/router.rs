//! The model router and its candidates (TS `models/routing/router.ts`).

use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use async_trait::async_trait;
use futures::future::BoxFuture;

use crate::agent::{CancelledError, InvocationContext};
use crate::error::{Error, Result};
use crate::hooks::{
    AfterInvocationEvent, AfterModelCallEvent, BeforeInvocationEvent, HookCallbackOptions, HookOrder,
};
use crate::middleware::{InvokeModelContext, InvokeModelStage, MiddlewareError, MiddlewareStage};
use crate::models::model::Model;
use crate::models::routing::fallback_strategy::FallbackStrategy;
use crate::models::routing::strategy::{RoutingAttempt, RoutingContext, RoutingStrategy};
use crate::plugins::Plugin;
use crate::types::agent::{InvocationState, LocalAgent};
use crate::types::json::JsonObject;
use crate::types::messages::{Message, SystemPrompt};
use crate::types::tools::ToolSpec;

/// What a [`RoutingCandidate`] routes to: a concrete model or an opaque nested router.
#[derive(Clone)]
#[non_exhaustive]
pub enum CandidateModel {
    /// A concrete model.
    Model(Arc<dyn Model>),
    /// A nested router, asked once with an empty attempt history.
    Router(ModelRouter),
}

impl From<Arc<dyn Model>> for CandidateModel {
    fn from(model: Arc<dyn Model>) -> Self {
        Self::Model(model)
    }
}

impl<M: Model> From<M> for CandidateModel {
    fn from(model: M) -> Self {
        Self::Model(Arc::new(model))
    }
}

impl<M: Model> From<Arc<M>> for CandidateModel {
    fn from(model: Arc<M>) -> Self {
        Self::Model(model)
    }
}

impl From<ModelRouter> for CandidateModel {
    fn from(router: ModelRouter) -> Self {
        Self::Router(router)
    }
}

impl fmt::Debug for CandidateModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Model(model) => f.debug_tuple("Model").field(&model_label(model)).finish(),
            Self::Router(router) => f.debug_tuple("Router").field(router).finish(),
        }
    }
}

#[derive(Clone)]
struct CandidateInner {
    model: CandidateModel,
    name: Option<String>,
    description: Option<String>,
    metadata: Option<JsonObject>,
}

/// A model or opaque model group with optional strategy-facing evidence (TS
/// `RoutingCandidate`).
///
/// Clones share one identity, and `==` compares that identity: a strategy returns one of the
/// candidates it was shown. Set the evidence before cloning the candidate or handing it to a
/// router. Classifier-based strategies may send `name`, `description` and `metadata` across
/// provider boundaries, so they must not contain secrets.
///
/// ```
/// use strands::models::routing::RoutingCandidate;
/// use strands::testing::MockMessageModel;
///
/// let candidate = RoutingCandidate::new(MockMessageModel::new())
///     .with_name("routine")
///     .with_description("Short factual requests");
/// assert_eq!(candidate.name(), Some("routine"));
/// assert_eq!(candidate, candidate.clone());
/// assert_ne!(candidate, RoutingCandidate::new(MockMessageModel::new()));
/// ```
#[derive(Clone)]
pub struct RoutingCandidate {
    inner: Arc<CandidateInner>,
}

impl RoutingCandidate {
    /// A candidate for `model` without evidence.
    pub fn new(model: impl Into<CandidateModel>) -> Self {
        Self {
            inner: Arc::new(CandidateInner {
                model: model.into(),
                name: None,
                description: None,
                metadata: None,
            }),
        }
    }

    /// Sets the strategy-facing name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        Arc::make_mut(&mut self.inner).name = Some(name.into());
        self
    }

    /// Sets the strategy-facing description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        Arc::make_mut(&mut self.inner).description = Some(description.into());
        self
    }

    /// Sets the strategy-facing evidence; it must be free of secrets.
    #[must_use]
    pub fn with_metadata(mut self, metadata: JsonObject) -> Self {
        Arc::make_mut(&mut self.inner).metadata = Some(metadata);
        self
    }

    /// The concrete model or nested router.
    pub fn model(&self) -> &CandidateModel {
        &self.inner.model
    }

    /// The strategy-facing name.
    pub fn name(&self) -> Option<&str> {
        self.inner.name.as_deref()
    }

    /// The strategy-facing description.
    pub fn description(&self) -> Option<&str> {
        self.inner.description.as_deref()
    }

    /// The strategy-facing evidence.
    pub fn metadata(&self) -> Option<&JsonObject> {
        self.inner.metadata.as_ref()
    }

    /// The candidate's name, or its model id when it has none (TS `candidateLabel`).
    pub(crate) fn label(&self) -> String {
        match (self.name(), self.model()) {
            (Some(name), _) if !name.is_empty() => name.to_owned(),
            (_, CandidateModel::Model(model)) => model_label(model),
            (_, CandidateModel::Router(_)) => "ModelRouter".to_owned(),
        }
    }

    /// Every concrete model this candidate can run, descending into a nested router.
    fn reachable_models(&self) -> Vec<Arc<dyn Model>> {
        match self.model() {
            CandidateModel::Model(model) => vec![Arc::clone(model)],
            CandidateModel::Router(router) => router
                .candidates()
                .iter()
                .flat_map(Self::reachable_models)
                .collect(),
        }
    }
}

impl From<Arc<dyn Model>> for RoutingCandidate {
    fn from(model: Arc<dyn Model>) -> Self {
        Self::new(model)
    }
}

impl<M: Model> From<M> for RoutingCandidate {
    fn from(model: M) -> Self {
        Self::new(model)
    }
}

impl<M: Model> From<Arc<M>> for RoutingCandidate {
    fn from(model: Arc<M>) -> Self {
        Self::new(model)
    }
}

impl From<ModelRouter> for RoutingCandidate {
    fn from(router: ModelRouter) -> Self {
        Self::new(router)
    }
}

impl PartialEq for RoutingCandidate {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for RoutingCandidate {}

impl Hash for RoutingCandidate {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.inner).hash(state);
    }
}

impl fmt::Debug for RoutingCandidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RoutingCandidate")
            .field("model", &self.inner.model)
            .field("name", &self.inner.name)
            .field("description", &self.inner.description)
            .field("metadata", &self.inner.metadata)
            .finish()
    }
}

/// Options for a [`ModelRouter`] (TS `ModelRouterOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Clone, Default, bon::Builder)]
#[builder(state_mod(vis = "pub"))]
pub struct ModelRouterOptions {
    /// The strategy responsible for every routing decision (default [`FallbackStrategy`]).
    #[builder(with = |strategy: impl RoutingStrategy| Arc::new(strategy) as Arc<dyn RoutingStrategy>)]
    pub strategy: Option<Arc<dyn RoutingStrategy>>,
    /// The maximum successful candidate switches per invocation (default unlimited).
    pub max_switches: Option<u32>,
}

impl ModelRouter {
    /// Attaches this router as `agent`'s configured model (TS `attachToAgent`): registers the
    /// selection middleware and the routing and cleanup hooks, once per agent.
    pub(crate) fn attach_to_agent(&self, agent: &LocalAgent) {
        let owner = agent.agent_sandbox().owner();
        {
            let mut agents = self.agents();
            agents.retain(|attached| attached.agent.strong_count() > 0);
            if agents.iter().any(|attached| attached.is(owner)) {
                return;
            }
            agents.push(AttachedAgent {
                agent: Arc::downgrade(owner),
                state: None,
            });
        }

        let router = self.clone();
        // Dropping a cleanup keeps its registration.
        let _ = agent.add_middleware(InvokeModelStage::input(move |context| {
            let router = router.clone();
            async move { router.selection_middleware(context).await }
        }));
        let hooks = agent.hooks();
        let router = self.clone();
        let _ = hooks.add_async_hook_with::<AfterModelCallEvent<'static>>(
            HookCallbackOptions {
                order: HookOrder::MODEL_ROUTING,
            },
            move |event| {
                let router = router.clone();
                Box::pin(async move { router.on_model_result(event).await })
            },
        );
        let router = self.clone();
        let _ = hooks.add_hook_with(
            HookCallbackOptions {
                order: HookOrder::SDK_LAST,
            },
            move |event: &mut AfterInvocationEvent<'_>| router.clear_state(event.agent()),
        );
        let router = self.clone();
        let _ = hooks.add_hook_with(
            HookCallbackOptions {
                order: HookOrder::SDK_FIRST,
            },
            move |event: &mut BeforeInvocationEvent<'_>| router.clear_state(event.agent()),
        );
    }

    /// The model routed for `agent`'s current invocation (TS `getRoutedModel`); `None` before
    /// the opening selection.
    pub(crate) fn routed_model(&self, agent: &LocalAgent) -> Option<Arc<dyn Model>> {
        self.state(agent.agent_sandbox().owner()).map(|state| state.model)
    }

    fn agents(&self) -> MutexGuard<'_, Vec<AttachedAgent>> {
        self.inner.agents.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn is_attached(&self, owner: &Arc<()>) -> bool {
        self.agents().iter().any(|attached| attached.is(owner))
    }

    fn state(&self, owner: &Arc<()>) -> Option<RoutingState> {
        self.agents()
            .iter()
            .find(|attached| attached.is(owner))
            .and_then(|attached| attached.state.clone())
    }

    fn set_state(&self, owner: &Arc<()>, state: Option<RoutingState>) {
        if let Some(attached) = self.agents().iter_mut().find(|attached| attached.is(owner)) {
            attached.state = state;
        }
    }

    /// Drops `agent`'s routing state so it never spans invocations.
    fn clear_state(&self, agent: &LocalAgent) {
        self.set_state(agent.agent_sandbox().owner(), None);
    }

    /// Applies the invocation's selection to the model call, opening one on the first call.
    async fn selection_middleware(
        &self,
        mut context: InvokeModelContext,
    ) -> std::result::Result<InvokeModelContext, MiddlewareError> {
        let owner = Arc::clone(context.agent().agent_sandbox().owner());
        let state = match self.state(&owner) {
            Some(state) => state,
            None => {
                let routing_context = self.routing_context(
                    context.messages.clone(),
                    context.system_prompt.clone(),
                    context.tool_specs.clone(),
                    context.invocation_state.clone(),
                    context.invocation_context(),
                    Vec::new(),
                );
                let (candidate, model) = self.open(&routing_context).await?;
                let state = RoutingState::new(candidate, model);
                self.set_state(&owner, Some(state.clone()));
                state
            }
        };
        context.model = state.model;
        Ok(context)
    }

    /// Chooses and resolves the candidate to start on. A decline serves the default model; a
    /// strategy error and a candidate that will not resolve both propagate.
    async fn open(&self, context: &RoutingContext) -> Result<(RoutingCandidate, Arc<dyn Model>)> {
        let strategy = self.inner.strategy.name();
        let Some(candidate) = self.ask(context).await? else {
            let model = self.default_model();
            // The constructor rejects an empty candidate list.
            let fallback = self
                .inner
                .candidates
                .first()
                .cloned()
                .ok_or_else(|| Error::other("ModelRouter requires at least one candidate model"))?;
            tracing::info!(
                "strategy=<{strategy}>, candidate=<{}>, model=<{}> | strategy declined the opening choice, using the default model",
                fallback.label(),
                model_label(&model)
            );
            return Ok((fallback, model));
        };
        let model = self.resolve(&candidate, context).await?;
        tracing::info!(
            "strategy=<{strategy}>, candidate=<{}>, model=<{}> | candidate selected",
            candidate.label(),
            model_label(&model)
        );
        Ok((candidate, model))
    }

    /// Asks the strategy for a candidate; one that isn't among `context.candidates` is an error.
    async fn ask(&self, context: &RoutingContext) -> Result<Option<RoutingCandidate>> {
        let candidate = self.inner.strategy.select(context).await?;
        match candidate {
            Some(candidate) if !context.candidates.contains(&candidate) => Err(Error::other(
                "strategy.select must return a candidate from context.candidates",
            )),
            candidate => Ok(candidate),
        }
    }

    /// Resolves a candidate to a concrete model. A nested router is asked with its own
    /// candidates and no attempts, so it contributes one candidate and never advances.
    fn resolve<'a>(
        &'a self,
        candidate: &'a RoutingCandidate,
        context: &'a RoutingContext,
    ) -> BoxFuture<'a, Result<Arc<dyn Model>>> {
        Box::pin(async move {
            let nested = match candidate.model() {
                CandidateModel::Model(model) => return Ok(Arc::clone(model)),
                CandidateModel::Router(nested) => nested,
            };
            let nested_context = nested.routing_context(
                context.messages.clone(),
                context.system_prompt.clone(),
                context.tool_specs.clone(),
                context.invocation_state.clone(),
                context.invocation_context.as_ref(),
                Vec::new(),
            );
            match nested.ask(&nested_context).await? {
                None => Ok(nested.default_model()),
                Some(selected) => nested.resolve(&selected, &nested_context).await,
            }
        })
    }

    /// Records the outcome and, after an unclaimed failure, applies the strategy's next choice.
    async fn on_model_result(&self, event: &mut AfterModelCallEvent<'_>) {
        let owner = Arc::clone(event.agent().agent_sandbox().owner());
        let Some(mut state) = self.state(&owner) else {
            return;
        };
        let error = event.error();
        if error.is_some_and(is_cancelled) {
            return;
        }
        if event.stop_data().is_some() {
            state
                .attempts
                .push(RoutingAttempt::success(state.candidate.clone()));
            // The candidate that succeeded opens the next round, counting as used.
            state.switched_to = vec![state.candidate.clone()];
            self.set_state(&owner, Some(state));
            return;
        }
        let Some(error) = error.filter(|_| !event.retry()) else {
            return;
        };

        state
            .attempts
            .push(RoutingAttempt::failure(state.candidate.clone(), error));
        match self.inner.max_switches {
            Some(max_switches) if state.switches >= max_switches => tracing::warn!(
                "maxSwitches=<{max_switches}> | switch cap reached, leaving the error to surface"
            ),
            _ => {
                if self.advance(event, &mut state, error).await {
                    event.set_retry(true);
                }
            }
        }
        self.set_state(&owner, Some(state));
    }

    /// Switches to the strategy's next candidate, returning whether the call should be retried.
    ///
    /// A strategy that fails, declines or names a candidate the round already used leaves the
    /// model's error to surface. A candidate that will not resolve takes its slot in the round
    /// and the strategy is asked once more, so the round stays bounded by the candidate count.
    async fn advance(
        &self,
        event: &mut AfterModelCallEvent<'_>,
        state: &mut RoutingState,
        error: &Error,
    ) -> bool {
        loop {
            let agent = event.agent();
            let context = self.routing_context(
                agent.messages().to_vec(),
                agent.system_prompt().cloned(),
                agent.tool_registry().tool_specs(),
                event.invocation_state().clone(),
                event.invocation_context(),
                state.attempts.clone(),
            );

            // A model error is already pending, so a broken answer must not replace it.
            let candidate = match self.ask(&context).await {
                Ok(Some(candidate)) => candidate,
                Ok(None) => return false,
                Err(error) => {
                    tracing::warn!(
                        "strategy=<{}>, error=<{}> | routing failed, leaving the error to surface",
                        self.inner.strategy.name(),
                        error_label(&error)
                    );
                    return false;
                }
            };
            // A round uses each candidate at most once, so a strategy that cycles cannot keep
            // resetting the retry budget.
            if state.switched_to.contains(&candidate) {
                tracing::info!(
                    "candidate=<{}> | already used this round, leaving the error to surface",
                    candidate.label()
                );
                return false;
            }

            match self.resolve(&candidate, &context).await {
                Ok(model) => {
                    tracing::info!(
                        "from_candidate=<{}>, to_candidate=<{}>, error=<{}> | model call failed, switching candidate",
                        state.candidate.label(),
                        candidate.label(),
                        error.name()
                    );
                    state.candidate = candidate.clone();
                    state.model = model;
                    state.switched_to.push(candidate);
                    state.switches = state.switches.saturating_add(1);
                    return true;
                }
                Err(resolution_error) => {
                    tracing::warn!(
                        "candidate=<{}>, error=<{}> | candidate could not be resolved, asking again",
                        candidate.label(),
                        error_label(&resolution_error)
                    );
                    // Recorded so a strategy reading attempts stops offering it, and slot-burned
                    // so termination does not depend on the strategy doing that.
                    state
                        .attempts
                        .push(RoutingAttempt::failure(candidate.clone(), &resolution_error));
                    state.switched_to.push(candidate);
                }
            }
        }
    }

    /// A [`RoutingContext`] over this router's candidates.
    fn routing_context(
        &self,
        messages: Vec<Message>,
        system_prompt: Option<SystemPrompt>,
        tool_specs: Vec<ToolSpec>,
        invocation_state: InvocationState,
        invocation_context: Option<&InvocationContext>,
        attempts: Vec<RoutingAttempt>,
    ) -> RoutingContext {
        let mut context = RoutingContext::new(self.inner.candidates.iter().cloned())
            .with_messages(messages)
            .with_tool_specs(tool_specs)
            .with_invocation_state(invocation_state)
            .with_attempts(attempts);
        context.system_prompt = system_prompt;
        context.invocation_context = InvocationContext::auxiliary(invocation_context);
        context
    }
}

impl AttachedAgent {
    fn is(&self, owner: &Arc<()>) -> bool {
        std::ptr::eq(self.agent.as_ptr(), Arc::as_ptr(owner))
    }
}

#[async_trait]
impl Plugin for ModelRouter {
    fn name(&self) -> &str {
        Self::NAME
    }

    /// Does nothing for the agent this router is the model of, which the agent attached when
    /// it was built.
    ///
    /// # Errors
    ///
    /// The router was passed as an ordinary plugin rather than as the model.
    async fn init_agent(self: Arc<Self>, agent: &mut LocalAgent) -> Result<()> {
        if self.is_attached(agent.agent_sandbox().owner()) {
            Ok(())
        } else {
            Err(Error::other(NOT_THE_MODEL))
        }
    }
}

/// Whether `error` is the cancel of the invocation (TS `CancelledError`).
fn is_cancelled(error: &Error) -> bool {
    matches!(error, Error::Other(source) if source.is::<CancelledError>())
}

/// An error as TS logs it (`String(error)`): its name, then its message.
fn error_label(error: &Error) -> String {
    format!("{}: {error}", error.name())
}

impl fmt::Debug for ModelRouterOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelRouterOptions")
            .field(
                "strategy",
                &self.strategy.as_ref().map(|strategy| strategy.name()),
            )
            .field("max_switches", &self.max_switches)
            .finish()
    }
}

pub(super) struct RouterInner {
    pub(super) candidates: Vec<RoutingCandidate>,
    pub(super) default_model: Arc<dyn Model>,
    pub(super) strategy: Arc<dyn RoutingStrategy>,
    pub(super) max_switches: Option<u32>,
    /// The agents this router is the model of, each with its invocation's routing state (TS
    /// keeps the state in the invocation state under an agent/router key).
    agents: Mutex<Vec<AttachedAgent>>,
}

struct AttachedAgent {
    agent: Weak<()>,
    state: Option<RoutingState>,
}

/// One invocation's candidate, attempt history and switch accounting.
#[derive(Clone)]
struct RoutingState {
    candidate: RoutingCandidate,
    model: Arc<dyn Model>,
    attempts: Vec<RoutingAttempt>,
    /// The candidates used in the current failure round; the opening candidate counts as used.
    switched_to: Vec<RoutingCandidate>,
    switches: u32,
}

impl RoutingState {
    fn new(candidate: RoutingCandidate, model: Arc<dyn Model>) -> Self {
        Self {
            switched_to: vec![candidate.clone()],
            candidate,
            model,
            attempts: Vec::new(),
            switches: 0,
        }
    }
}

const NOT_THE_MODEL: &str = "ModelRouter must be passed through Agent({ model }), not plugins";

/// Routes each agent invocation among a fixed set of candidate models (TS `ModelRouter`).
///
/// The default [`FallbackStrategy`] prefers the candidate with the fewest recorded failures and
/// breaks ties by declaration order. `max_switches` bounds successful candidate changes per
/// invocation. Pass the router as the agent's model; clones share one router.
///
/// ```
/// use strands::models::routing::{ModelRouter, RoutingCandidate};
/// use strands::testing::MockMessageModel;
/// use strands::Agent;
///
/// let router = ModelRouter::new([
///     RoutingCandidate::new(MockMessageModel::new()).with_name("primary"),
///     RoutingCandidate::new(MockMessageModel::new()).with_name("fallback"),
/// ])?;
/// let agent = Agent::builder().model(router).build()?;
/// # Ok::<(), strands::Error>(())
/// ```
#[derive(Clone)]
pub struct ModelRouter {
    pub(super) inner: Arc<RouterInner>,
}

impl ModelRouter {
    /// The plugin name (TS `name`).
    pub const NAME: &'static str = "strands:model-router";

    /// A router over `models` (models, nested routers or candidates) with the default options.
    ///
    /// # Errors
    ///
    /// As [`with_options`](Self::with_options).
    pub fn new(models: impl IntoIterator<Item = impl Into<RoutingCandidate>>) -> Result<Self> {
        Self::with_options(models, ModelRouterOptions::default())
    }

    /// A router over `models` (models, nested routers or candidates).
    ///
    /// # Errors
    ///
    /// The candidates are empty, repeat a candidate, a model or a name, or include a stateful
    /// model.
    pub fn with_options(
        models: impl IntoIterator<Item = impl Into<RoutingCandidate>>,
        options: ModelRouterOptions,
    ) -> Result<Self> {
        let candidates: Vec<RoutingCandidate> = models.into_iter().map(Into::into).collect();
        let default_model = match candidates.first().map(RoutingCandidate::model) {
            None => return Err(Error::other("ModelRouter requires at least one candidate model")),
            Some(CandidateModel::Model(model)) => Arc::clone(model),
            Some(CandidateModel::Router(router)) => router.default_model(),
        };
        reject_stateful(&candidates)?;
        reject_duplicates(&candidates)?;
        Ok(Self {
            inner: Arc::new(RouterInner {
                candidates,
                default_model,
                strategy: options.strategy.unwrap_or_else(|| Arc::new(FallbackStrategy)),
                max_switches: options.max_switches,
                agents: Mutex::default(),
            }),
        })
    }

    /// The candidates in declaration order.
    pub fn candidates(&self) -> &[RoutingCandidate] {
        &self.inner.candidates
    }

    /// The first declared candidate's concrete model, resolved without consulting a strategy.
    pub fn default_model(&self) -> Arc<dyn Model> {
        Arc::clone(&self.inner.default_model)
    }
}

impl fmt::Debug for ModelRouter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelRouter")
            .field("candidates", &self.inner.candidates)
            .field("strategy", &self.inner.strategy.name())
            .field("max_switches", &self.inner.max_switches)
            .finish()
    }
}

/// Rejects any stateful candidate model.
fn reject_stateful(candidates: &[RoutingCandidate]) -> Result<()> {
    for candidate in candidates {
        if let CandidateModel::Model(model) = candidate.model()
            && model.stateful()
        {
            return Err(Error::other(format!(
                "candidate=<{}> is stateful; routing among stateful models is not supported",
                candidate.label()
            )));
        }
    }
    Ok(())
}

/// Rejects repeated candidates, repeated models and colliding names.
///
/// Strategies track health per candidate, so one model behind two candidates would get two
/// failure budgets and never be demoted. The model check reaches through a nested router;
/// names stay per level, since a strategy only chooses among the candidates it is shown.
fn reject_duplicates(candidates: &[RoutingCandidate]) -> Result<()> {
    let mut seen_models: Vec<Arc<dyn Model>> = Vec::new();
    for (index, candidate) in candidates.iter().enumerate() {
        let earlier = candidates.get(..index).unwrap_or_default();
        if earlier.contains(candidate) {
            return Err(Error::other("duplicate RoutingCandidate instance"));
        }
        for model in candidate.reachable_models() {
            if seen_models.iter().any(|seen| Arc::ptr_eq(seen, &model)) {
                return Err(Error::other(format!(
                    "candidate=<{}> repeats a model already routed to; construct a separate instance so each candidate has its own health",
                    candidate.label()
                )));
            }
            seen_models.push(model);
        }
        let Some(name) = candidate.name() else { continue };
        if earlier.iter().any(|other| other.name() == Some(name)) {
            return Err(Error::other(format!("duplicate candidate name=<{name}>")));
        }
    }
    Ok(())
}

/// Labels a model by its model id (TS `modelLabel`).
pub(super) fn model_label(model: &Arc<dyn Model>) -> String {
    model.model_id().unwrap_or_else(|| "Model".to_owned())
}
