//! Interventions (TS `src/interventions/`): policy handlers that gate the agent's lifecycle
//! events and answer with typed actions.

pub mod actions;
pub mod handler;
pub(crate) mod registry;

pub(crate) use actions::default_evaluate;
pub use actions::{
    AfterModelCallAction, AfterToolCallAction, BeforeInvocationAction, BeforeModelCallAction,
    BeforeToolCallAction, Confirm, Deny, Evaluate, Guide, Proceed, Transform, confirm, deny, guide, proceed,
    transform,
};
pub use handler::{InterventionHandler, OnError};
pub(crate) use registry::InterventionRegistry;
