//! Tools (TS `tools/` and `registry/`).
//!
//! A [`Tool`] streams progress events and ends with one [`ToolOutcome`]. Build tools with
//! [`#[tool]`](macro@crate::tool) on a function, [`#[tools]`](macro@crate::tools) on an `impl` block,
//! [`FunctionTool`], or by implementing [`Tool`]. A failing tool produces an error-status result
//! the model sees as `Error: <message>`; it never fails the invocation.

pub mod context;
#[cfg(test)]
pub(crate) mod fixtures;
pub mod function_tool;
#[cfg(test)]
mod macro_tests;
pub mod registry;
pub(crate) mod schema;
pub mod tool;
pub mod tool_source;

pub use context::ToolContext;
pub use function_tool::FunctionTool;
pub use registry::ToolRegistry;
pub use tool::{
    Tool, ToolError, ToolOutcome, ToolStream, ToolStreamEvent, ToolStreamItem, is_valid_tool_name,
};
pub use tool_source::ToolSource;
