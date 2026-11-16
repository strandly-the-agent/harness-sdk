//! The harness error type.

/// An error of the harness: option and config resolution, and anything the SDK reports while the
/// agent is built.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HarnessError {
    /// An error of the SDK.
    #[error(transparent)]
    Sdk(#[from] strands::Error),
    /// A model string names a provider whose cargo feature is disabled.
    #[error("model '{spec}' requires the strands feature \"{feature}\"")]
    ModelFeatureDisabled {
        /// The model string.
        spec: String,
        /// The cargo feature that enables the provider.
        feature: &'static str,
    },
    /// A model string names a provider the TypeScript harness has and this crate doesn't yet.
    #[error("model provider {} is not supported in Rust yet", json_string(.provider))]
    UnsupportedModelProvider {
        /// The provider prefix of the model string.
        provider: String,
    },
    /// A model string names an unknown provider.
    #[error(
        "Unknown model provider {} in {}. Supported providers: {supported}. Pass a Model instance for anything else.",
        json_string(.provider),
        json_string(.spec)
    )]
    UnknownModelProvider {
        /// The provider prefix of the model string.
        provider: String,
        /// The model string.
        spec: String,
        /// The supported provider names, comma separated.
        supported: String,
    },
    /// The resolved model doesn't take the requested reasoning effort.
    #[error("Effort {} is not supported by {subject}. {detail}", json_string(.effort))]
    UnsupportedEffort {
        /// The requested effort.
        effort: String,
        /// The model the effort was requested for, as the message names it.
        subject: String,
        /// The levels the model does take.
        detail: String,
    },
    /// Prompt caching was requested explicitly for a model that has none.
    #[error("Model {model} does not support prompt caching. Pass caching: false.")]
    CachingUnsupported {
        /// The model, as `provider/name`.
        model: String,
    },
    /// No summarizer model is known for the main model's provider.
    #[error(
        "No default web_fetch model for provider {}. Pass builtinTools.web_fetch.model explicitly, or disable web_fetch via builtinTools.",
        json_string(.provider)
    )]
    NoWebFetchModel {
        /// The provider prefix of the main model string.
        provider: String,
    },
}

/// `text` as TS `JSON.stringify` prints it.
fn json_string(text: &str) -> String {
    serde_json::Value::from(text).to_string()
}

/// The harness result type.
pub type Result<T, E = HarnessError> = std::result::Result<T, E>;
