//! The `shell` tool (`strands-ts/src/vended-tools/shell/make-shell.ts`).
//!
//! Stateless: every call runs the command in a fresh `sh -c` process, so variables and the
//! working directory do not survive across calls. Like TS, the command is routed through a
//! [`Sandbox`]: the tool's own sandbox if it was vended by one, else the invoking agent's
//! ([`crate::Agent::sandbox`]), else a [`LocalSandbox`] built from [`ShellConfig`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::sandbox::{ExecuteOptions, LocalSandbox, Sandbox, SandboxError, SandboxRef};
use crate::tools::tool::{create_error_result, Tool, ToolContext};
use crate::tools::wrap_in_tool_result;
use crate::types::{ToolResultBlock, ToolSpec};

/// Name of the shell tool.
pub const SHELL_TOOL_NAME: &str = "shell";

/// Description of the shell tool (`SANDBOX_SHELL_DESCRIPTION` in TS).
pub const SANDBOX_SHELL_DESCRIPTION: &str = "Executes shell commands and returns output (stdout), error (stderr), and exit_code (non-zero means the command failed). Each call runs in a fresh shell; state such as variables and the working directory does not persist across calls.";

/// Exit code reported for a command killed by its timeout (the `timeout(1)` convention).
const TIMEOUT_EXIT_CODE: i32 = 124;

/// Result of a shell command (`ShellOutput` in TS).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ShellOutput {
    /// Standard output.
    pub output: String,
    /// Standard error.
    pub error: String,
    /// Process exit code; non-zero means the command failed.
    pub exit_code: i32,
}

/// Why a shell command could not be run to completion.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ShellError {
    /// The command exceeded its timeout and was killed (`ShellTimeoutError` in TS).
    ///
    /// The message carries the output captured before the kill, JSON-encoded on a second line,
    /// because a tool error reaches the model as text.
    #[error("Execution timed out after {seconds} seconds\n{}", serde_json::to_string(partial).unwrap_or_default())]
    Timeout {
        /// Timeout in seconds, as requested.
        seconds: f64,
        /// Output captured before the command was killed, with `exit_code` 124.
        partial: ShellOutput,
    },
    /// The command could not be spawned or its output could not be read
    /// (`ShellExecutionError` in TS).
    #[error("{0}")]
    Execution(String),
}

