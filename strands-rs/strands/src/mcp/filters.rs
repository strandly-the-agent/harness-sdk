//! Tool filters (TS `McpToolFilters`, `McpToolMatcher`).

use std::fmt;
use std::sync::Arc;

use regex::Regex;

use super::mcp_tool::McpTool;

/// A callback filter; it sees the tool under its agent-facing (prefixed) name.
pub type McpToolFilterCallback = Arc<dyn Fn(&McpTool) -> bool + Send + Sync>;

/// One filter entry (TS `McpToolMatcher`).
///
/// Names and patterns match the server-side tool name; a pattern matches from the start of the
/// name (TS adds the sticky flag), not necessarily the whole name.
#[derive(Clone)]
#[non_exhaustive]
pub enum McpToolMatcher {
    /// The exact server-side name.
    Name(String),
    /// A pattern anchored at the start of the server-side name.
    Pattern(Regex),
    /// A callback on the listed tool.
    Callback(McpToolFilterCallback),
}

impl McpToolMatcher {
    /// A callback matcher.
    pub fn callback(callback: impl Fn(&McpTool) -> bool + Send + Sync + 'static) -> Self {
        Self::Callback(Arc::new(callback))
    }

    fn matches(&self, tool: &McpTool, server_name: &str) -> bool {
        match self {
            Self::Name(name) => name == server_name,
            Self::Pattern(pattern) => pattern.find(server_name).is_some_and(|found| found.start() == 0),
            Self::Callback(callback) => callback(tool),
        }
    }
}

impl From<&str> for McpToolMatcher {
    fn from(name: &str) -> Self {
        Self::Name(name.to_owned())
    }
}

impl From<String> for McpToolMatcher {
    fn from(name: String) -> Self {
        Self::Name(name)
    }
}

impl From<Regex> for McpToolMatcher {
    fn from(pattern: Regex) -> Self {
        Self::Pattern(pattern)
    }
}

impl fmt::Debug for McpToolMatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Name(name) => f.debug_tuple("Name").field(name).finish(),
            Self::Pattern(pattern) => f.debug_tuple("Pattern").field(&pattern.as_str()).finish(),
            Self::Callback(_) => f.write_str("Callback(..)"),
        }
    }
}

/// Which tools a client exposes (TS `McpToolFilters`).
///
/// With `allowed` set, only matching tools are exposed (an empty list exposes none); `rejected`
/// then removes matches, so it wins over `allowed`.
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default)]
pub struct McpToolFilters {
    /// When set, only tools matching one of these are exposed.
    pub allowed: Option<Vec<McpToolMatcher>>,
    /// Tools matching one of these are excluded.
    pub rejected: Option<Vec<McpToolMatcher>>,
}

impl McpToolFilters {
    /// Whether `tool` (server-side name `server_name`) passes (TS `shouldIncludeTool`).
    pub(crate) fn includes(&self, tool: &McpTool, server_name: &str) -> bool {
        let any =
            |matchers: &[McpToolMatcher]| matchers.iter().any(|matcher| matcher.matches(tool, server_name));
        if self.allowed.as_deref().is_some_and(|allowed| !any(allowed)) {
            return false;
        }
        !self.rejected.as_deref().is_some_and(any)
    }
}
