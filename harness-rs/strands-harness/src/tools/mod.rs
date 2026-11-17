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
    MakeProgrammaticToolCallerOptionsBuilder, make_programmatic_tool_caller,
    make_programmatic_tool_caller_options_builder, programmatic_tool_caller,
};
pub(crate) use shell::make_shell;
pub use subagent::{
    AgentBuilder, AgentSpec, Choice, ChoiceOption, ContextAxis, ContextMode, Fixed, Inherit,
    InstructionsMode, MakeSubagentOptions, McpServersMode, ModelMode, Open, Preset, PresetOptions, ToolsMode,
    make_subagent,
};
pub(crate) use subagent::{CONTEXT_MODES, GENERALIST};
pub use subagent::{
    MakeSubagentOptionsBuilder, PresetBuilder, make_subagent_options_builder, preset_builder,
};
pub use web_fetch::{
    MakeWebFetchOptions, MakeWebFetchOptionsBuilder, make_web_fetch, make_web_fetch_options_builder,
};
#[cfg(feature = "mcp")]
pub use web_search::{exa_web_search, make_exa_web_search};
