//! The sandbox-routed Python tool (TS `vended-tools/python-repl/`).

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::JsonValue;
use crate::error::{Error, Result};
use crate::sandbox::{ExecuteOptions, Sandbox, is_valid_language};
use crate::tools::function_tool::InputValidator;
use crate::tools::{FunctionTool, ToolError};

use super::shell::timeout_from_secs;
use super::{js_number, resolve_sandbox, schema_tool};

/// The model-facing description of the Python tool (TS `PYTHON_REPL_DESCRIPTION`).
pub const PYTHON_REPL_DESCRIPTION: &str = "Executes Python code and returns output (stdout), error (stderr), and exit_code (non-zero means the code failed). Each call runs in a fresh interpreter; variables, imports, and definitions do not persist across calls. Files written to the working directory persist while the sandbox is alive, so save intermediate results to files when later calls need them. stdin is not available (input() fails). Use print() to surface values.";

const DEFAULT_LANGUAGE: &str = "python3";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const TIMEOUT_EXIT_CODE: i32 = 124;

/// Options for [`make_python_repl`] (TS `MakePythonReplOptions` plus the bound sandbox).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(on(String, into), state_mod(vis = "pub"))]
pub struct MakePythonReplOptions {
    /// The tool name (default `python_repl`).
    pub name: Option<String>,
    /// The description (default [`PYTHON_REPL_DESCRIPTION`]).
    pub description: Option<String>,
    /// The interpreter (default `python3`); must match [`LANGUAGE_PATTERN`](crate::sandbox::LANGUAGE_PATTERN).
    pub language: Option<String>,
    /// Runs code in this sandbox instead of the agent's.
    pub sandbox: Option<Arc<dyn Sandbox>>,
}

/// What the Python tool returns (TS `PythonReplOutput`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PythonReplOutput {
    /// Standard output.
    pub output: String,
    /// Standard error.
    pub error: String,
    /// The exit code; non-zero means the code failed.
    pub exit_code: i32,
}

/// The sandbox failed to run the code (TS `PythonReplError`).
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
#[non_exhaustive]
pub struct PythonReplError {
    message: String,
    #[source]
    source: Option<Error>,
}

impl PythonReplError {
    /// The TS `error.name`.
    pub const NAME: &'static str = "PythonReplError";

    /// An error with `message`.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: None,
        }
    }

    /// Sets the underlying cause (TS `cause`).
    #[must_use]
    pub fn with_source(mut self, source: Error) -> Self {
        self.source = Some(source);
        self
    }
}

#[derive(Deserialize)]
struct PythonReplInput {
    code: String,
    timeout: Option<f64>,
}

fn python_repl_input_schema() -> JsonValue {
    serde_json::json!({
        "type": "object",
        "properties": {
            "code": { "type": "string", "description": "Python source to execute." },
            "timeout": {
                "description": "Timeout in seconds (default: 120). Must be positive.",
                "type": "number",
                "exclusiveMinimum": 0,
            },
        },
        "required": ["code"],
        "additionalProperties": false,
    })
}

/// The zod `positive` text, which the schema can't carry.
fn check_timeout(input: &JsonValue) -> Result<(), ToolError> {
    match input.get("timeout").and_then(JsonValue::as_f64) {
        Some(timeout) if timeout <= 0.0 => Err(ToolError::msg(format!(
            "Invalid input at /timeout: timeout must be a positive number of seconds, got {}",
            js_number(timeout)
        ))),
        _ => Ok(()),
    }
}

/// A Python tool (TS `makePythonRepl`): runs code through the sandbox's `execute_code` with the
/// invocation's cancel signal.
///
/// # Errors
///
/// `name` is empty, or `language` doesn't match [`LANGUAGE_PATTERN`](crate::sandbox::LANGUAGE_PATTERN).
pub fn make_python_repl(options: MakePythonReplOptions) -> Result<FunctionTool> {
    let name = options.name.unwrap_or_else(|| "python_repl".to_owned());
    if name.is_empty() {
        return Err(Error::other("name must be a non-empty string"));
    }
    let language = options.language.unwrap_or_else(|| DEFAULT_LANGUAGE.to_owned());
    if !is_valid_language(&language) {
        return Err(Error::other(format!(
            "language contains invalid characters: {language}"
        )));
    }
    let description = options
        .description
        .unwrap_or_else(|| PYTHON_REPL_DESCRIPTION.to_owned());
    Ok(build(name, description, language, options.sandbox))
}

/// The default Python tool (TS `pythonRepl`); runs code in the agent's sandbox.
pub fn python_repl() -> FunctionTool {
    build(
        "python_repl".to_owned(),
        PYTHON_REPL_DESCRIPTION.to_owned(),
        DEFAULT_LANGUAGE.to_owned(),
        None,
    )
}

fn build(
    name: String,
    description: String,
    language: String,
    bound: Option<Arc<dyn Sandbox>>,
) -> FunctionTool {
    let schema = python_repl_input_schema();
    schema_tool(
        name,
        description,
        schema.clone(),
        InputValidator::new(&schema),
        check_timeout,
        move |input: PythonReplInput, context| {
            let (bound, language) = (bound.clone(), language.clone());
            async move {
                let sandbox = resolve_sandbox(bound.as_ref(), &context)?;
                let timeout = match input.timeout {
                    Some(seconds) => timeout_from_secs(seconds)?,
                    None => DEFAULT_TIMEOUT,
                };
                let options = ExecuteOptions {
                    timeout: Some(timeout),
                    cancel_signal: Some(context.cancel_signal().clone()),
                    ..ExecuteOptions::default()
                };
                match sandbox.execute_code(&input.code, &language, options).await {
                    Ok(result) => Ok(PythonReplOutput {
                        output: result.stdout,
                        error: result.stderr,
                        exit_code: result.exit_code,
                    }),
                    Err(Error::SandboxAbort(error)) => Err(error.into()),
                    Err(Error::SandboxTimeout(error)) => {
                        let partial = PythonReplOutput {
                            output: error.stdout().to_owned(),
                            error: error.stderr().to_owned(),
                            exit_code: TIMEOUT_EXIT_CODE,
                        };
                        Err(ToolError::msg(format!(
                            "{error}\n{}",
                            serde_json::to_string(&partial)?
                        )))
                    }
                    Err(error) => Err(PythonReplError::new(error.to_string()).with_source(error).into()),
                }
            }
        },
    )
}

#[cfg(test)]
mod tests;
