//! Vended tools shipped with the SDK (`strands-ts/src/vended-tools/`).

#[cfg(feature = "a2a")]
mod a2a_client;
mod shell;

#[cfg(feature = "a2a")]
pub use a2a_client::{
    A2AClientOperation, A2AClientTool, A2AClientToolBuilder, A2A_CLIENT_TOOL_NAME,
    DEFAULT_A2A_CLIENT_DESCRIPTION,
};
pub use shell::{
    ShellConfig, ShellError, ShellOutput, ShellTool, SANDBOX_SHELL_DESCRIPTION, SHELL_TOOL_NAME,
};
