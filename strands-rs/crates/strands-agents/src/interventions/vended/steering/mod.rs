//! Steering (`strands-ts/src/vended-interventions/steering/**`): just-in-time guidance for an
//! agent, instead of front-loading every instruction into the system prompt.
//!
//! A steering handler sits on the tool-call gate, reads what its
//! [`SteeringContextProvider`]s have observed, and decides: proceed, guide the agent with
//! feedback, or ask a human. [`LlmSteeringHandler`] delegates that judgement to a model, scored
//! against the natural-language rules in its system prompt.
//!
//! ```no_run
//! # async fn demo(model: impl strands_agents::models::Model + 'static) -> strands_agents::error::Result<()> {
//! use strands_agents::interventions::vended::steering::LlmSteeringHandler;
//! use strands_agents::Agent;
//!
//! let agent = Agent::builder()
//!     .model(model)
//!     .intervention(LlmSteeringHandler::new("Guide the agent away from repeating failed calls."))
//!     .build()?;
//! # Ok(())
//! # }
//! ```
//!
//! # Divergences from TS
//!
//! - Steering is an **intervention** (as in TS; Python models it as a plugin). TS's abstract
//!   `SteeringHandler` class becomes [`SteeringProviders`] (the provider plumbing) plus
//!   [`SteeringAction`] → [`ToolCallAction`](crate::interventions::ToolCallAction): Rust has no
//!   inheritance, and a blanket `impl<T: SteeringHandler> InterventionHandler for T` would overlap
//!   the existing handler impls. Handlers implement
//!   [`InterventionHandler`](crate::interventions::InterventionHandler) directly.
//! - Context providers self-register hooks through
//!   [`InterventionHandler::init_agent`](crate::interventions::InterventionHandler::init_agent),
//!   this port's equivalent of TS's `LifecycleObserver` probe on intervention handlers.
//! - TS's `afterModelCall` steering gate is [`InterventionHandler::after_model_call`](crate::interventions::InterventionHandler::after_model_call):
//!   an `Action::Guide` there retries the model call with the feedback appended as a user message.
//!   `LlmSteeringHandler` leaves it at `proceed`, exactly as TS's `LLMSteeringHandler` does.

mod context;
mod handler;
mod llm;
mod tool_ledger;

pub use context::{SteeringContextData, SteeringContextProvider};
pub use handler::{SteeringAction, SteeringProviders};
pub use llm::{LlmSteeringHandler, PromptBuilder, DEFAULT_LLM_STEERING_NAME};
pub use tool_ledger::{ToolLedgerProvider, DEFAULT_MAX_ENTRIES, DEFAULT_TOOL_LEDGER_NAME};
