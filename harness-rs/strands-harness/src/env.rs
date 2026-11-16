//! The crate's seam for reading the process environment.
#![expect(
    dead_code,
    reason = "used once the modules that read the environment land; drop this when they have"
)]

use std::sync::Arc;

/// Looks up an environment variable; tests pass their own lookup instead of mutating the
/// process environment.
pub(crate) type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The lookup that reads the process environment.
pub(crate) fn process_env() -> EnvLookup {
    Arc::new(|name| std::env::var(name).ok())
}
