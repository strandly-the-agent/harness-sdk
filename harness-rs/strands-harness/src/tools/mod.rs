//! The harness's built-in tools (TS `tools/`).

mod file_tools;
mod programmatic_tool_caller;
mod shell;
mod subagent;
mod web_fetch;
mod web_search;

pub use file_tools::{MakeReadOptions, edit, make_read, read, write};
pub use web_fetch::{MakeWebFetchOptions, make_web_fetch};
#[cfg(feature = "mcp")]
pub use web_search::{exa_web_search, make_exa_web_search};
