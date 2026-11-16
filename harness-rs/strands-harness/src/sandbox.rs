//! Sandbox helpers shared by the harness tools and plugins (TS `sandbox.ts`).

use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use strands::sandbox::{ExecuteOptions, Sandbox};

/// Prints the PowerShell edition (`Desktop` or `Core`); `sh` expands the unset variable to an
/// empty line.
pub(crate) const SHELL_PROBE: &str = "echo \"${PSEdition}\"";

/// The shell dialect a sandbox runs commands in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SandboxShell {
    Sh,
    PowerShell,
}

/// The shells already probed, one entry per live sandbox (TS keeps a `WeakMap`). Each tool or
/// plugin that asks owns one.
#[derive(Default)]
pub(crate) struct ShellCache(Mutex<Vec<(Weak<dyn Sandbox>, SandboxShell)>>);

impl ShellCache {
    /// The shell `sandbox` runs commands in, probed once per sandbox.
    pub(crate) async fn shell_of(&self, sandbox: &Arc<dyn Sandbox>) -> strands::Result<SandboxShell> {
        if let Some(shell) = self.cached(sandbox) {
            return Ok(shell);
        }
        let options = ExecuteOptions {
            timeout: Some(Duration::from_secs(10)),
            ..ExecuteOptions::default()
        };
        let result = sandbox.execute(SHELL_PROBE, options).await?;
        // A failed probe is inconclusive: answer `sh` for this call and probe again next time.
        if result.exit_code != 0 {
            return Ok(SandboxShell::Sh);
        }
        let shell = match result.stdout.trim() {
            "Desktop" | "Core" => SandboxShell::PowerShell,
            _ => SandboxShell::Sh,
        };
        self.entries().push((Arc::downgrade(sandbox), shell));
        Ok(shell)
    }

    fn cached(&self, sandbox: &Arc<dyn Sandbox>) -> Option<SandboxShell> {
        let mut entries = self.entries();
        entries.retain(|(known, _)| known.strong_count() > 0);
        entries
            .iter()
            .find(|(known, _)| std::ptr::addr_eq(known.as_ptr(), Arc::as_ptr(sandbox)))
            .map(|(_, shell)| *shell)
    }

    fn entries(&self) -> std::sync::MutexGuard<'_, Vec<(Weak<dyn Sandbox>, SandboxShell)>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
