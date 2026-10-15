//! Vended tools shipped with the SDK (`strands-ts/src/vended-tools/`).

mod handoff_to_user;
mod shell;
mod sleep;

pub use handoff_to_user::{
    HandoffToUserConfig, HandoffToUserTool, DEFAULT_HANDOFF_TO_USER_DESCRIPTION, HANDOFF_INTERRUPT_NAME,
    HANDOFF_TO_USER_TOOL_NAME,
};
pub use shell::{
    ShellConfig, ShellError, ShellOutput, ShellTool, SANDBOX_SHELL_DESCRIPTION, SHELL_TOOL_NAME,
};
pub use sleep::{
    sleep_description, SleepConfig, SleepTool, DEFAULT_MAX_DURATION, SLEEP_TOOL_NAME,
};
