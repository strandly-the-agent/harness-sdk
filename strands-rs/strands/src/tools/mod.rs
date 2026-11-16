//! Tools (TS `src/tools/` and `src/registry/`).

#[doc(hidden)]
pub mod executors;
pub mod function_tool;
pub(crate) mod macro_support;
pub mod registry;
pub(crate) mod schema;
#[doc(hidden)]
pub mod structured_output;
pub mod tool;

use std::sync::Arc;

use crate::agent::{Agent, AgentAsTool, AgentAsToolOptions};

#[doc(inline)]
pub use executors::{ConcurrentToolExecutor, SequentialToolExecutor, ToolExecutor};
#[doc(inline)]
pub use function_tool::FunctionTool;
#[doc(inline)]
pub use registry::ToolRegistry;
#[doc(inline)]
pub use structured_output::{STRUCTURED_OUTPUT_TOOL_NAME, StructuredOutputSchema};
#[doc(inline)]
pub use tool::{Tool, ToolContext, ToolError, ToolOutcome, ToolStream, ToolStreamEvent, ToolStreamItem};

/// Something `.tools(..)` accepts: a tool, an agent (as a tool), an MCP client or a list of
/// sources.
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
    /// An MCP client; the agent lists its tools in `Agent::initialize`.
    #[cfg(feature = "mcp")]
    McpClient(crate::mcp::McpClient),
}

impl ToolSource {
    /// The tools in order, with lists flattened.
    ///
    /// # Errors
    ///
    /// An agent in the list has a session manager, which the default tool options conflict with.
    pub(crate) fn into_tools(self) -> crate::Result<Vec<Arc<dyn Tool>>> {
        Ok(match self {
            Self::Tool(tool) => vec![tool],
            Self::List(sources) => {
                let mut tools = Vec::new();
                for source in sources {
                    tools.extend(source.into_tools()?);
                }
                tools
            }
            Self::Agent(tool) => {
                tool.check()?;
                vec![tool]
            }
            #[cfg(feature = "mcp")]
            Self::McpClient(_) => Vec::new(),
        })
    }

    /// The MCP clients in order, with lists flattened (TS `flattenTools`).
    #[cfg(feature = "mcp")]
    pub(crate) fn mcp_clients(&self) -> Vec<crate::mcp::McpClient> {
        match self {
            Self::McpClient(client) => vec![client.clone()],
            Self::List(sources) => sources.iter().flat_map(Self::mcp_clients).collect(),
            Self::Tool(_) | Self::Agent(_) => Vec::new(),
        }
    }
}

#[cfg(feature = "mcp")]
impl From<crate::mcp::McpClient> for ToolSource {
    fn from(client: crate::mcp::McpClient) -> Self {
        Self::McpClient(client)
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
        Self::Agent(Arc::new(AgentAsTool::new(agent, AgentAsToolOptions::default())))
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
