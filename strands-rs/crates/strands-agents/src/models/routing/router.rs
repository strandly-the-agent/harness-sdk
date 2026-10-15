//! Model routing primitives (`strands-ts/src/models/routing/router.ts`).

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use serde::Serialize;
use serde_json::{Map, Value};

use super::fallback::FallbackStrategy;
use super::strategy::RoutingStrategy;
use crate::error::{Error, Result};
use crate::models::Model;

/// What a [`RoutingCandidate`] routes to: a concrete model or an opaque nested router.
#[derive(Clone)]
pub enum CandidateModel {
    /// A concrete model.
    Model(Arc<dyn Model>),
    /// An opaque nested router, resolved by its own strategy.
    Router(Arc<ModelRouter>),
}

#[derive(Clone)]
struct CandidateInner {
    model: CandidateModel,
    name: Option<String>,
    description: Option<String>,
    metadata: Option<Map<String, Value>>,
}

/// A model or opaque model group with optional strategy-facing evidence.
///
/// Clones share one identity: the router and its strategies track candidate health by identity,
/// so [`PartialEq`] and [`Hash`] compare the shared allocation, not the fields. Each `with_*`
/// builder returns a fresh identity, so finish building before cloning.
///
/// Classifier-based strategies may send `name`, `description`, and `metadata` across provider
/// boundaries, so they must not contain secrets.
#[derive(Clone)]
pub struct RoutingCandidate(Arc<CandidateInner>);

impl RoutingCandidate {
    /// Candidate for a concrete model.
    pub fn new(model: Arc<dyn Model>) -> Self {
        Self::from_candidate_model(CandidateModel::Model(model))
    }

    /// Candidate for an opaque nested router.
    pub fn router(router: Arc<ModelRouter>) -> Self {
        Self::from_candidate_model(CandidateModel::Router(router))
    }

    fn from_candidate_model(model: CandidateModel) -> Self {
        Self(Arc::new(CandidateInner {
            model,
            name: None,
            description: None,
            metadata: None,
        }))
    }

    /// The same candidate with a strategy-facing name.
    pub fn with_name(self, name: impl Into<String>) -> Self {
        let name = name.into();
        self.map_inner(|inner| inner.name = Some(name))
    }

    /// The same candidate with a strategy-facing description.
    pub fn with_description(self, description: impl Into<String>) -> Self {
        let description = description.into();
        self.map_inner(|inner| inner.description = Some(description))
    }

    /// The same candidate with strategy-facing evidence, which must serialize to a JSON object.
    pub fn with_metadata<T: Serialize>(self, metadata: T) -> Result<Self> {
        let value = serde_json::to_value(metadata)
            .map_err(|error| Error::other(format!("metadata must be JSON-serializable: {error}")))?;
        let Value::Object(map) = value else {
            return Err(Error::other("metadata must be an object"));
        };
        Ok(self.map_inner(|inner| inner.metadata = Some(map)))
    }

    /// What this candidate routes to.
    pub fn model(&self) -> &CandidateModel {
        &self.0.model
    }

    /// The strategy-facing name, if set.
    pub fn name(&self) -> Option<&str> {
        self.0.name.as_deref()
    }

    /// The strategy-facing description, if set.
    pub fn description(&self) -> Option<&str> {
        self.0.description.as_deref()
    }

    /// The strategy-facing evidence, if set.
    pub fn metadata(&self) -> Option<&Map<String, Value>> {
        self.0.metadata.as_ref()
    }

    /// The candidate's name, or its model id, for logs and error messages.
    pub fn label(&self) -> String {
        match self.name() {
            Some(name) if !name.is_empty() => name.to_owned(),
            _ => match self.model() {
                CandidateModel::Model(model) => model_label(model),
                CandidateModel::Router(_) => "ModelRouter".to_owned(),
            },
        }
    }

    fn map_inner(self, apply: impl FnOnce(&mut CandidateInner)) -> Self {
        let mut inner = Arc::try_unwrap(self.0).unwrap_or_else(|shared| (*shared).clone());
        apply(&mut inner);
        Self(Arc::new(inner))
    }

    fn id(&self) -> *const CandidateInner {
        Arc::as_ptr(&self.0)
    }
}

impl From<Arc<dyn Model>> for RoutingCandidate {
    fn from(model: Arc<dyn Model>) -> Self {
        Self::new(model)
    }
}

impl From<Arc<ModelRouter>> for RoutingCandidate {
    fn from(router: Arc<ModelRouter>) -> Self {
        Self::router(router)
    }
}

impl PartialEq for RoutingCandidate {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for RoutingCandidate {}

impl Hash for RoutingCandidate {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id().hash(state);
    }
}

impl std::fmt::Debug for RoutingCandidate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoutingCandidate")
            .field("label", &self.label())
            .finish()
    }
}

