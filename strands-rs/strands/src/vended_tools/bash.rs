//! The persistent, host-spawned bash tool (TS `vended-tools/bash/`).
//!
//! Each agent gets its own `bash` process, kept across calls so variables and the working
//! directory persist. It runs on the host, not in the agent's sandbox, as in TS.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout};

use crate::JsonValue;
use crate::retry::model_retry_strategy::{Sleeper, tokio_sleeper};
use crate::tools::function_tool::InputValidator;
use crate::tools::{FunctionTool, ToolContext, ToolError};

use super::{js_number, no_precheck, schema_tool};

const DEFAULT_DESCRIPTION: &str = "Executes bash shell commands in a persistent session. Supports execute and restart modes. Commands persist state (variables, directory) within the session. Node.js only.";
const DEFAULT_TIMEOUT_SECS: f64 = 120.0;

/// What the bash tool returns for a command (TS `BashOutput`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct BashOutput {
    /// Standard output, trimmed.
    pub output: String,
    /// Standard error, trimmed.
    pub error: String,
}

/// A bash command ran past its timeout (TS `BashTimeoutError`); the session is restarted.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
#[non_exhaustive]
pub struct BashTimeoutError {
    message: String,
}

impl BashTimeoutError {
    /// The TS `error.name`.
    pub const NAME: &'static str = "BashTimeoutError";

    /// An error with `message`.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// The bash session failed (TS `BashSessionError`).
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
#[non_exhaustive]
pub struct BashSessionError {
    message: String,
}

impl BashSessionError {
    /// The TS `error.name`.
    pub const NAME: &'static str = "BashSessionError";

