//! What `.tools(..)` accepts: tools and lists of them.

use std::sync::Arc;

use crate::tools::tool::Tool;

/// Anything that provides tools to an agent's `.tools(..)`.
///
/// Built through `From`: a tool, an `Arc<dyn Tool>`, or a `Vec` of anything convertible. A list
/// mixing tool types needs [`tool_list!`](crate::tool_list) or [`ToolSource::from`], because
/// `.into()` has no single target there.
///
/// ```
/// use strands::tools::FunctionTool;
/// use strands::{ToolSource, tool_list};
///
/// let ping = FunctionTool::new("ping", "Replies pong.", |_input, _ctx| async { Ok("pong") });
/// let typed = FunctionTool::typed("add", "Adds.", |(a, b): (i64, i64), _ctx| async move { Ok(a + b) });
/// let sources: Vec<ToolSource> = tool_list![ping, typed];
/// assert_eq!(sources.len(), 2);
/// ```
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum ToolSource {
    /// One tool.
    Tool(Arc<dyn Tool>),
    /// Several sources, in order.
    List(Vec<ToolSource>),
}

impl ToolSource {
    /// The tools this source holds, in order.
    pub(crate) fn into_tools(self) -> Vec<Arc<dyn Tool>> {
        match self {
            Self::Tool(tool) => vec![tool],
            Self::List(sources) => sources.into_iter().flat_map(Self::into_tools).collect(),
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

/// Builds a `Vec<ToolSource>` from tools of different types.
///
/// ```
/// use strands::tools::FunctionTool;
/// use strands::{ToolSource, tool_list};
///
/// let ping = FunctionTool::new("ping", "Replies pong.", |_input, _ctx| async { Ok("pong") });
/// let sources: Vec<ToolSource> = tool_list![ping];
/// # let _ = sources;
/// ```
#[macro_export]
macro_rules! tool_list {
    ($($source:expr),* $(,)?) => {
        ::std::vec![$($crate::ToolSource::from($source)),*]
    };
}
