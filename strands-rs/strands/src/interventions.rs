//! Interventions (TS `src/interventions/`): policy handlers that gate the agent's lifecycle
//! events and answer with typed actions.

pub mod actions;
pub mod handler;
pub mod registry;

pub use actions::{
    AfterModelCallAction, AfterToolCallAction, BeforeInvocationAction, BeforeModelCallAction,
    BeforeToolCallAction, Confirm, Deny, Guide, Proceed, Transform, confirm, default_evaluate, deny, guide,
    proceed, transform,
};
pub use handler::{InterventionHandler, OnError};
pub use registry::InterventionRegistry;
