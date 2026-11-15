//! The sandbox-routed shell tool (TS `vended-tools/shell/`).

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::JsonValue;
use crate::error::{Error, Result};
use crate::sandbox::{ExecuteOptions, Sandbox};
use crate::tools::FunctionTool;
use crate::tools::function_tool::InputValidator;

use super::{no_precheck, resolve_sandbox, schema_tool};

/// The model-facing description of the shell tool (TS `SANDBOX_SHELL_DESCRIPTION`).
pub const SANDBOX_SHELL_DESCRIPTION: &str = "Executes shell commands and returns output (stdout), error (stderr), and exit_code (non-zero means the command failed). Each call runs in a fresh shell; state such as variables and the working directory does not persist across calls.";

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
/// The `timeout(1)` exit code for a command killed by its time limit.
const TIMEOUT_EXIT_CODE: i32 = 124;

/// Options for [`make_shell`] (TS `MakeShellOptions` plus the bound sandbox).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(on(String, into))]
pub struct MakeShellOptions {
    /// The tool name (default `shell`).
    pub name: Option<String>,
    /// The description (default [`SANDBOX_SHELL_DESCRIPTION`]).
    pub description: Option<String>,
    /// Replaces the input schema; input is validated against it.
    pub input_schema: Option<JsonValue>,
    /// The timeout when the model omits `timeout` (default 120 s).
    pub default_timeout: Option<Duration>,
    /// Runs commands in this sandbox instead of the agent's.
    pub sandbox: Option<Arc<dyn Sandbox>>,
}

/// What the shell tool returns (TS `ShellOutput`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ShellOutput {
    /// Standard output.
    pub output: String,
    /// Standard error; empty when there was none.
    pub error: String,
    /// The exit code; non-zero means the command failed.
    pub exit_code: i32,
}

impl ShellOutput {
    /// An output.
    pub fn new(output: impl Into<String>, error: impl Into<String>, exit_code: i32) -> Self {
        Self {
            output: output.into(),
            error: error.into(),
            exit_code,
        }
    }
}

/// A shell command ran past its timeout (TS `ShellTimeoutError`). The message ends with the
/// output so far as JSON, so the model sees it.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
#[non_exhaustive]
pub struct ShellTimeoutError {
    message: String,
    partial: Option<ShellOutput>,
}

impl ShellTimeoutError {
    /// The TS `error.name`.
    pub const NAME: &'static str = "ShellTimeoutError";

    /// An error with `message` and the output captured before the kill.
    pub fn new(message: impl Into<String>, partial: Option<ShellOutput>) -> Self {
        Self {
            message: message.into(),
            partial,
        }
    }

    /// The output captured before the command was killed.
    pub fn partial(&self) -> Option<&ShellOutput> {
        self.partial.as_ref()
    }
}

/// A shell command failed to run (TS `ShellExecutionError`).
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
#[non_exhaustive]
pub struct ShellExecutionError {
    message: String,
}

impl ShellExecutionError {
    /// The TS `error.name`.
    pub const NAME: &'static str = "ShellExecutionError";

    /// An error with `message`.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[derive(Deserialize)]
struct ShellInput {
    command: String,
    timeout: Option<f64>,
}

/// The input schema for a default timeout, as `z.toJSONSchema` writes it.
fn shell_input_schema(default_timeout: Duration) -> JsonValue {
    serde_json::json!({
        "type": "object",
        "properties": {
            "command": { "type": "string", "description": "The shell command to execute." },
            "timeout": {
                "default": seconds_json(default_timeout),
                "description": "Optional per-execution timeout in seconds.",
                "type": "number",
                "exclusiveMinimum": 0,
            },
        },
        "required": ["command"],
        "additionalProperties": false,
    })
}

/// Seconds as a JSON number: an integer when whole, so it never gains `.0`.
fn seconds_json(duration: Duration) -> JsonValue {
    if duration.subsec_nanos() == 0 {
        return duration.as_secs().into();
    }
    serde_json::Number::from_f64(duration.as_secs_f64()).map_or(JsonValue::Null, JsonValue::Number)
}

/// Converts a model-supplied timeout in seconds.
pub(crate) fn timeout_from_secs(seconds: f64) -> Result<Duration> {
    Duration::try_from_secs_f64(seconds).map_err(|_| Error::other(format!("Invalid timeout: {seconds}")))
}

/// A shell tool (TS `makeShell`). Each call runs `command` in a fresh shell of the bound sandbox
/// or, when none is bound, the agent's sandbox.
///
/// # Errors
///
/// `default_timeout` is zero.
///
/// ```
/// use std::time::Duration;
/// use strands::vended_tools::{MakeShellOptions, make_shell};
///
/// let shell = make_shell(MakeShellOptions {
///     default_timeout: Some(Duration::from_secs(300)),
///     ..Default::default()
/// })?;
/// # let _ = shell;
/// # Ok::<(), strands::Error>(())
/// ```
pub fn make_shell(options: MakeShellOptions) -> Result<FunctionTool> {
    let default_timeout = options.default_timeout.unwrap_or(DEFAULT_TIMEOUT);
    if default_timeout.is_zero() {
        return Err(Error::other(format!(
            "defaultTimeout must be a positive finite number, got {}",
            default_timeout.as_secs_f64()
        )));
    }
    let schema = options
        .input_schema
        .unwrap_or_else(|| shell_input_schema(default_timeout));
    let bound = options.sandbox;
    Ok(schema_tool(
        options.name.unwrap_or_else(|| "shell".to_owned()),
        options
            .description
            .unwrap_or_else(|| SANDBOX_SHELL_DESCRIPTION.to_owned()),
        schema.clone(),
        InputValidator::new(&schema),
        no_precheck,
        move |input: ShellInput, context| {
            let bound = bound.clone();
            async move {
                let sandbox = resolve_sandbox(bound.as_ref(), &context)?;
                let timeout = match input.timeout {
                    Some(seconds) => timeout_from_secs(seconds)?,
                    None => default_timeout,
                };
                let options = ExecuteOptions {
                    timeout: Some(timeout),
                    ..ExecuteOptions::default()
                };
                match sandbox.execute(&input.command, options).await {
                    Ok(result) => Ok(ShellOutput::new(result.stdout, result.stderr, result.exit_code)),
                    Err(Error::SandboxTimeout(error)) => {
                        let partial = ShellOutput::new(error.stdout(), error.stderr(), TIMEOUT_EXIT_CODE);
                        let message = format!("{error}\n{}", serde_json::to_string(&partial)?);
                        Err(ShellTimeoutError::new(message, Some(partial)).into())
                    }
                    Err(error) => Err(ShellExecutionError::new(error.to_string()).into()),
                }
            }
        },
    ))
}

#[cfg(test)]
mod tests;
