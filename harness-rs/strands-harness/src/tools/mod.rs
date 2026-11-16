//! The harness's built-in tools (TS `tools/`).

mod file_tools;
mod programmatic_tool_caller;
mod shell;
mod subagent;
mod web_fetch;
mod web_search;

#[expect(unused_imports, reason = "used by builtin_tools, which lands separately")]
pub(crate) use file_tools::{MakeReadOptions, make_read};
pub use file_tools::{edit, read, write};
#[expect(unused_imports, reason = "used by builtin_tools, which lands separately")]
pub(crate) use shell::make_shell;
pub use web_fetch::{MakeWebFetchOptions, make_web_fetch};
#[cfg(feature = "mcp")]
pub use web_search::{exa_web_search, make_exa_web_search};
