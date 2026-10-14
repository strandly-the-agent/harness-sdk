//! The `shell` tool (`strands-ts/src/vended-tools/shell/make-shell.ts`).
//!
//! Stateless: every call runs the command in a fresh `sh -c` process, so variables and the
//! working directory do not survive across calls. The TS tool routes through
//! `Sandbox.execute`; this port runs on the host, reproducing the local-environment executor
//! (`sh -c`, inherited environment plus [`ShellConfig::env`], optional `cwd`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::error::{Error, Result};
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
            spec: shell_tool_spec(),
        }
    }

    /// Run `command`, killing it after `timeout` (defaults to [`ShellConfig::timeout`]).
    pub async fn execute(
        &self,
        command: &str,
        timeout: Option<f64>,
    ) -> std::result::Result<ShellOutput, ShellError> {
        let seconds = timeout.unwrap_or(self.config.timeout.as_secs_f64());
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(command)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = &self.config.cwd {
            cmd.current_dir(cwd);
        }
        if let Some(env) = &self.config.env {
            cmd.envs(env);
        }

        let mut child = cmd.spawn().map_err(|e| ShellError::Execution(e.to_string()))?;
        let stdout = collect(child.stdout.take());
        let stderr = collect(child.stderr.take());

        let status = tokio::time::timeout(Duration::from_secs_f64(seconds), child.wait()).await;
        match status {
            Ok(status) => {
                let status = status.map_err(|e| ShellError::Execution(e.to_string()))?;
                Ok(ShellOutput {
                    output: stdout.read().await,
                    error: stderr.read().await,
                    exit_code: exit_code_of(&status),
                })
            }
            Err(_) => {
                let _ = child.kill().await;
                Err(ShellError::Timeout {
                    seconds,
                    partial: ShellOutput {
                        output: stdout.read().await,
                        error: stderr.read().await,
                        exit_code: TIMEOUT_EXIT_CODE,
                    },
                })
            }
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

        let run = self.execute(&input.command, input.timeout);
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

/// Exit code of a finished process; a signal maps to `128 + signo` as a shell does.
fn exit_code_of(status: &std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    1
}

/// A pipe being drained in the background, readable as a `String` once the child is done.
struct Collected {
    buffer: Arc<Mutex<Vec<u8>>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Collected {
    async fn read(mut self) -> String {
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
        let bytes = self.buffer.lock().expect("output buffer poisoned").clone();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

fn collect<R>(reader: Option<R>) -> Collected
where
    R: AsyncReadExt + Unpin + Send + 'static,
{
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let task = reader.map(|mut reader| {
        let buffer = Arc::clone(&buffer);
        tokio::spawn(async move {
            let mut chunk = [0u8; 8192];
            while let Ok(read) = reader.read(&mut chunk).await {
                if read == 0 {
                    break;
                }
                buffer
                    .lock()
                    .expect("output buffer poisoned")
                    .extend_from_slice(&chunk[..read]);
            }
        })
    });
    Collected { buffer, task }
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
