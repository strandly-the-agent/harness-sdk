//! Vended tools shipped with the SDK (`strands-ts/src/vended-tools/`).

mod file_editor;
mod handoff_to_user;
#[cfg(feature = "http")]
mod http_request;
#[cfg(feature = "mcp")]
mod mcp_router;
mod shell;
mod sleep;

pub use file_editor::{
    FileEditorConfig, FileEditorTool, DEFAULT_FILE_EDITOR_DESCRIPTION, DEFAULT_MAX_FILE_SIZE,
    FILE_EDITOR_TOOL_NAME,
};
pub use handoff_to_user::{
    HandoffToUserConfig, HandoffToUserTool, DEFAULT_HANDOFF_TO_USER_DESCRIPTION, HANDOFF_INTERRUPT_NAME,
    HANDOFF_TO_USER_TOOL_NAME,
};
#[cfg(feature = "http")]
pub use http_request::{
    HttpRequestConfig, HttpRequestInput, HttpRequestOutput, HttpRequestTool,
    DEFAULT_HTTP_REQUEST_DESCRIPTION, DEFAULT_TIMEOUT_SECONDS, HTTP_REQUEST_TOOL_NAME,
};
#[cfg(feature = "mcp")]
pub use mcp_router::{
    McpRouterCommand, McpRouterConfig, McpRouterInput, McpRouterTool, DEFAULT_MAX_CONNECTIONS,
    MCP_ROUTER_DESCRIPTION, MCP_ROUTER_TOOL_NAME,
};
pub use shell::{
    ShellConfig, ShellError, ShellOutput, ShellTool, SANDBOX_SHELL_DESCRIPTION, SHELL_TOOL_NAME,
};
pub use sleep::{sleep_description, SleepConfig, SleepTool, DEFAULT_MAX_DURATION, SLEEP_TOOL_NAME};
