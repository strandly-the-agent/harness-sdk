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

#[doc(inline)]
#[cfg(feature = "anthropic")]
pub use self::anthropic::AnthropicModel;
#[doc(inline)]
#[cfg(feature = "bedrock")]
pub use self::bedrock::BedrockModel;
#[doc(inline)]
pub use self::model::{
    BaseModelConfig, BaseModelConfigBuilder, BoxStream, Model, ModelRequest, ModelRequestBuilder,
    base_model_config_builder, model_request_builder,
};
#[doc(inline)]
#[cfg(feature = "openai")]
pub use self::openai::OpenAIModel;
#[doc(inline)]
pub use self::streaming::ModelStreamEvent;
