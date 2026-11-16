//! The crate's seam for reading the process environment.

use std::sync::Arc;

/// Looks up an environment variable; tests pass their own lookup instead of mutating the
/// process environment.
#[cfg_attr(
    all(not(feature = "mcp"), not(test)),
    expect(dead_code, reason = "used by web_search, whose tool needs the `mcp` feature")
)]
pub(crate) type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The lookup that reads the process environment.
#[cfg_attr(
    not(feature = "mcp"),
    expect(dead_code, reason = "used by web_search, whose tool needs the `mcp` feature")
)]
pub(crate) fn process_env() -> EnvLookup {
    Arc::new(|name| std::env::var(name).ok())
}
