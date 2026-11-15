//! Tool executors: how the agent runs the tools of one model turn (TS `tools/executors/`).
//!
//! [`ConcurrentToolExecutor`] (the default) runs every tool of a turn at once, each on its own
//! task; [`SequentialToolExecutor`] runs them one after another in the order the model asked.
//! Both run each call through the same pipeline: `BeforeToolCallEvent` hooks (which may cancel
//! the call, rewrite its input or swap the tool), the tool itself, then `AfterToolCallEvent` hooks
//! (which may rewrite the result or ask for a retry). A tool that panics produces an error result.

mod concurrent;
pub(crate) mod executor;
mod sequential;

pub use concurrent::ConcurrentToolExecutor;
pub use sequential::SequentialToolExecutor;

/// Which executor runs an agent's tools (TS `ToolExecutorStrategy` plus the executor instances).
///
/// ```
/// use strands::tools::executors::{SequentialToolExecutor, ToolExecutor, ToolExecutorStrategy};
///
/// assert_eq!(ToolExecutor::from(ToolExecutorStrategy::Sequential), ToolExecutor::from(SequentialToolExecutor));
/// assert_eq!(ToolExecutor::default(), ToolExecutor::from(ToolExecutorStrategy::Concurrent));
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExecutor {
    /// Run a turn's tools at once.
    Concurrent(ConcurrentToolExecutor),
    /// Run a turn's tools one at a time.
    Sequential(SequentialToolExecutor),
}

impl Default for ToolExecutor {
    fn default() -> Self {
        Self::Concurrent(ConcurrentToolExecutor)
    }
}

/// The executor strategies by name (TS `'sequential' | 'concurrent'`).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExecutorStrategy {
    /// [`SequentialToolExecutor`].
    Sequential,
    /// [`ConcurrentToolExecutor`].
    Concurrent,
}

impl From<ToolExecutorStrategy> for ToolExecutor {
    fn from(strategy: ToolExecutorStrategy) -> Self {
        match strategy {
            ToolExecutorStrategy::Sequential => Self::Sequential(SequentialToolExecutor),
            ToolExecutorStrategy::Concurrent => Self::Concurrent(ConcurrentToolExecutor),
        }
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
