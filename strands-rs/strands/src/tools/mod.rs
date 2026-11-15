//! Tools (TS `src/tools/` and `src/registry/`).

pub mod executors;
pub mod function_tool;
pub(crate) mod macro_support;
pub mod registry;
pub(crate) mod schema;
pub mod structured_output;
pub mod tool;

use std::sync::Arc;

use crate::agent::{Agent, AgentAsTool, AgentAsToolOptions};

pub use executors::{ConcurrentToolExecutor, SequentialToolExecutor, ToolExecutor};
pub use function_tool::FunctionTool;
pub use registry::ToolRegistry;
pub use structured_output::{STRUCTURED_OUTPUT_TOOL_NAME, StructuredOutputSchema};
pub use tool::{Tool, ToolContext, ToolError, ToolOutcome, ToolStream, ToolStreamEvent, ToolStreamItem};

/// Something `.tools(..)` accepts: a tool, an agent (as a tool) or a list of sources.
///
/// Mixed lists need [`ToolSource::from`] or [`tool_list!`](crate::tool_list), because `.into()`
/// has no single target there.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ToolSource {
    /// One tool.
    Tool(Arc<dyn Tool>),
    /// Several sources, flattened in order.
    List(Vec<ToolSource>),
    /// An agent, as a tool (TS `AgentAsTool`).
    Agent(Arc<AgentAsTool>),
}

impl ToolSource {
    /// The tools in order, with lists flattened.
    pub(crate) fn into_tools(self) -> Vec<Arc<dyn Tool>> {
        match self {
            Self::Tool(tool) => vec![tool],
            Self::List(sources) => sources.into_iter().flat_map(Self::into_tools).collect(),
            Self::Agent(tool) => vec![tool],
        }
    }
}

impl<T: Tool> From<T> for ToolSource {
    fn from(tool: T) -> Self {
        Self::Tool(Arc::new(tool))
    }
}

/// An agent becomes a tool with default [`AgentAsToolOptions`]
/// (TS wraps agents in `tools` with `asTool()`).
impl From<Agent> for ToolSource {
    fn from(agent: Agent) -> Self {
        Self::Agent(Arc::new(agent.as_tool(AgentAsToolOptions::default())))
    }
}

impl From<Arc<dyn Tool>> for ToolSource {
    fn from(tool: Arc<dyn Tool>) -> Self {
        Self::Tool(tool)
    }
}

impl<T: Tool> From<Arc<T>> for ToolSource {
    fn from(tool: Arc<T>) -> Self {
        Self::Tool(tool)
    }
}

impl<T: Into<ToolSource>> From<Vec<T>> for ToolSource {
    fn from(sources: Vec<T>) -> Self {
        Self::List(sources.into_iter().map(Into::into).collect())
    }
}

/// Builds a `Vec<ToolSource>` from tools of different types.
///
/// ```
/// use strands::tool_list;
/// use strands::tools::{FunctionTool, ToolSource};
///
/// let ping = FunctionTool::new("ping", "Ping.", |_, _| async { Ok::<_, strands::ToolError>("pong") });
/// let echo = FunctionTool::new("echo", "Echo.", |input, _| async move { Ok::<_, strands::ToolError>(input) });
/// let tools: Vec<ToolSource> = tool_list![ping, echo];
/// assert_eq!(tools.len(), 2);
/// ```
#[macro_export]
macro_rules! tool_list {
    ($($tool:expr),* $(,)?) => {
        ::std::vec![$($crate::tools::ToolSource::from($tool)),*]
    };
}
