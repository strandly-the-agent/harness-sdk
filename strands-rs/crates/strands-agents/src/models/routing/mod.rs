//! Model routing: pick among candidate models per invocation.
//!
//! Ports `strands-ts/src/models/routing/` ([`ModelRouter`], [`RoutingCandidate`],
//! [`RoutingStrategy`], [`FallbackStrategy`]).
//!
//! Divergences from TS:
//! - [`RoutingCandidate`] is `Arc`-backed, so clones share one identity and `PartialEq`/`Hash`
//!   compare that identity. TS relies on object identity and `Object.freeze`.
//! - Candidate models are `Arc<dyn Model>` (a nested router is `Arc<ModelRouter>`) so that the
//!   duplicate-model guard and strategy health tracking can compare identity.
//! - Checks TS performs at runtime are enforced by the type system here and have no error path:
//!   `models must be a sequence of candidates`, `candidate must be a Model or ModelRouter`,
//!   `strategy must implement RoutingStrategy`, `maxSwitches must be a non-negative integer`
//!   (`usize`), and metadata containing a non-finite number (`serde_json::Value` cannot hold one).
//! - Candidate labels use `Model::model_id`; Rust has no runtime provider class name.
//! - Selection runs as wrap-phase middleware, not TS's `InvokeModelStage.Input`: `add_input` is
//!   sync while `RoutingStrategy::select` is async. Wrap composes innermost, so it still sees the
//!   context after continuation input middleware, as TS requires.
//! - `RoutingStrategy::name` replaces TS's `strategy.constructor.name` in routing logs.
//! - [`ClassifierStrategy`] options are fallible `with_*` builders instead of an options bag, and
//!   keep TS's verbatim messages (`maxMessageChars must be a positive integer`); non-integer and
//!   negative values are unrepresentable. Its timeout races an injectable [`Sleeper`](crate::retry::Sleeper)
//!   and cancels the in-flight call through `StreamOptions::cancel` (TS's `AbortController`).
//! - `AgentBuilder::aux_model` takes `impl Model`, so TS's
//!   `auxModel must be a Model or a Bedrock model id, not a ModelRouter` has no error path.

mod classifier;
mod fallback;
mod router;
mod selection;
mod strategy;

pub use classifier::ClassifierStrategy;
pub use fallback::FallbackStrategy;
pub use router::{CandidateModel, ModelRouter, RoutingCandidate};
pub use strategy::{RoutingAttempt, RoutingContext, RoutingStrategy};

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Arc;

    use crate::models::{Model, ModelEventStream, StreamOptions};
    use crate::testing::{MockMessageModel, MockResponse};
    use crate::types::Message;
    use async_trait::async_trait;

    /// A fresh mock model that always replies `ok`.
    pub(crate) fn model() -> Arc<dyn Model> {
        Arc::new(MockMessageModel::new(vec![MockResponse::message(
            Message::assistant("ok"),
        )]))
    }

    /// A fresh mock model that reports itself as stateful.
    pub(crate) fn stateful_model() -> Arc<dyn Model> {
        Arc::new(StatefulModel(MockMessageModel::new(vec![MockResponse::message(
            Message::assistant("ok"),
        )])))
    }

    struct StatefulModel(MockMessageModel);

    #[async_trait]
    impl Model for StatefulModel {
        fn model_id(&self) -> Option<String> {
            self.0.model_id()
        }

        fn stateful(&self) -> bool {
            true
        }

        fn stream<'a>(&'a self, messages: &'a [Message], options: &'a StreamOptions) -> ModelEventStream<'a> {
            self.0.stream(messages, options)
        }
    }
}
