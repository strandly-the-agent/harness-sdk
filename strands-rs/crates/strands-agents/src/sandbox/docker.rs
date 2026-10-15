//! Commands in a Docker container via `docker exec` (`strands-ts/src/sandbox/docker.ts`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::{self, BoxStream};

use super::stream_process::{self, Spawn};
use super::{posix_shell, ExecuteOptions, ExecutionResult, FileInfo, Sandbox, SandboxEvent, SandboxResult};
use crate::tools::vended::ShellTool;
use crate::tools::ToolRef;

/// Stand-in stderr when the `docker` CLI is not installed (verbatim from TS).
const DOCKER_MISSING: &str = "docker is not installed or not on PATH";

/// Runs commands in a running Docker container with `docker exec`.
///
/// Shells out to the `docker` CLI like TS and Python do, so the SDK needs no Docker API
/// dependency; [`with_docker_binary`](Self::with_docker_binary) points at another one (a
/// non-standard install, or `podman`). File and code operations run as shell commands in the
/// container through [`posix_shell`].
///
/// The sandbox is stateless — every call spawns a fresh `docker exec` — and does not manage the
/// container's lifecycle: the container must already be running. Like TS, there is no
/// image/auto-start option, because [`Sandbox`] has no `start`/`stop` hook to tear one down
/// again.
#[derive(Clone, Debug)]
pub struct DockerSandbox {
    container: String,
    working_dir: Option<String>,
    user: Option<String>,
    env: Option<HashMap<String, String>>,
    binary: PathBuf,
    description: String,
}

impl DockerSandbox {
    /// Sandbox running commands in the container with this id or name.
    pub fn new(container: impl Into<String>) -> Self {
        let container = container.into();
        Self {
            description: format!("a Docker container (\"{container}\")"),
            container,
            working_dir: None,
            user: None,
            env: None,
            binary: PathBuf::from("docker"),
        }
    }

    /// Directory commands run in (`docker exec -w`); the container's own when unset.
    ///
    /// The path must exist and be writable by the effective [`with_user`](Self::with_user).
    pub fn with_working_dir(mut self, working_dir: impl Into<String>) -> Self {
        self.working_dir = Some(working_dir.into());
        self
    }

    /// User commands run as, in `"uid"`, `"uid:gid"` or `"name"` form (`docker exec --user`);
    /// the container's own when unset.
    pub fn with_user(mut self, user: impl Into<String>) -> Self {
        self.user = Some(user.into());
        self
    }

    /// Environment variables added to every command (`docker exec -e`); a per-call
    /// [`ExecuteOptions::env`] entry wins over one set here.
    pub fn with_env(mut self, env: HashMap<String, String>) -> Self {
        self.env = Some(env);
        self
    }

    /// Path to the `docker` CLI; `docker` from `PATH` by default.
    pub fn with_docker_binary(mut self, binary: impl Into<PathBuf>) -> Self {
        self.binary = binary.into();
        self
    }

    /// Container commands run in.
    pub fn container(&self) -> &str {
        &self.container
    }

    /// Directory commands run in, when one was configured.
    pub fn working_dir(&self) -> Option<&str> {
        self.working_dir.as_deref()
    }

    /// The `docker` arguments for `command`, in the order the CLI needs them.
    fn argv(&self, command: &str, options: &ExecuteOptions) -> SandboxResult<Vec<String>> {
        let mut args = vec!["exec".to_owned()];
        if options.stdin.is_some() {
            args.push("-i".to_owned());
        }
        // An unset user/cwd defers to the container's own configuration.
        if let Some(user) = &self.user {
            args.extend(["--user".to_owned(), user.clone()]);
        }
        let cwd = options
            .cwd
            .as_ref()
            .map(|cwd| cwd.to_string_lossy().into_owned())
            .or_else(|| self.working_dir.clone());
        if let Some(cwd) = cwd {
            args.extend(["-w".to_owned(), cwd]);
        }
        // Values are process argv, not shell syntax, so Docker stores them verbatim and they need
        // no quoting — unlike the SSH backend, which builds a shell command string.
        let env = posix_shell::merge_env(self.env.as_ref(), options.env.as_ref());
        for (key, value) in posix_shell::sorted_env(&env)? {
            args.extend(["-e".to_owned(), format!("{key}={value}")]);
        }
        // `docker exec` wants the container and command after all flags, and `--` keeps a
        // dash-prefixed container name (`--privileged`) from being parsed as a flag that would
        // override the options above.
        args.extend([
            "--".to_owned(),
            self.container.clone(),
            "sh".to_owned(),
            "-c".to_owned(),
            command.to_owned(),
        ]);
        Ok(args)
    }

    fn spawn(&self, command: &str, options: &ExecuteOptions) -> SandboxResult<Spawn> {
        Ok(Spawn::new(&self.binary, self.argv(command, options)?).enoent_message(DOCKER_MISSING))
    }
}

#[async_trait]
impl Sandbox for DockerSandbox {
    fn kind(&self) -> &str {
        "docker"
    }

