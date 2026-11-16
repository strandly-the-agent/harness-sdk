//! The model router and its candidates (TS `models/routing/router.ts`).

use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::models::model::Model;
use crate::models::routing::fallback_strategy::FallbackStrategy;
use crate::models::routing::strategy::RoutingStrategy;
use crate::types::json::JsonObject;

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
pub struct ModelRouterOptions {
    /// The strategy responsible for every routing decision (default [`FallbackStrategy`]).
    #[builder(with = |strategy: impl RoutingStrategy| Arc::new(strategy) as Arc<dyn RoutingStrategy>)]
    pub strategy: Option<Arc<dyn RoutingStrategy>>,
    /// The maximum successful candidate switches per invocation (default unlimited).
    pub max_switches: Option<u32>,
}

impl fmt::Debug for ModelRouterOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelRouterOptions")
            .field("strategy", &self.strategy.as_ref().map(|strategy| strategy.name()))
            .field("max_switches", &self.max_switches)
            .finish()
    }
}

pub(super) struct RouterInner {
    pub(super) candidates: Vec<RoutingCandidate>,
    pub(super) default_model: Arc<dyn Model>,
    pub(super) strategy: Arc<dyn RoutingStrategy>,
    pub(super) max_switches: Option<u32>,
}

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