/// Routes each agent invocation among an immutable set of candidate models.
///
/// The default [`FallbackStrategy`] prefers the candidate with the fewest recorded failures and
/// breaks ties by declaration order. `max_switches` bounds successful candidate changes per
/// invocation.
///
/// ```no_run
/// # use std::sync::Arc;
/// # use strands_agents::models::routing::{ModelRouter, RoutingCandidate};
/// # fn build(primary: Arc<dyn strands_agents::models::Model>, fallback: Arc<dyn strands_agents::models::Model>)
/// # -> strands_agents::error::Result<()> {
/// let router = ModelRouter::new(vec![
///     RoutingCandidate::new(primary).with_name("primary"),
///     RoutingCandidate::new(fallback).with_name("fallback"),
/// ])?;
/// # let _ = router;
/// # Ok(())
/// # }
/// ```
pub struct ModelRouter {
    candidates: Vec<RoutingCandidate>,
    strategy: Arc<dyn RoutingStrategy>,
    max_switches: Option<usize>,
}

impl ModelRouter {
    /// Plugin name this router registers under.
    pub const NAME: &'static str = "strands:model-router";

    /// Router over `models`, using [`FallbackStrategy`].
    ///
    /// Returns an error when the candidates are empty, duplicated, named alike, or stateful.
    pub fn new<C: Into<RoutingCandidate>>(models: impl IntoIterator<Item = C>) -> Result<Self> {
        let candidates: Vec<RoutingCandidate> = models.into_iter().map(Into::into).collect();
        if candidates.is_empty() {
            return Err(Error::other("ModelRouter requires at least one candidate model"));
        }
        reject_stateful(&candidates)?;
        reject_duplicates(&candidates)?;
        Ok(Self {
            candidates,
            strategy: Arc::new(FallbackStrategy),
            max_switches: None,
        })
    }

    /// The same router, deciding with `strategy`.
    pub fn with_strategy(mut self, strategy: impl RoutingStrategy + 'static) -> Self {
        self.strategy = Arc::new(strategy);
        self
    }

    /// The same router, capped at `max_switches` successful candidate changes per invocation.
    pub fn with_max_switches(mut self, max_switches: usize) -> Self {
        self.max_switches = Some(max_switches);
        self
    }

    /// Plugin name.
    pub fn name(&self) -> &str {
        Self::NAME
    }

    /// The configured candidate instances, in declaration order.
    pub fn candidates(&self) -> &[RoutingCandidate] {
        &self.candidates
    }

    /// The strategy responsible for every routing decision.
    pub fn strategy(&self) -> &Arc<dyn RoutingStrategy> {
        &self.strategy
    }

    /// The cap on successful candidate changes per invocation, if set.
    pub fn max_switches(&self) -> Option<usize> {
        self.max_switches
    }

    /// First declared candidate resolved without consulting a strategy.
    pub fn default_model(&self) -> Arc<dyn Model> {
        match self.candidates[0].model() {
            CandidateModel::Model(model) => Arc::clone(model),
            CandidateModel::Router(router) => router.default_model(),
        }
    }
}

impl std::fmt::Debug for ModelRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelRouter")
            .field("candidates", &self.candidates)
            .field("max_switches", &self.max_switches)
            .finish()
    }
}

/// Reject any stateful candidate model. Nested routers validated their own candidates already.
fn reject_stateful(candidates: &[RoutingCandidate]) -> Result<()> {
    for candidate in candidates {
        if let CandidateModel::Model(model) = candidate.model() {
            if model.stateful() {
                return Err(Error::other(format!(
                    "candidate=<{}> is stateful; routing among stateful models is not supported",
                    candidate.label()
                )));
            }
        }
    }
    Ok(())
}

/// Reject repeated candidates, repeated models, or colliding names.
///
/// Strategies track health per candidate, so one model behind two candidates would get two failure
/// budgets and never be demoted. The model check reaches through a nested router; names stay per
/// level, since a strategy only chooses among the candidates it is shown.
fn reject_duplicates(candidates: &[RoutingCandidate]) -> Result<()> {
    let mut seen_candidates: HashSet<*const CandidateInner> = HashSet::new();
    let mut seen_models: Vec<Arc<dyn Model>> = Vec::new();
    let mut seen_names: HashSet<&str> = HashSet::new();

    for candidate in candidates {
        if !seen_candidates.insert(candidate.id()) {
            return Err(Error::other("duplicate RoutingCandidate instance"));
        }

        let mut reachable = Vec::new();
        collect_models(candidate, &mut reachable);
        for model in reachable {
            if seen_models.iter().any(|seen| Arc::ptr_eq(seen, &model)) {
                return Err(Error::other(format!(
                    "candidate=<{}> repeats a model already routed to; construct a separate instance so each candidate has its own health",
                    candidate.label()
                )));
            }
            seen_models.push(model);
        }

        if let Some(name) = candidate.name() {
            if !seen_names.insert(name) {
                return Err(Error::other(format!("duplicate candidate name=<{name}>")));
            }
        }
    }
    Ok(())
}

