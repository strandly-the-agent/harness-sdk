//! The harness `shell` tool (TS `buildBuiltinTools` in `builtin-tools.ts`): the SDK's stateless,
//! sandbox-routed shell, so its schema is just `command` and `timeout`.

use strands::tools::FunctionTool;
use strands::vended_tools::MakeShellOptions;

/// The `shell` built-in with the configured description (TS `ShellConfig.description`), or the
/// SDK's default description when `None`.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by builtin_tools, which lands separately")
)]
pub(crate) fn make_shell(description: Option<String>) -> crate::Result<FunctionTool> {
    Ok(strands::vended_tools::make_shell(MakeShellOptions {
        description,
        ..MakeShellOptions::default()
    })?)
}

#[cfg(test)]
mod tests;
