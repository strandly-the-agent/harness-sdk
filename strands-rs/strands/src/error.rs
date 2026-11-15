//! Errors returned by the SDK (TS `src/errors.ts`).

/// A boxed error, used for error sources and plumbing failures.
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// The result type used throughout the SDK.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Every error the SDK returns.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Plumbing failures (I/O, JSON) and the plain `Error` TS throws for malformed data.
    #[error("{0}")]
    Other(BoxError),
}

impl Error {
    /// Wraps a plumbing error or a message as [`Error::Other`].
    pub fn other(error: impl Into<BoxError>) -> Self {
        Self::Other(error.into())
    }
}
