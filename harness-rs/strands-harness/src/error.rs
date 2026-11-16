//! The harness error type.

/// An error of the harness: option and config resolution, and anything the SDK reports while the
/// agent is built.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HarnessError {
    /// An error of the SDK.
    #[error(transparent)]
    Sdk(#[from] strands::Error),
}

/// The harness result type.
pub type Result<T, E = HarnessError> = std::result::Result<T, E>;
