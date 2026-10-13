//! Tools: the `Tool` trait, the registry, and built-in tool types.

mod registry;
mod tool;

pub use registry::{ToolRegistry, MAX_TOOL_NAME_LENGTH};
pub use tool::{create_error_result, is_valid_tool_name, Tool, ToolContext, ToolRef, ToolStreamEvent};
