//! The harness's built-in tools (TS `tools/`).

mod file_tools;
mod programmatic_tool_caller;
mod shell;
mod subagent;
mod web_fetch;
mod web_search;

pub(crate) use file_tools::{MakeReadOptions, make_read};
pub use file_tools::{edit, read, write};
pub use programmatic_tool_caller::{
    DEFAULT_PROGRAMMATIC_TOOL_CALLER_DESCRIPTION, MakeProgrammaticToolCallerOptions,
    make_programmatic_tool_caller, programmatic_tool_caller,
};
pub(crate) use shell::make_shell;
pub use subagent::{
    AgentBuilder, AgentSpec, Choice, ChoiceOption, ContextAxis, ContextMode, Fixed, Inherit,
    InstructionsMode, MakeSubagentOptions, McpServersMode, ModelMode, Open, Preset, PresetOptions, ToolsMode,
    make_subagent,
};
pub(crate) use subagent::{CONTEXT_MODES, GENERALIST};
pub use web_fetch::{MakeWebFetchOptions, make_web_fetch};
#[cfg(feature = "mcp")]
pub use web_search::{exa_web_search, make_exa_web_search};
