//! Vended tools shipped with the SDK (`strands-ts/src/vended-tools/`).

mod shell;

pub use shell::{
    ShellConfig, ShellError, ShellOutput, ShellTool, SANDBOX_SHELL_DESCRIPTION, SHELL_TOOL_NAME,
};
