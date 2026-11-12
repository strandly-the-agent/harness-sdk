//! Tool sources: what `.tools(..)` accepts.

use std::fmt;
use std::sync::Arc;

use crate::agent::Agent;
use crate::tools::Tool;

/// Something that provides tools to an agent: a tool, a list, or an agent (wrapped with
/// [`Agent::as_tool`]).
///
/// Mixed lists need [`tool_list!`](crate::tool_list) or `ToolSource::from`, because `.into()`
/// has no single target there.
#[non_exhaustive]
pub enum ToolSource {
    /// One tool.
    Tool(Arc<dyn Tool>),
    /// Several sources, flattened in order.
    List(Vec<ToolSource>),
    /// An agent, registered as a tool with default options.
    Agent(Box<Agent>),
}

impl ToolSource {
    pub(crate) fn flatten(self, tools: &mut Vec<Arc<dyn Tool>>) {
        match self {
            Self::Tool(tool) => tools.push(tool),
            Self::List(sources) => sources.into_iter().for_each(|source| source.flatten(tools)),
            Self::Agent(agent) => tools.push(Arc::new(
                agent.as_tool(crate::agent::AgentAsToolOptions::default()),
            )),
        }
    }
}

impl<T: Tool> From<T> for ToolSource {
    fn from(tool: T) -> Self {
        Self::Tool(Arc::new(tool))
    }
}

impl From<Arc<dyn Tool>> for ToolSource {
    fn from(tool: Arc<dyn Tool>) -> Self {
        Self::Tool(tool)
    }
}

impl<T: Into<ToolSource>> From<Vec<T>> for ToolSource {
    fn from(sources: Vec<T>) -> Self {
        Self::List(sources.into_iter().map(Into::into).collect())
    }
}

impl From<Agent> for ToolSource {
    fn from(agent: Agent) -> Self {
        Self::Agent(Box::new(agent))
    }
}

impl fmt::Debug for ToolSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tool(tool) => f.debug_tuple("Tool").field(&tool.name()).finish(),
            Self::List(sources) => f.debug_tuple("List").field(sources).finish(),
            Self::Agent(agent) => f.debug_tuple("Agent").field(&agent.name()).finish(),
        }
    }
}

/// Builds a `Vec<ToolSource>` from tools, lists and agents of different types.
///
/// ```
/// use strands::tools::FunctionTool;
/// use strands::{tool_list, ToolSource};
///
/// let a = FunctionTool::builder("a", "First.").callback(|_, _| async { Ok(serde_json::json!(1)) });
/// let b = FunctionTool::builder("b", "Second.").callback(|_, _| async { Ok(serde_json::json!(2)) });
/// let tools: Vec<ToolSource> = tool_list![a, vec![b]];
/// assert_eq!(tools.len(), 2);
/// ```
#[macro_export]
macro_rules! tool_list {
    ($($source:expr),* $(,)?) => {
        ::std::vec![$($crate::ToolSource::from($source)),*]
    };
}
