//! Model abstraction (TS `src/models/`).

#[cfg(feature = "anthropic")]
pub mod anthropic;
#[cfg(feature = "bedrock")]
pub mod bedrock;
pub(crate) mod defaults;
#[cfg(any(feature = "anthropic", feature = "openai"))]
pub(crate) mod http_transport;
pub mod model;
pub mod model_info;
pub(crate) mod model_proxy;
pub(crate) mod native_output;
#[cfg(feature = "openai")]
pub mod openai;
pub mod routing;
pub mod streaming;
pub(crate) mod strict_schema;
pub mod summarize;

#[cfg(feature = "anthropic")]
pub use self::anthropic::AnthropicModel;
#[cfg(feature = "bedrock")]
pub use self::bedrock::BedrockModel;
pub use self::model::{BaseModelConfig, BoxStream, Model, ModelRequest};
#[cfg(feature = "openai")]
pub use self::openai::OpenAIModel;
pub use self::streaming::ModelStreamEvent;
