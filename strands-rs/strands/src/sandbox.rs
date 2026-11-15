//! Where an agent's commands, code and file operations run (TS `sandbox/`).
//!
//! [`Sandbox`] is the abstraction; [`PosixShellSandbox`] derives it from a shell; and
//! [`NotASandboxLocalEnvironment`], the default, runs on the host with no isolation.

mod base;
mod constants;
mod not_a_sandbox_local_environment;
pub mod posix_shell;
pub(crate) mod stream_process;
mod types;

use std::sync::Arc;

pub use base::{ExecuteOptions, Sandbox};
pub use constants::LANGUAGE_PATTERN;
pub use not_a_sandbox_local_environment::NotASandboxLocalEnvironment;
pub use posix_shell::{PosixShellSandbox, build_shell_env_prefix, shell_quote, validate_env_keys};
pub use types::{ExecutionEvent, ExecutionResult, FileInfo, OutputFile, StreamChunk, StreamType};

use crate::error::DefaultNotConfiguredError;

/// The message of the error an agent without a sandbox returns (TS `defaultSandbox`).
pub(crate) const NO_SANDBOX_MESSAGE: &str =
    "No Sandbox configured. Pass a `sandbox` to the Agent to use sandbox features in this environment.";

/// The agent config `sandbox` key (TS `Sandbox | false`).
///
/// Leaving the key unset uses [`NotASandboxLocalEnvironment`]; [`SandboxSource::Off`] opts out,
/// and tools that need a sandbox then fail with [`DefaultNotConfiguredError`].
#[derive(Clone)]
#[non_exhaustive]
pub enum SandboxSource {
    /// Run commands and file operations in this sandbox.
    Sandbox(Arc<dyn Sandbox>),
    /// No sandbox (TS `false`).
    Off,
}

impl SandboxSource {
    /// The source for `sandbox`.
    pub fn new(sandbox: impl Sandbox) -> Self {
        Self::Sandbox(Arc::new(sandbox))
    }
}

impl From<Arc<dyn Sandbox>> for SandboxSource {
    fn from(sandbox: Arc<dyn Sandbox>) -> Self {
        Self::Sandbox(sandbox)
    }
}

impl std::fmt::Debug for SandboxSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sandbox(_) => f.write_str("Sandbox(..)"),
            Self::Off => f.write_str("Off"),
        }
    }
}

/// An agent's sandbox and an identity token for per-agent host state (the `bash` sessions).
#[derive(Clone)]
pub(crate) struct AgentSandbox {
    sandbox: Option<Arc<dyn Sandbox>>,
    configured: bool,
    owner: Arc<()>,
}

impl AgentSandbox {
    pub(crate) fn from_source(source: Option<SandboxSource>) -> Self {
        let (sandbox, configured) = match source {
            None => (Some(default_sandbox()), false),
            Some(SandboxSource::Sandbox(sandbox)) => (Some(sandbox), true),
            Some(SandboxSource::Off) => (None, false),
        };
        Self {
            sandbox,
            configured,
            owner: Arc::new(()),
        }
    }

    /// The sandbox, or [`DefaultNotConfiguredError`] with `sandbox: Off`.
    pub(crate) fn get(&self) -> Result<Arc<dyn Sandbox>, DefaultNotConfiguredError> {
        self.sandbox
            .clone()
            .ok_or_else(|| DefaultNotConfiguredError::new(NO_SANDBOX_MESSAGE))
    }

    /// Tools vended by an explicitly configured sandbox (TS `if (this._sandbox)`).
    pub(crate) fn vended_tools(&self) -> Vec<Arc<dyn crate::tools::Tool>> {
        match (&self.sandbox, self.configured) {
            (Some(sandbox), true) => sandbox.get_tools(),
            _ => Vec::new(),
        }
    }

    /// Identifies the agent; shared by every view of one agent.
    pub(crate) fn owner(&self) -> &Arc<()> {
        &self.owner
    }
}

impl Default for AgentSandbox {
    fn default() -> Self {
        Self::from_source(None)
    }
}

impl std::fmt::Debug for AgentSandbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentSandbox")
            .field("configured", &self.configured)
            .field("off", &self.sandbox.is_none())
            .finish_non_exhaustive()
    }
}

fn default_sandbox() -> Arc<dyn Sandbox> {
    Arc::new(NotASandboxLocalEnvironment::new())
}

#[cfg(test)]
pub(crate) mod test_sandbox;
#[cfg(test)]
mod tests;
