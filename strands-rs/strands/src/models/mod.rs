//! Model abstraction (TS `src/models/`).

#[cfg(feature = "anthropic")]
pub mod anthropic;
#[cfg(feature = "bedrock")]
pub mod bedrock;
pub(crate) mod defaults;
pub mod model;
pub mod model_info;
pub(crate) mod model_proxy;
pub(crate) mod native_output;
#[cfg(feature = "openai")]
pub mod openai;
pub mod streaming;
pub(crate) mod strict_schema;
pub mod summarize;
