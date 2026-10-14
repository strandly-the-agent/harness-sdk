//! Tools: the `Tool` trait, the registry, and built-in tool types.

mod executor;
mod function_tool;
mod noop_tool;
mod registry;
mod structured_output;
mod tool;
pub mod vended;

pub use executor::{
    ConcurrentToolExecutor, ExecuteToolsInput, SequentialToolExecutor, ToolExecutor, ToolExecutorEvent,
    EXECUTION_CANCELLED_MESSAGE, HOOK_CANCELLED_MESSAGE,
};
pub use function_tool::{
    function_tool, input_schema_for, wrap_in_tool_result, FunctionTool, FunctionToolBuilder,
};
pub use noop_tool::{noop_tool_spec, NoopTool, NOOP_TOOL_DESCRIPTION, NOOP_TOOL_NAME};
pub use registry::{ToolRegistry, MAX_TOOL_NAME_LENGTH};
pub use structured_output::{StructuredOutputTool, STRUCTURED_OUTPUT_TOOL_NAME};
pub use tool::{
    create_error_result, is_valid_tool_name, tool_error_or_propagate, Tool, ToolContext, ToolRef,
    ToolStreamEvent,
};
