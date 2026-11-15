//! Tool executors (TS `tools/executors/`): how the tool calls of one model turn run.

pub mod concurrent;
pub(crate) mod executor;
pub mod sequential;

pub use concurrent::ConcurrentToolExecutor;
pub use sequential::SequentialToolExecutor;

use executor::{ToolExecutionInput, ToolExecutorStream};

/// The executor an agent runs tool calls with (TS `toolExecutor`: an executor instance or the
/// `'concurrent'`/`'sequential'` shorthand). Defaults to [`ConcurrentToolExecutor`].
///
/// ```
/// use strands::tools::executors::{SequentialToolExecutor, ToolExecutor};
///
/// let executor = ToolExecutor::from(SequentialToolExecutor::new());
/// assert!(matches!(executor, ToolExecutor::Sequential(_)));
/// assert!(matches!(ToolExecutor::default(), ToolExecutor::Concurrent(_)));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolExecutor {
    /// Runs the tool calls concurrently (TS `'concurrent'`).
    Concurrent(ConcurrentToolExecutor),
    /// Runs the tool calls one at a time, in order (TS `'sequential'`).
    Sequential(SequentialToolExecutor),
}

impl Default for ToolExecutor {
    fn default() -> Self {
        Self::Concurrent(ConcurrentToolExecutor::new())
    }
}

impl From<ConcurrentToolExecutor> for ToolExecutor {
    fn from(executor: ConcurrentToolExecutor) -> Self {
        Self::Concurrent(executor)
    }
}

impl From<SequentialToolExecutor> for ToolExecutor {
    fn from(executor: SequentialToolExecutor) -> Self {
        Self::Sequential(executor)
    }
}

impl ToolExecutor {
    #[allow(dead_code, reason = "the agent loop drives the executors")]
    pub(crate) fn execute(&self, input: ToolExecutionInput) -> ToolExecutorStream {
        match self {
            Self::Concurrent(executor) => executor.execute(input),
            Self::Sequential(executor) => executor.execute(input),
        }
    }
}

#[cfg(test)]
mod tests;
