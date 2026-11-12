//! Tools (TS `tools/`, `registry/`).

mod context;
mod error;
pub(crate) mod executors;
pub(crate) mod function_tool;
mod registry;
pub(crate) mod schema;
mod source;
pub(crate) mod structured_output;
pub(crate) mod tool;
pub(crate) mod types;

pub use context::ToolContext;
pub use error::ToolError;
pub use executors::{ConcurrentToolExecutor, SequentialToolExecutor, ToolExecutor};
pub use function_tool::{FunctionTool, FunctionToolBuilder, InputValidator};
pub use registry::ToolRegistry;
pub use schema::schema_for;
pub use source::ToolSource;
pub use structured_output::{NO_STRUCTURED_OUTPUT_MESSAGE, STRUCTURED_OUTPUT_TOOL_NAME};
pub use tool::{
    MAX_TOOL_NAME_LENGTH, Tool, ToolOutcome, ToolStream, ToolStreamEvent, ToolStreamItem, is_valid_tool_name,
};
pub use types::{ToolChoice, ToolResultStatus, ToolSpec, ToolUse, UnknownToolResultStatus};