    fn description(&self) -> &str {
        &self.description
    }

    async fn execute(&self, command: &str, options: ExecuteOptions) -> SandboxResult<ExecutionResult> {
        stream_process::run(self.spawn(command, &options)?, options).await
    }

    fn execute_streaming<'a>(
        &'a self,
        command: &'a str,
        options: ExecuteOptions,
    ) -> BoxStream<'a, SandboxResult<SandboxEvent>> {
        match self.spawn(command, &options) {
            Ok(spawn) => stream_process::stream(spawn, options),
            Err(error) => Box::pin(stream::once(async move { Err(error) })),
        }
    }

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

    /// The shell tool bound to this container. Cloning is sound because the sandbox is stateless.
    fn tools(&self) -> Vec<ToolRef> {
        vec![Arc::new(ShellTool::with_sandbox(Arc::new(self.clone())))]
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::sandbox::{SandboxError, StreamType};
    use crate::testing::FakeBinary;

    fn argv(sandbox: &DockerSandbox, command: &str) -> Vec<String> {
        sandbox.argv(command, &ExecuteOptions::default()).unwrap()
    }

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn terminates_flags_with_dashdash_so_a_dash_prefixed_container_is_not_parsed_as_a_flag() {
        let args = argv(&DockerSandbox::new("--privileged"), "echo hi");
        assert_eq!(
            args[args.len() - 5..],
            ["--", "--privileged", "sh", "-c", "echo hi"]
        );
    }

    #[test]
    fn omits_user_and_working_dir_when_they_are_unset() {
        assert_eq!(
            argv(&DockerSandbox::new("my-container"), "echo hi"),
            ["exec", "--", "my-container", "sh", "-c", "echo hi"]
        );
    }

    #[test]
    fn uses_custom_user_and_working_dir() {
        let sandbox = DockerSandbox::new("my-container")
            .with_user("root")
            .with_working_dir("/app");
        assert_eq!(
            argv(&sandbox, "ls"),
            [
                "exec",
                "--user",
                "root",
                "-w",
                "/app",
                "--",
                "my-container",
                "sh",
                "-c",
                "ls"
            ]
        );
    }

    #[test]
    fn cwd_option_overrides_working_dir() {
        let sandbox = DockerSandbox::new("my-container").with_working_dir("/app");
        let options = ExecuteOptions {
            cwd: Some(PathBuf::from("/override")),
            ..ExecuteOptions::default()
        };
        assert_eq!(
            sandbox.argv("pwd", &options).unwrap(),
            ["exec", "-w", "/override", "--", "my-container", "sh", "-c", "pwd"]
        );
    }

    #[test]
    fn passes_env_vars_as_e_flags_before_the_container_id() {
        let sandbox = DockerSandbox::new("my-container");
        let options = ExecuteOptions {
            env: Some(env(&[("FOO", "bar"), ("BAZ", "qux")])),
            ..ExecuteOptions::default()
        };
        assert_eq!(
            sandbox.argv("echo $FOO", &options).unwrap(),
            [
                "exec",
                "-e",
                "BAZ=qux",
                "-e",
                "FOO=bar",
                "--",
                "my-container",
                "sh",
                "-c",
                "echo $FOO"
            ]
        );
    }

    #[test]
    fn layers_a_per_call_env_var_over_the_sandbox_env() {
        let sandbox = DockerSandbox::new("c").with_env(env(&[("FOO", "sandbox"), ("KEEP", "yes")]));
        let options = ExecuteOptions {
            env: Some(env(&[("FOO", "call")])),
            ..ExecuteOptions::default()
        };
        let args = sandbox.argv("echo", &options).unwrap();
        assert_eq!(args[..5], ["exec", "-e", "FOO=call", "-e", "KEEP=yes"]);
    }

    #[test]
    fn rejects_invalid_env_var_names() {
        let sandbox = DockerSandbox::new("my-container");
        let options = ExecuteOptions {
            env: Some(env(&[("FOO=bar BAZ", "val")])),
            ..ExecuteOptions::default()
        };
        let error = sandbox.argv("cmd", &options).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Invalid environment variable name: FOO=bar BAZ"
        );
    }

    #[test]
    fn keeps_stdin_open_with_i_when_stdin_is_provided() {
        let sandbox = DockerSandbox::new("c");
        let options = ExecuteOptions {
            stdin: Some("payload".to_owned()),
            ..ExecuteOptions::default()
        };
        assert_eq!(
            sandbox.argv("cat", &options).unwrap(),
            ["exec", "-i", "--", "c", "sh", "-c", "cat"]
        );
    }

    #[tokio::test]
    async fn reports_a_missing_docker_cli_as_exit_127() {
        let sandbox = DockerSandbox::new("c").with_docker_binary("/nonexistent/docker");
        let result = sandbox
            .execute("echo hi", ExecuteOptions::default())
            .await
            .unwrap();
        assert_eq!(result.exit_code, 127);
        assert_eq!(result.stderr, DOCKER_MISSING);
    }

    #[tokio::test]
    async fn surfaces_an_invalid_env_var_name_from_execute_and_execute_streaming() {
        use futures::StreamExt;

        let sandbox = DockerSandbox::new("c").with_docker_binary("/nonexistent/docker");
        let options = ExecuteOptions {
            env: Some(env(&[("BAD-KEY", "v")])),
            ..ExecuteOptions::default()
        };
        let error = sandbox.execute("cmd", options.clone()).await.unwrap_err();
        assert!(matches!(error, SandboxError::Other(_)), "{error}");

        let events: Vec<_> = sandbox.execute_streaming("cmd", options).collect().await;
        assert_eq!(events.len(), 1);
        assert!(events[0].is_err());
    }

    #[tokio::test]
    async fn runs_the_constructed_argv_through_the_docker_binary() {
        let fake = FakeBinary::argv_echo();
        let sandbox = DockerSandbox::new("my-container")
            .with_user("root")
            .with_docker_binary(fake.path());
        let result = sandbox
            .execute("echo hi", ExecuteOptions::default())
            .await
            .unwrap();
        assert_eq!(
            result.stdout.lines().collect::<Vec<_>>(),
            [
                "exec",
                "--user",
                "root",
                "--",
                "my-container",
                "sh",
                "-c",
                "echo hi"
            ]
        );
        assert_eq!(result.exit_code, 0);
    }

    #[tokio::test]
    async fn streams_chunks_then_the_result() {
        use futures::StreamExt;

        let fake = FakeBinary::argv_echo();
        let sandbox = DockerSandbox::new("c").with_docker_binary(fake.path());
        let events: Vec<_> = sandbox
            .execute_streaming("echo hi", ExecuteOptions::default())
            .collect()
            .await;
        let mut chunks = Vec::new();
        let mut result = None;
        for event in events {
            match event.unwrap() {
                SandboxEvent::Chunk(chunk) => chunks.push(chunk),
                SandboxEvent::Result(terminal) => result = Some(terminal),
            }
        }
        assert!(!chunks.is_empty());
        assert!(chunks.iter().all(|c| c.stream_type == StreamType::Stdout));
        assert_eq!(
            chunks.iter().map(|c| c.data.as_str()).collect::<String>(),
            result.expect("terminal result").stdout
        );
    }

    #[tokio::test]
    async fn times_out_a_slow_docker_exec_and_keeps_the_partial_output() {
        let fake = FakeBinary::script("printf stalling; sleep 5\n");
        let sandbox = DockerSandbox::new("c").with_docker_binary(fake.path());
        let error = sandbox
            .execute(
                "echo hi",
                ExecuteOptions::with_timeout(Duration::from_millis(300)),
            )
            .await
            .unwrap_err();
        match error {
            SandboxError::Timeout { stdout, .. } => assert_eq!(stdout, "stalling"),
            other => panic!("expected a timeout, got {other}"),
        }
    }

    /// The remaining backend behaviour is the shared shell layer, so it is exercised against a
    /// stand-in `docker` that runs the container command on the local host.
    #[tokio::test]
    async fn runs_code_and_file_operations_as_shell_commands_in_the_container() {
        let fake = FakeBinary::local_exec();
        let dir = std::env::temp_dir().join(format!("strands-docker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sandbox = DockerSandbox::new("c").with_docker_binary(fake.path());
        let file = dir.join("it's a file.bin").to_string_lossy().into_owned();

        let bytes: Vec<u8> = (0..=255).collect();
        sandbox.write_file(&file, &bytes).await.unwrap();
        assert_eq!(sandbox.read_file(&file).await.unwrap(), bytes);

        let listing = sandbox.list_files(&dir.to_string_lossy()).await.unwrap();
        assert_eq!(
            listing,
            vec![FileInfo {
                name: "it's a file.bin".to_owned(),
                is_dir: Some(false),
                size: None,
            }]
        );

        let code = sandbox
            .execute_code("print('from python')", "python3", ExecuteOptions::default())
            .await
            .unwrap();
        assert_eq!(code.stdout.trim(), "from python");

        sandbox.remove_file(&file).await.unwrap();
        assert!(matches!(
            sandbox.read_file(&file).await.unwrap_err(),
            SandboxError::PathNotFound(_)
        ));
        assert!(matches!(
            sandbox
                .list_files("/tmp/strands-nonexistent-dir-xyz")
                .await
                .unwrap_err(),
            SandboxError::PathNotFound(_)
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn describes_itself_by_container_and_vends_a_shell_tool() {
        let sandbox = DockerSandbox::new("my-container");
        assert_eq!(sandbox.kind(), "docker");
        assert!(sandbox.description().contains("my-container"));
        assert_eq!(sandbox.container(), "my-container");
        assert_eq!(sandbox.working_dir(), None);
        let tools = sandbox.tools();
        assert_eq!(
            tools.iter().map(|t| t.name()).collect::<Vec<_>>(),
            [crate::tools::vended::SHELL_TOOL_NAME]
        );
    }
}
