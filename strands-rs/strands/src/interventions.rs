//! Interventions (TS `src/interventions/`): policy handlers that gate the agent's lifecycle
//! events and answer with typed actions.

#[doc(hidden)]
pub mod actions;
#[doc(hidden)]
pub mod handler;
pub(crate) mod registry;

pub(crate) use actions::default_evaluate;
#[doc(inline)]
pub use actions::{
    AfterModelCallAction, AfterToolCallAction, BeforeInvocationAction, BeforeModelCallAction,
    BeforeToolCallAction, Confirm, Deny, Evaluate, Guide, Proceed, Transform, confirm, deny, guide, proceed,
    transform,
};
#[doc(inline)]
pub use handler::{InterventionHandler, OnError};
pub(crate) use registry::InterventionRegistry;