    /// An error with `message`.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum Mode {
    Execute,
    Restart,
}

#[derive(Deserialize)]
struct BashInput {
    mode: Mode,
    command: Option<String>,
    timeout: Option<f64>,
}

fn bash_input_schema() -> JsonValue {
    serde_json::json!({
        "type": "object",
        "properties": {
            "mode": {
                "type": "string",
                "enum": ["execute", "restart"],
                "description": "Operation mode: \"execute\" to run a command, \"restart\" to restart the session",
            },
            "command": {
                "description": "The bash command to execute (required when mode is \"execute\")",
                "type": "string",
            },
            "timeout": {
                "description": "Timeout in seconds (default: 120, applies only to execute mode)",
                "type": "number",
                "exclusiveMinimum": 0,
            },
        },
        "required": ["mode"],
        "additionalProperties": false,
    })
}

/// The bash tool (TS `bash`). It executes arbitrary commands on the host without isolation;
/// use it only with trusted input. Sessions end when the tool is dropped.
pub fn bash() -> FunctionTool {
    bash_with(None, tokio_sleeper())
}

/// The bash tool starting sessions in `cwd` (the process's working directory when `None`) and
/// timing out on `sleeper`.
pub(crate) fn bash_with(cwd: Option<PathBuf>, sleeper: Sleeper) -> FunctionTool {
    let sessions = Arc::new(Sessions {
        entries: Mutex::new(Vec::new()),
        cwd,
        sleeper,
    });
    let schema = bash_input_schema();
    schema_tool(
        "bash".to_owned(),
        DEFAULT_DESCRIPTION.to_owned(),
        schema.clone(),
        InputValidator::new(&schema),
        no_precheck,
        move |input: BashInput, context: ToolContext| {
            let sessions = Arc::clone(&sessions);
            async move { Box::pin(sessions.call(input, &context)).await }
        },
    )
}

type SharedSession = Arc<tokio::sync::Mutex<BashSession>>;

/// Sessions keyed by agent (TS keeps them in a `WeakMap` keyed by the agent).
struct Sessions {
    entries: Mutex<Vec<(Weak<()>, SharedSession)>>,
    cwd: Option<PathBuf>,
    sleeper: Sleeper,
}

impl Sessions {
    async fn call(&self, input: BashInput, context: &ToolContext) -> Result<JsonValue, ToolError> {
        let owner = context.agent().agent_sandbox().owner();
        match input.mode {
            Mode::Restart => {
                let old = self.replace(owner, DEFAULT_TIMEOUT_SECS);
                if let Some(old) = old {
                    old.lock().await.stop();
                }
                Ok("Bash session restarted".into())
            }
            Mode::Execute => {
                let Some(command) = input.command.filter(|command| !command.is_empty()) else {
                    return Err(ToolError::msg("command is required when mode is \"execute\""));
                };
                let session = self.get_or_create(owner, input.timeout.unwrap_or(DEFAULT_TIMEOUT_SECS));
                let output = Box::pin(session.lock().await.run(&command, input.timeout)).await?;
                Ok(serde_json::to_value(output)?)
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<(Weak<()>, SharedSession)>> {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries.retain(|(agent, _)| agent.strong_count() > 0);
        entries
    }

    fn get_or_create(&self, owner: &Arc<()>, timeout: f64) -> SharedSession {
        let mut entries = self.lock();
        if let Some((_, session)) = entries
            .iter()
            .find(|(agent, _)| agent.as_ptr() == Arc::as_ptr(owner))
        {
            return Arc::clone(session);
        }
        let session = Arc::new(tokio::sync::Mutex::new(self.new_session(timeout)));
        entries.push((Arc::downgrade(owner), Arc::clone(&session)));
        session
    }

    /// Installs a fresh session for `owner` and returns the one it replaced.
    fn replace(&self, owner: &Arc<()>, timeout: f64) -> Option<SharedSession> {
        let mut entries = self.lock();
        let old = entries
            .iter()
            .position(|(agent, _)| agent.as_ptr() == Arc::as_ptr(owner))
            .map(|index| entries.remove(index).1);
        let session = Arc::new(tokio::sync::Mutex::new(self.new_session(timeout)));
        entries.push((Arc::downgrade(owner), session));
        old
    }

    fn new_session(&self, timeout: f64) -> BashSession {
        let id = uuid::Uuid::now_v7().simple().to_string();
        BashSession {
            process: None,
            timeout,
            sentinel: format!("__BASH_DONE_{id}__"),
            cwd: self.cwd.clone(),
            sleeper: Arc::clone(&self.sleeper),
        }
    }
}

struct BashProcess {
    child: Box<dyn ChildWrapper>,
    stdin: ChildStdin,
    stdout: ChildStdout,
    stderr: ChildStderr,
}

impl Drop for BashProcess {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

struct BashSession {
    process: Option<BashProcess>,
    timeout: f64,
    sentinel: String,
    cwd: Option<PathBuf>,
    sleeper: Sleeper,
}

enum Read {
    Stdout(std::io::Result<usize>),
    Stderr(std::io::Result<usize>),
    TimedOut,
}

impl BashSession {
    fn start(&mut self) -> Result<&mut BashProcess, BashSessionError> {
        if let Some(process) = &mut self.process
            && matches!(process.child.try_wait(), Ok(None))
        {
            return self
                .process
                .as_mut()
                .ok_or_else(|| BashSessionError::new("Bash session not properly initialized"));
        }
        let cwd = match &self.cwd {
            Some(cwd) => cwd.clone(),
            None => std::env::current_dir()
                .map_err(|error| BashSessionError::new(format!("Failed to start bash session: {error}")))?,
        };
        let mut command = CommandWrap::with_new("bash", |command| {
            command
                .current_dir(cwd)
                .env("PS1", "")
                .env("PS2", "")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
        });
        #[cfg(unix)]
        command.wrap(process_wrap::tokio::ProcessGroup::leader());
        command.wrap(KillOnDrop);
        let mut child = command
            .spawn()
            .map_err(|error| BashSessionError::new(format!("Failed to start bash session: {error}")))?;
        let streams = (child.stdin().take(), child.stdout().take(), child.stderr().take());
        let (Some(stdin), Some(stdout), Some(stderr)) = streams else {
            return Err(BashSessionError::new("Failed to create bash process streams"));
        };
        Ok(self.process.insert(BashProcess {
            child,
            stdin,
            stdout,
            stderr,
        }))
    }

    fn stop(&mut self) {
        self.process = None;
    }

    async fn run(&mut self, command: &str, timeout: Option<f64>) -> Result<BashOutput, ToolError> {
        let effective = timeout.unwrap_or(self.timeout);
        let sentinel = self.sentinel.clone();
        let deadline = (self.sleeper)(Duration::try_from_secs_f64(effective).unwrap_or(Duration::MAX));
        self.start()?;
        // The call owns the process: an error, or a dropped call, kills the session with it.
        let Some(mut process) = self.process.take() else {
            return Err(BashSessionError::new("Bash session not properly initialized").into());
        };
        // The stderr sentinel orders the command's stderr before the result is taken.
        let script = format!("{command}\necho \"{sentinel}\"\necho \"{sentinel}\" >&2\n");
        if let Err(error) = process.stdin.write_all(script.as_bytes()).await {
            return Err(BashSessionError::new(format!("Failed to write command: {error}")).into());
        }
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let (mut out_buf, mut err_buf) = ([0u8; 8192], [0u8; 8192]);
        let mut deadline = std::pin::pin!(deadline);
        let timed_out = || {
            let message = format!("Command timed out after {} seconds", js_number(effective));
            Err(BashTimeoutError::new(message).into())
        };
        loop {
            let read = tokio::select! {
                read = process.stdout.read(&mut out_buf) => Read::Stdout(read),
                read = process.stderr.read(&mut err_buf) => Read::Stderr(read),
                () = &mut deadline => Read::TimedOut,
            };
            match read {
                Read::Stdout(Ok(n)) if n > 0 => {
                    stdout.extend_from_slice(out_buf.get(..n).unwrap_or_default());
                }
                Read::Stderr(Ok(n)) if n > 0 => {
                    stderr.extend_from_slice(err_buf.get(..n).unwrap_or_default());
                }
                Read::TimedOut => return timed_out(),
                Read::Stdout(_) | Read::Stderr(_) => {
                    // A closed pipe is session death only once bash exits; the timeout stays armed.
                    let status = tokio::select! {
                        status = process.child.wait() => status,
                        () = &mut deadline => return timed_out(),
                    };
                    let code = status
                        .ok()
                        .and_then(|status| status.code())
                        .map_or_else(|| "unknown".to_owned(), |code| code.to_string());
                    let message = format!("Bash process exited unexpectedly with code {code}");
                    return Err(BashSessionError::new(message).into());
                }
            }
            if let Some(output) = finished(&stdout, &stderr, &sentinel) {
                self.process = Some(process);
                return Ok(output);
            }
        }
    }
}

/// The output once both streams carry the sentinel.
fn finished(stdout: &[u8], stderr: &[u8], sentinel: &str) -> Option<BashOutput> {
    let (stdout, stderr) = (String::from_utf8_lossy(stdout), String::from_utf8_lossy(stderr));
    if !stdout.contains(sentinel) || !stderr.contains(sentinel) {
        return None;
    }
    Some(BashOutput {
        output: stdout.replacen(sentinel, "", 1).trim().to_owned(),
        error: stderr.replacen(sentinel, "", 1).trim().to_owned(),
    })
}

#[cfg(test)]
mod tests;
