//! Logging helpers. The SDK logs through `tracing`; install any subscriber to see output.

/// Emit a `tracing::warn!` once per distinct message for the lifetime of the process.
pub fn warn_once(message: &str) {
    crate::models::warn_once_impl(message)
}
