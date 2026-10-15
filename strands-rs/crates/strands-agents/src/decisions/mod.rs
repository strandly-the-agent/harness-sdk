//! Decision models: closed questions answered by a model, with typed answers
//! (`strands-ts/src/decisions/`).
//!
//! A decision is a stateless, single-shot question set — not a conversation. Pass some state and
//! an ordered [`Questions`] map to [`ask`](DecisionModelExt::ask) and get one [`Answer`] per
//! question back, plus token usage and latency.
//!
//! ```no_run
//! use strands_agents::decisions::{DecisionModelExt, LlmDecisionModel, Questions};
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let decision = LlmDecisionModel::builder().build().await?;
//! let result = decision
//!     .ask(
//!         "The fruit is an apple",
//!         &Questions::new()
//!             .boolean("is_fruit", "Is the thing talked about a fruit?")
//!             .choice("color", "What color is the fruit?", ["red", "green", "blue"]),
//!     )
//!     .await?;
//!
//! assert_eq!(result.answer("is_fruit").and_then(|a| a.as_bool()), Some(true));
//! # Ok(())
//! # }
//! ```

mod decision_model;
mod llm_decision_model;
mod types;

pub use decision_model::{DecisionModel, DecisionModelExt};
pub use llm_decision_model::{LlmDecisionModel, LlmDecisionModelBuilder};
pub use types::{
    Answer, DecisionInput, DecisionMetadata, DecisionResult, Question, Questions, Uncertain, Uncertainty,
};