/// Collect every concrete model this candidate can run, descending into a nested router.
fn collect_models(candidate: &RoutingCandidate, out: &mut Vec<Arc<dyn Model>>) {
    match candidate.model() {
        CandidateModel::Model(model) => out.push(Arc::clone(model)),
        CandidateModel::Router(router) => {
            for nested in router.candidates() {
                collect_models(nested, out);
            }
        }
    }
}

/// Label a model by its model id; Rust has no runtime provider class name.
fn model_label(model: &Arc<dyn Model>) -> String {
    model.model_id().unwrap_or_else(|| "Model".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::routing::test_support::{model, stateful_model};

    #[test]
    fn preserves_candidate_and_model_identity() {
        let selected_model = model();
        let selected = RoutingCandidate::new(Arc::clone(&selected_model))
            .with_name("vision")
            .with_description("Handles images");
        let router = ModelRouter::new(vec![selected.clone()]).unwrap();

        assert_eq!(router.candidates(), std::slice::from_ref(&selected));
        assert_eq!(selected.name(), Some("vision"));
        assert_eq!(selected.description(), Some("Handles images"));
        let CandidateModel::Model(candidate_model) = selected.model() else {
            panic!("expected a concrete model");
        };
        assert!(Arc::ptr_eq(candidate_model, &selected_model));
    }

    #[test]
    fn preserves_caller_metadata_and_validates_that_it_is_a_json_object() {
        let metadata = serde_json::json!({ "modelId": "fast-v1", "supportsToolUse": true });
        let with_metadata = RoutingCandidate::new(model())
            .with_metadata(metadata.clone())
            .unwrap();

        assert_eq!(with_metadata.metadata(), metadata.as_object());
        let error = RoutingCandidate::new(model())
            .with_metadata(serde_json::json!([]))
            .unwrap_err();
        assert_eq!(error.to_string(), "metadata must be an object");
    }

    #[test]
    fn normalizes_models_and_exposes_stable_plugin_configuration() {
        let first = model();
        let nested_default = model();
        let nested = Arc::new(ModelRouter::new(vec![Arc::clone(&nested_default)]).unwrap());
        let router = ModelRouter::new(vec![
            RoutingCandidate::new(Arc::clone(&first)),
            RoutingCandidate::router(Arc::clone(&nested)),
        ])
        .unwrap();

        assert_eq!(router.name(), "strands:model-router");
        assert_eq!(router.candidates().len(), 2);
        assert_eq!(router.max_switches(), None);
        assert!(Arc::ptr_eq(&router.default_model(), &first));
        assert!(matches!(
            router.candidates()[1].model(),
            CandidateModel::Router(_)
        ));
        assert!(Arc::ptr_eq(
            &ModelRouter::new(vec![RoutingCandidate::router(nested)])
                .unwrap()
                .default_model(),
            &nested_default
        ));
    }

    #[test]
    fn rejects_empty_models() {
        let error = ModelRouter::new(Vec::<RoutingCandidate>::new()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "ModelRouter requires at least one candidate model"
        );
    }

    #[test]
    fn rejects_stateful_models() {
        let error = ModelRouter::new(vec![stateful_model()]).unwrap_err();
        assert_eq!(
            error.to_string(),
            "candidate=<test-model> is stateful; routing among stateful models is not supported"
        );
    }

    #[test]
    fn rejects_duplicate_topology() {
        let shared = model();
        let duplicate = RoutingCandidate::new(model()).with_name("same");

        let repeated_model = ModelRouter::new(vec![Arc::clone(&shared), Arc::clone(&shared)]).unwrap_err();
        assert_eq!(
            repeated_model.to_string(),
            "candidate=<test-model> repeats a model already routed to; construct a separate instance so each candidate has its own health"
        );

        let repeated_candidate = ModelRouter::new(vec![duplicate.clone(), duplicate]).unwrap_err();
        assert_eq!(
            repeated_candidate.to_string(),
            "duplicate RoutingCandidate instance"
        );

        let repeated_name = ModelRouter::new(vec![
            RoutingCandidate::new(model()).with_name("same"),
            RoutingCandidate::new(model()).with_name("same"),
        ])
        .unwrap_err();
        assert_eq!(repeated_name.to_string(), "duplicate candidate name=<same>");

        let nested = Arc::new(ModelRouter::new(vec![Arc::clone(&shared)]).unwrap());
        let nested_repeat = ModelRouter::new(vec![
            RoutingCandidate::new(Arc::clone(&shared)),
            RoutingCandidate::router(nested),
        ])
        .unwrap_err();
        assert_eq!(
            nested_repeat.to_string(),
            "candidate=<ModelRouter> repeats a model already routed to; construct a separate instance so each candidate has its own health"
        );
    }

    #[test]
    fn candidate_label_falls_back_to_the_model_id() {
        assert_eq!(RoutingCandidate::new(model()).label(), "test-model");
        assert_eq!(RoutingCandidate::new(model()).with_name("fast").label(), "fast");
    }
}
