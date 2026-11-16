//! Strategy-based model routing (TS `models/routing/`).
//!
//! A [`ModelRouter`] asks its [`RoutingStrategy`] before the first model call of an invocation
//! and after unclaimed failures until routing stops. A strategy that declines the opening
//! selection leaves the first candidate's concrete default model in place; declining after a
//! failure preserves the pending error.
//!
//! A failure round uses each candidate at most once. A successful call starts a new round with
//! that candidate already counted as used. Nested routers are opaque candidates: they make one
//! selection with an empty attempt history and do not fail over internally for the outer router.
//!
//! The agent's model stays the first candidate's concrete model, so the opening
//! `BeforeModelCallEvent` reports that default while `AfterModelCallEvent` reports the routed
//! model. Token estimation also uses the default model, so prefer candidates with comparable
//! context windows and tokenizers. If a candidate fails after emitting part of a streamed
//! response, consumers see that partial output followed by the replacement model's full response.
//!
//! The API is provisional and may change before it is finalized.

mod classifier_strategy;
mod fallback_strategy;
mod router;
mod strategy;

pub use classifier_strategy::{
    ClassifierStrategy, ClassifierStrategyOptions, ClassifierStrategyOptionsBuilder,
    classifier_strategy_options_builder,
};
pub use fallback_strategy::FallbackStrategy;
pub use router::{
    CandidateModel, ModelRouter, ModelRouterOptions, ModelRouterOptionsBuilder, RoutingCandidate,
    model_router_options_builder,
};
pub use strategy::{RoutingAttempt, RoutingContext, RoutingException, RoutingStrategy};

#[cfg(test)]
mod tests;
