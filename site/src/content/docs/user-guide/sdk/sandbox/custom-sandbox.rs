//! Rust snippets for `sandbox/custom-sandbox.mdx`.

use std::sync::Arc;

use async_trait::async_trait;
use strands_agents::sandbox::{
    posix_shell, ExecuteOptions, ExecutionResult, FileInfo, Sandbox, SandboxResult,
};
use strands_agents::tools::ToolRef;
use strands_agents::ShellTool;

/// Stand-in for the reader's own microVM client.
mod fc {
    use strands_agents::sandbox::{ExecuteOptions, ExecutionResult, SandboxResult};

    pub async fn exec(
        _vm_id: &str,
        _command: &str,
        _options: ExecuteOptions,
    ) -> SandboxResult<ExecutionResult> {
        unimplemented!("call your microVM API here")
    }
}

// --8<-- [start:custom_sandbox]
#[derive(Clone, Debug)]
struct FirecrackerSandbox {
    vm_id: String,
}

#[async_trait]
impl Sandbox for FirecrackerSandbox {
    fn kind(&self) -> &str {
        "firecracker"
    }

    // The one primitive: run a shell command in your environment.
    async fn execute(&self, command: &str, options: ExecuteOptions) -> SandboxResult<ExecutionResult> {
        fc::exec(&self.vm_id, command, options).await
    }

    // The rest is shell plumbing `posix_shell` already implements.
    async fn execute_code(
        &self,
        code: &str,
        language: &str,
        options: ExecuteOptions,
    ) -> SandboxResult<ExecutionResult> {
        posix_shell::execute_code(self, code, language, options).await
    }

    async fn read_file(&self, path: &str) -> SandboxResult<Vec<u8>> {
        posix_shell::read_file(self, path).await
    }

    async fn write_file(&self, path: &str, content: &[u8]) -> SandboxResult<()> {
        posix_shell::write_file(self, path, content).await
    }

    async fn remove_file(&self, path: &str) -> SandboxResult<()> {
        posix_shell::remove_file(self, path).await
    }

    async fn list_files(&self, path: &str) -> SandboxResult<Vec<FileInfo>> {
        posix_shell::list_files(self, path).await
    }
    // --8<-- [end:custom_sandbox]

    // --8<-- [start:vend_tools]
    fn tools(&self) -> Vec<ToolRef> {
        vec![Arc::new(ShellTool::with_sandbox(Arc::new(self.clone())))]
    }
    // --8<-- [end:vend_tools]
}