/// Configuration of a [`ShellTool`].
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ShellConfig {
    /// Default timeout when the model does not pass one (TS default: 120 seconds).
    pub timeout: Duration,
    /// Working directory; inherited from the process when `None`.
    pub cwd: Option<PathBuf>,
    /// Extra environment variables layered on top of the inherited environment.
    pub env: Option<HashMap<String, String>>,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(120),
            cwd: None,
            env: None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ShellInput {
    command: String,
    #[serde(default)]
    timeout: Option<f64>,
}

/// The stateless `shell` tool: runs each command in a fresh `sh -c` process.
///
/// Output is never truncated (the TS tool has no cap either), so a chatty command can return a
/// very large result.
#[derive(Debug)]
pub struct ShellTool {
    config: ShellConfig,
    sandbox: Option<SandboxRef>,
    spec: ToolSpec,
}

impl Default for ShellTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellTool {
    /// Shell tool with the default configuration (120 second timeout).
    pub fn new() -> Self {
        Self::with_config(ShellConfig::default())
    }

    /// Shell tool with an explicit configuration.
    pub fn with_config(config: ShellConfig) -> Self {
        Self {
            config,
            sandbox: None,
            spec: shell_tool_spec(),
        }
    }

    /// Shell tool that always runs through `sandbox` (how a sandbox vends its own shell tool).
    pub fn with_sandbox(sandbox: SandboxRef) -> Self {
        Self {
            sandbox: Some(sandbox),
            ..Self::new()
        }
    }

    fn fallback_sandbox(&self) -> SandboxRef {
        let mut sandbox = match &self.config.cwd {
            Some(cwd) => LocalSandbox::with_base_dir(cwd.clone()),
            None => LocalSandbox::new(),
        };
        if let Some(env) = &self.config.env {
            sandbox = sandbox.with_env(env.clone());
        }
        Arc::new(sandbox)
    }

    /// Run `command`, killing it after `timeout` (defaults to [`ShellConfig::timeout`]).
    pub async fn execute(
        &self,
        command: &str,
        timeout: Option<f64>,
    ) -> std::result::Result<ShellOutput, ShellError> {
        self.execute_in(self.fallback_sandbox().as_ref(), command, timeout)
            .await
    }

    /// Run `command` through `sandbox`, mapping sandbox failures onto [`ShellError`].
    pub async fn execute_in(
        &self,
        sandbox: &dyn Sandbox,
        command: &str,
        timeout: Option<f64>,
    ) -> std::result::Result<ShellOutput, ShellError> {
        let seconds = timeout.unwrap_or(self.config.timeout.as_secs_f64());
        let options = ExecuteOptions {
            timeout: Some(Duration::from_secs_f64(seconds)),
            ..ExecuteOptions::default()
        };
        match sandbox.execute(command, options).await {
            Ok(result) => Ok(ShellOutput {
                output: result.stdout,
                error: result.stderr,
                exit_code: result.exit_code,
            }),
            Err(SandboxError::Timeout {
                seconds,
                stdout,
                stderr,
            }) => Err(ShellError::Timeout {
                seconds,
                partial: ShellOutput {
                    output: stdout,
                    error: stderr,
                    exit_code: TIMEOUT_EXIT_CODE,
                },
            }),
            Err(error) => Err(ShellError::Execution(error.to_string())),
        }
    }
}

/// Spec of the shell tool.
fn shell_tool_spec() -> ToolSpec {
    ToolSpec {
        name: SHELL_TOOL_NAME.to_owned(),
        description: SANDBOX_SHELL_DESCRIPTION.to_owned(),
        input_schema: Some(serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The shell command to execute." },
                "timeout": {
                    "type": "number",
                    "exclusiveMinimum": 0,
                    "description": "Timeout in seconds (default: 120).",
                },
            },
            "required": ["command"],
            "additionalProperties": false,
        })),
        output_schema: None,
        annotations: None,
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        &self.spec.name
    }

    fn description(&self) -> &str {
        &self.spec.description
    }

    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn invoke(&self, ctx: ToolContext) -> Result<ToolResultBlock> {
        let input: ShellInput = ctx.input()?;
        if input.timeout.is_some_and(|t| !(t.is_finite() && t > 0.0)) {
            return Ok(create_error_result(
                "timeout must be a positive number of seconds",
                &ctx.tool_use.tool_use_id,
            ));
        }

        let sandbox = match self.sandbox.clone() {
            Some(sandbox) => sandbox,
            None => match ctx.agent.sandbox() {
                Ok(sandbox) => sandbox,
                Err(_) => self.fallback_sandbox(),
            },
        };
        let run = self.execute_in(sandbox.as_ref(), &input.command, input.timeout);
        let outcome = tokio::select! {
            biased;
            () = ctx.cancel.cancelled() => return Err(Error::Cancelled),
            outcome = run => outcome,
        };

        match outcome {
            Ok(output) => Ok(wrap_in_tool_result(
                serde_json::to_value(output)?,
                &ctx.tool_use.tool_use_id,
            )),
            Err(err) => Ok(create_error_result(err, &ctx.tool_use.tool_use_id)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::tool::test_context;
    use crate::types::{ToolResultContent, ToolResultStatus};

    fn ctx(input: serde_json::Value) -> ToolContext {
        test_context(SHELL_TOOL_NAME, "test-id", input)
    }

    async fn run(command: &str) -> ShellOutput {
        let result = ShellTool::new()
            .invoke(ctx(serde_json::json!({ "command": command })))
            .await
            .unwrap();
        assert_eq!(result.status, ToolResultStatus::Success);
        match &result.content[0] {
            ToolResultContent::Json(value) => serde_json::from_value(value.clone()).unwrap(),
            other => panic!("expected json content, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn executes_command_via_sandbox() {
        let out = run("echo \"hello sandbox\"").await;
        assert!(out.output.contains("hello sandbox"));
        assert_eq!(out.error, "");
        assert_eq!(out.exit_code, 0);
    }

    #[tokio::test]
    async fn a_timeout_kills_the_whole_process_group() {
        let started = std::time::Instant::now();
        let error = ShellTool::new()
            .execute("sleep 4 & sleep 10", Some(0.5))
            .await
            .unwrap_err();
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(2),
            "the call waited for a grandchild to release the pipes: {elapsed:?}"
        );
        match error {
            ShellError::Timeout { partial, seconds } => {
                assert_eq!(partial.exit_code, TIMEOUT_EXIT_CODE);
                assert_eq!(seconds, 0.5);
            }
            other => panic!("expected a timeout, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_cancelled_call_kills_the_whole_process_group() {
        let context = ctx(serde_json::json!({ "command": "sleep 4 & sleep 10" }));
        let cancel = context.cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            cancel.cancel();
        });
        let started = std::time::Instant::now();
        let error = ShellTool::new().invoke(context).await.unwrap_err();
        let elapsed = started.elapsed();
        assert!(matches!(error, Error::Cancelled), "{error}");
        assert!(
            elapsed < Duration::from_secs(2),
            "the cancelled call waited for the process tree: {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn reports_a_non_zero_exit_code() {
        assert_eq!(run("exit 42").await.exit_code, 42);
    }

    #[tokio::test]
    async fn captures_stderr_via_sandbox() {
        let out = run("echo \"oops\" >&2").await;
        assert!(out.error.contains("oops"));
        assert_eq!(out.exit_code, 0);
    }

    #[tokio::test]
    async fn does_not_persist_state_between_calls_stateless() {
        run("export MY_VAR=hello").await;
        assert_eq!(run("echo \"${MY_VAR:-empty}\"").await.output.trim(), "empty");
    }

    #[tokio::test]
    async fn respects_timeout() {
        let err = ShellTool::new().execute("sleep 10", Some(0.1)).await.unwrap_err();
        assert!(matches!(err, ShellError::Timeout { .. }));
    }

    #[tokio::test]
    async fn throws_shell_timeout_error_on_timeout() {
        let err = ShellTool::new().execute("sleep 10", Some(0.1)).await.unwrap_err();
        match err {
            ShellError::Timeout { seconds, partial } => {
                assert_eq!(seconds, 0.1);
                assert_eq!(partial.exit_code, TIMEOUT_EXIT_CODE);
            }
            other => panic!("expected a timeout, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn timeout_error_carries_the_partial_output_with_the_same_field_names_as_a_success_result() {
        let err = ShellTool::new()
            .execute("echo partial; echo warn >&2; sleep 5", Some(0.3))
            .await
            .unwrap_err();
        let ShellError::Timeout { partial, .. } = &err else {
            panic!("expected a timeout, got {err:?}")
        };
        assert_eq!(
            partial,
            &ShellOutput {
                output: "partial\n".to_owned(),
                error: "warn\n".to_owned(),
                exit_code: 124,
            }
        );
        assert_eq!(
            err.to_string(),
            format!(
                "Execution timed out after 0.3 seconds\n{}",
                serde_json::to_string(partial).unwrap()
            )
        );
    }

    #[tokio::test]
    async fn timeout_surfaces_as_a_failed_tool_result_that_still_includes_the_partial_output() {
        let result = ShellTool::new()
            .invoke(ctx(serde_json::json!({
                "command": "echo partial; sleep 5",
                "timeout": 0.3,
            })))
            .await
            .unwrap();

        assert_eq!(result.status, ToolResultStatus::Error);
        let text = result.text();
        let (first, second) = text.split_once('\n').unwrap();
        assert_eq!(first, "Error: Execution timed out after 0.3 seconds");
        assert_eq!(
            serde_json::from_str::<ShellOutput>(second).unwrap(),
            ShellOutput {
                output: "partial\n".to_owned(),
                error: String::new(),
                exit_code: 124,
            }
        );
    }

    #[test]
    fn exposes_only_command_and_timeout_with_no_mode_discriminator() {
        let tool = ShellTool::new();
        let schema = tool.spec().input_schema.clone().unwrap();
        let mut keys: Vec<&String> = schema["properties"].as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, ["command", "timeout"]);
        assert_eq!(schema["required"], serde_json::json!(["command"]));
        assert!(schema["properties"].get("mode").is_none());
    }

    #[test]
    fn spec_matches_the_ts_name_and_description() {
        let tool = ShellTool::new();
        assert_eq!(tool.name(), "shell");
        assert_eq!(tool.description(), SANDBOX_SHELL_DESCRIPTION);
        assert_eq!(tool.spec(), &shell_tool_spec());
    }

    #[test]
    fn defaults_to_a_120_second_timeout() {
        assert_eq!(ShellConfig::default().timeout, Duration::from_secs(120));
    }

    #[tokio::test]
    async fn runs_in_the_configured_cwd_with_the_configured_env() {
        let dir = std::env::temp_dir();
        let tool = ShellTool::with_config(ShellConfig {
            cwd: Some(dir.clone()),
            env: Some(HashMap::from([("MY_VAR".to_owned(), "set".to_owned())])),
            ..ShellConfig::default()
        });
        let out = tool.execute("pwd; echo \"$MY_VAR\"", None).await.unwrap();
        assert!(out.output.contains(dir.to_str().unwrap()));
        assert!(out.output.contains("set"));
    }

    #[tokio::test]
    async fn cancellation_aborts_the_command() {
        let context = ctx(serde_json::json!({ "command": "sleep 10" }));
        context.cancel.cancel();
        let err = ShellTool::new().invoke(context).await.unwrap_err();
        assert!(matches!(err, Error::Cancelled));
    }

    #[tokio::test]
    async fn rejects_a_non_positive_timeout() {
        let result = ShellTool::new()
            .invoke(ctx(serde_json::json!({ "command": "echo hi", "timeout": 0 })))
            .await
            .unwrap();
        assert_eq!(result.status, ToolResultStatus::Error);
    }
}
