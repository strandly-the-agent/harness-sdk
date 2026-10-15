//! Commands on a remote host over OpenSSH (`strands-ts/src/sandbox/ssh.ts`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::{self, BoxStream};

use super::stream_process::{self, Spawn};
use super::{
    posix_shell, ExecuteOptions, ExecutionResult, FileInfo, Sandbox, SandboxError, SandboxEvent,
    SandboxResult,
};
use crate::tools::vended::{ShellTool, SANDBOX_SHELL_DESCRIPTION};
use crate::tools::ToolRef;

/// Stand-in stderr when the `ssh` CLI is not installed (verbatim from TS).
const SSH_MISSING: &str = "ssh is not installed or not on PATH";

/// Known-safe `ssh_config` options, lowercased (verbatim from the TS `ALLOWED_SSH_OPTIONS`).
///
/// Options that execute commands, tunnel traffic, or load external config are excluded, so an
/// attacker-controlled option string cannot turn into local code execution.
/// Full reference: <https://man.openbsd.org/ssh_config>
const ALLOWED_SSH_OPTIONS: &[&str] = &[
    "addressfamily",
    "bindaddress",
    "bindinterface",
    "canonicaldomains",
    "canonicalizefallbacklocal",
    "canonicalizehostname",
    "canonicalizemaxdots",
    "canonicalizepermittedcnames",
    "checkhostip",
    "ciphers",
    "compression",
    "connectionattempts",
    "connecttimeout",
    "hostkeyalgorithms",
    "hostname",
    "identitiesonly",
    "ipqos",
    "kbdinteractiveauthentication",
    "kexalgorithms",
    "loglevel",
    "macs",
    "numberofpasswordprompts",
    "passwordauthentication",
    "port",
    "preferredauthentications",
    "pubkeyacceptedalgorithms",
    "pubkeyauthentication",
    "rekeylimit",
    "serveralivecountmax",
    "serveraliveinterval",
    "tcpkeepalive",
    "updatehostkeys",
    "user",
    "verifyhostkeydns",
];

/// Runs commands on a remote host with `ssh`.
///
/// Shells out to the OpenSSH client like TS and Python do, so the SDK needs no SSH library;
/// [`with_ssh_binary`](Self::with_ssh_binary) points at another one. Every session uses
/// `BatchMode=yes`: interactive prompts are disabled, so authentication must be key-based.
///
/// The sandbox is stateless — every call spawns a fresh `ssh` process. Because `ssh` runs a single
/// remote command string, environment variables are rendered **into** that string as an `export`
/// prefix ([`posix_shell::shell_env_prefix`]) rather than passed as argv flags the way
/// [`DockerSandbox`](super::DockerSandbox) uses `docker exec -e`.
///
/// Unlike TS, an `ssh_options` entry outside the `ALLOWED_SSH_OPTIONS` allowlist is rejected when the argv is
/// built rather than at construction time (Rust builders do not fail); no connection is ever made
/// with an unvetted option either way.
#[derive(Clone, Debug)]
pub struct SshSandbox {
    host: String,
    working_dir: String,
    port: u16,
    identity_file: Option<PathBuf>,
    allow_unknown_hosts: bool,
    ssh_options: Vec<String>,
    allow_unsafe_ssh_options: bool,
    env: Option<HashMap<String, String>>,
    binary: PathBuf,
    description: String,
}

impl SshSandbox {
    /// Sandbox running commands in `working_dir` on `host` (an `ssh` destination such as
    /// `"user@example.com"` or `"192.168.1.10"`).
    pub fn new(host: impl Into<String>, working_dir: impl Into<String>) -> Self {
        let host = host.into();
        Self {
            description: format!("a remote host over SSH (\"{host}\")"),
            host,
            working_dir: working_dir.into(),
            port: 22,
            identity_file: None,
            allow_unknown_hosts: false,
            ssh_options: Vec::new(),
            allow_unsafe_ssh_options: false,
            env: None,
            binary: PathBuf::from("ssh"),
        }
    }

    /// Port to connect to (`ssh -p`); 22 by default.
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Private key file to authenticate with (`ssh -i`); the client's own default keys when unset.
    pub fn with_identity_file(mut self, identity_file: impl Into<PathBuf>) -> Self {
        self.identity_file = Some(identity_file.into());
        self
    }

    /// Connect to hosts with an unknown or changed key (`StrictHostKeyChecking=no`).
    ///
    /// When unset, `StrictHostKeyChecking=accept-new` trusts the host on first connect but
    /// refuses once its key changes.
    pub fn with_allow_unknown_hosts(mut self, allow: bool) -> Self {
        self.allow_unknown_hosts = allow;
        self
    }

    /// Extra `ssh -o` options, each `"Name=value"`; rejected unless known-safe or
    /// [`with_allow_unsafe_ssh_options`](Self::with_allow_unsafe_ssh_options) is set.
    pub fn with_ssh_options(mut self, options: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.ssh_options = options.into_iter().map(Into::into).collect();
        self
    }

    /// Pass [`with_ssh_options`](Self::with_ssh_options) through unvalidated.
    ///
    /// Options such as `ProxyCommand` or `LocalCommand` then run arbitrary programs on the local
    /// machine, so only set this for options you control.
    pub fn with_allow_unsafe_ssh_options(mut self, allow: bool) -> Self {
        self.allow_unsafe_ssh_options = allow;
        self
    }

    /// Environment variables added to every command; a per-call [`ExecuteOptions::env`] entry wins
    /// over one set here.
    pub fn with_env(mut self, env: HashMap<String, String>) -> Self {
        self.env = Some(env);
        self
    }

    /// Path to the `ssh` CLI; `ssh` from `PATH` by default.
    pub fn with_ssh_binary(mut self, binary: impl Into<PathBuf>) -> Self {
        self.binary = binary.into();
        self
    }

    /// Destination commands run on.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Remote directory commands run in.
    pub fn working_dir(&self) -> &str {
        &self.working_dir
    }

    /// The `ssh` arguments for `command`, in the order the CLI needs them.
    fn argv(&self, command: &str, options: &ExecuteOptions) -> SandboxResult<Vec<String>> {
        if !self.allow_unsafe_ssh_options {
            for option in &self.ssh_options {
                let name = option
                    .split(|c: char| c == '=' || c.is_whitespace())
                    .next()
                    .unwrap_or_default();
                if !ALLOWED_SSH_OPTIONS.contains(&name.to_lowercase().as_str()) {
                    return Err(SandboxError::Other(format!(
                        "SSH option \"{name}\" is not allowed. Set allow_unsafe_ssh_options to bypass."
                    )));
                }
            }
        }

        let cwd = options
            .cwd
            .as_ref()
            .map(|cwd| cwd.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.working_dir.clone());
        let env = posix_shell::merge_env(self.env.as_ref(), options.env.as_ref());
        let prefix = posix_shell::shell_env_prefix(&env)?;
        let remote_command = format!("cd {} && {prefix}{command}", super::shell_quote(&cwd));

        let strict = if self.allow_unknown_hosts {
            "no"
        } else {
            "accept-new"
        };
        let mut args = vec![
            "-o".to_owned(),
            format!("StrictHostKeyChecking={strict}"),
            "-o".to_owned(),
            "BatchMode=yes".to_owned(),
            "-p".to_owned(),
            self.port.to_string(),
        ];
        if let Some(identity_file) = &self.identity_file {
            args.extend(["-i".to_owned(), identity_file.to_string_lossy().into_owned()]);
        }
        for option in &self.ssh_options {
            args.extend(["-o".to_owned(), option.clone()]);
        }
        // `ssh` wants the destination and command after all flags, and `--` keeps a dash-prefixed
        // host (`-oProxyCommand=evil`) from being parsed as a flag that would run a local program.
        args.extend(["--".to_owned(), self.host.clone(), remote_command]);
        Ok(args)
    }

    fn spawn(&self, command: &str, options: &ExecuteOptions) -> SandboxResult<Spawn> {
        Ok(Spawn::new(&self.binary, self.argv(command, options)?).enoent_message(SSH_MISSING))
    }
}

#[async_trait]
impl Sandbox for SshSandbox {
    fn kind(&self) -> &str {
        "ssh"
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

    /// The shell tool bound to this host, its description naming the host so the model knows
    /// where commands land. Cloning is sound because the sandbox is stateless.
    fn tools(&self) -> Vec<ToolRef> {
        let shell = ShellTool::with_sandbox(Arc::new(self.clone())).with_description(format!(
            "{SANDBOX_SHELL_DESCRIPTION} Runs on host \"{}\". Working directory: {}.",
            self.host, self.working_dir
        ));
        vec![Arc::new(shell)]
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::sandbox::StreamType;
    use crate::testing::FakeBinary;

    fn argv(sandbox: &SshSandbox, command: &str) -> Vec<String> {
        sandbox.argv(command, &ExecuteOptions::default()).unwrap()
    }

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn stores_host_and_working_dir() {
        let sandbox = SshSandbox::new("user@server.com", "/remote/path");
        assert_eq!(sandbox.host(), "user@server.com");
        assert_eq!(sandbox.working_dir(), "/remote/path");
        assert_eq!(sandbox.kind(), "ssh");
        assert!(sandbox.description().contains("user@server.com"));
    }

    #[test]
    fn permits_known_safe_options() {
        let sandbox = SshSandbox::new("h", "/w").with_ssh_options([
            "ConnectTimeout=10",
            "ServerAliveInterval=30",
            "Compression=yes",
            "IdentitiesOnly=yes",
        ]);
        let args = argv(&sandbox, "ls");
        assert_eq!(
            args[args.len() - 11..args.len() - 3],
            [
                "-o",
                "ConnectTimeout=10",
                "-o",
                "ServerAliveInterval=30",
                "-o",
                "Compression=yes",
                "-o",
                "IdentitiesOnly=yes"
            ]
        );
    }

    #[test]
    fn rejects_proxy_command() {
        let sandbox = SshSandbox::new("h", "/w").with_ssh_options(["ProxyCommand=nc evil.com 22"]);
        let error = sandbox.argv("ls", &ExecuteOptions::default()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "SSH option \"ProxyCommand\" is not allowed. Set allow_unsafe_ssh_options to bypass."
        );
    }

    #[test]
    fn rejects_include() {
        let sandbox = SshSandbox::new("h", "/w").with_ssh_options(["Include=/tmp/evil"]);
        assert!(sandbox.argv("ls", &ExecuteOptions::default()).is_err());
    }

    #[test]
    fn rejects_match() {
        let sandbox = SshSandbox::new("h", "/w").with_ssh_options(["Match host evil"]);
        assert!(sandbox.argv("ls", &ExecuteOptions::default()).is_err());
    }

    #[test]
    fn rejects_local_command() {
        let sandbox =
            SshSandbox::new("h", "/w").with_ssh_options(["PermitLocalCommand=yes", "LocalCommand=rm -rf /"]);
        assert!(sandbox.argv("ls", &ExecuteOptions::default()).is_err());
    }

    #[test]
    fn rejects_forwarding_directives() {
        for option in [
            "LocalForward=8080 localhost:80",
            "RemoteForward=8080 localhost:80",
            "DynamicForward=1080",
            "ForwardAgent=yes",
        ] {
            let sandbox = SshSandbox::new("h", "/w").with_ssh_options([option]);
            assert!(
                sandbox.argv("ls", &ExecuteOptions::default()).is_err(),
                "{option}"
            );
        }
    }

    #[test]
    fn option_allowlist_is_case_insensitive() {
        let sandbox = SshSandbox::new("h", "/w").with_ssh_options(["CONNECTTIMEOUT=10"]);
        assert!(sandbox.argv("ls", &ExecuteOptions::default()).is_ok());
        let sandbox = SshSandbox::new("h", "/w").with_ssh_options(["pRoXyCoMmAnD=nc evil.com 22"]);
        assert!(sandbox.argv("ls", &ExecuteOptions::default()).is_err());
    }

    #[test]
    fn allow_unsafe_ssh_options_bypasses_validation() {
        let sandbox = SshSandbox::new("h", "/w")
            .with_ssh_options(["ProxyCommand=nc proxy.internal 22"])
            .with_allow_unsafe_ssh_options(true);
        let args = argv(&sandbox, "ls");
        assert_eq!(
            args[args.len() - 5..args.len() - 3],
            ["-o", "ProxyCommand=nc proxy.internal 22"]
        );
    }

    #[test]
    fn builds_ssh_args_with_defaults() {
        assert_eq!(
            argv(&SshSandbox::new("user@server.com", "/remote/path"), "echo hi"),
            [
                "-o",
                "StrictHostKeyChecking=accept-new",
                "-o",
                "BatchMode=yes",
                "-p",
                "22",
                "--",
                "user@server.com",
                "cd '/remote/path' && echo hi"
            ]
        );
    }

    #[test]
    fn uses_strict_host_key_checking_no_when_allow_unknown_hosts_is_set() {
        let sandbox = SshSandbox::new("h", "/w").with_allow_unknown_hosts(true);
        assert_eq!(
            argv(&sandbox, "ls"),
            [
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "BatchMode=yes",
                "-p",
                "22",
                "--",
                "h",
                "cd '/w' && ls"
            ]
        );
    }

    #[test]
    fn includes_identity_file_when_provided() {
        let sandbox = SshSandbox::new("h", "/w").with_identity_file("/keys/id_ed25519");
        let args = argv(&sandbox, "ls");
        assert_eq!(args[6..8], ["-i", "/keys/id_ed25519"]);
        assert_eq!(args[8..], ["--", "h", "cd '/w' && ls"]);
    }

    #[test]
    fn uses_custom_port() {
        let sandbox = SshSandbox::new("h", "/w").with_port(2222);
        assert_eq!(argv(&sandbox, "ls")[4..6], ["-p", "2222"]);
    }

    #[test]
    fn terminates_flags_with_dashdash_so_a_dash_prefixed_host_is_not_parsed_as_a_flag() {
        let sandbox = SshSandbox::new("-oProxyCommand=evil", "/w");
        let args = argv(&sandbox, "ls");
        assert_eq!(
            args[args.len() - 3..],
            ["--", "-oProxyCommand=evil", "cd '/w' && ls"]
        );
    }

    #[test]
    fn quotes_cwd_with_single_quotes() {
        let sandbox = SshSandbox::new("h", "/path with spaces/and'quote");
        assert_eq!(
            argv(&sandbox, "ls").last().unwrap(),
            "cd '/path with spaces/and'\\''quote' && ls"
        );
    }

    #[test]
    fn uses_cwd_option_when_provided() {
        let sandbox = SshSandbox::new("h", "/w");
        let options = ExecuteOptions {
            cwd: Some(PathBuf::from("/override")),
            ..ExecuteOptions::default()
        };
        assert_eq!(
            sandbox.argv("pwd", &options).unwrap().last().unwrap(),
            "cd '/override' && pwd"
        );
    }

    #[test]
    fn prefixes_command_with_env_vars_when_provided() {
        let sandbox = SshSandbox::new("h", "/w");
        let options = ExecuteOptions {
            env: Some(env(&[("FOO", "bar"), ("BAZ", "qux")])),
            ..ExecuteOptions::default()
        };
        assert_eq!(
            sandbox.argv("echo $FOO", &options).unwrap().last().unwrap(),
            "cd '/w' && export BAZ='qux' FOO='bar' && echo $FOO"
        );
    }

    #[test]
    fn layers_a_per_call_env_var_over_the_sandbox_env() {
        let sandbox = SshSandbox::new("h", "/w").with_env(env(&[("FOO", "sandbox"), ("KEEP", "yes")]));
        let options = ExecuteOptions {
            env: Some(env(&[("FOO", "call")])),
            ..ExecuteOptions::default()
        };
        assert_eq!(
            sandbox.argv("echo", &options).unwrap().last().unwrap(),
            "cd '/w' && export FOO='call' KEEP='yes' && echo"
        );
    }

    #[test]
    fn rejects_invalid_env_var_names() {
        let sandbox = SshSandbox::new("h", "/w");
        let options = ExecuteOptions {
            env: Some(env(&[("FOO=bar BAZ", "val")])),
            ..ExecuteOptions::default()
        };
        assert_eq!(
            sandbox.argv("cmd", &options).unwrap_err().to_string(),
            "Invalid environment variable name: FOO=bar BAZ"
        );
    }

    #[tokio::test]
    async fn surfaces_an_invalid_env_var_name_from_execute_and_execute_streaming() {
        use futures::StreamExt;

        let sandbox = SshSandbox::new("h", "/w").with_ssh_binary("/nonexistent/ssh");
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
    async fn reports_a_missing_ssh_cli_as_exit_127() {
        let sandbox = SshSandbox::new("h", "/w").with_ssh_binary("/nonexistent/ssh");
        let result = sandbox
            .execute("echo hi", ExecuteOptions::default())
            .await
            .unwrap();
        assert_eq!(result.exit_code, 127);
        assert_eq!(result.stderr, SSH_MISSING);
    }

    #[tokio::test]
    async fn runs_the_constructed_argv_through_the_ssh_binary() {
        let fake = FakeBinary::argv_echo();
        let sandbox = SshSandbox::new("h", "/w")
            .with_port(2222)
            .with_ssh_binary(fake.path());
        let result = sandbox
            .execute("echo hi", ExecuteOptions::default())
            .await
            .unwrap();
        assert_eq!(
            result.stdout.lines().collect::<Vec<_>>(),
            [
                "-o",
                "StrictHostKeyChecking=accept-new",
                "-o",
                "BatchMode=yes",
                "-p",
                "2222",
                "--",
                "h",
                "cd '/w' && echo hi"
            ]
        );
    }

    #[tokio::test]
    async fn streams_stdout_chunks_that_add_up_to_the_terminal_result() {
        use futures::StreamExt;

        let fake = FakeBinary::local_exec();
        let sandbox = SshSandbox::new("h", "/").with_ssh_binary(fake.path());
        let events: Vec<_> = sandbox
            .execute_streaming("printf 'a\\nb\\n'", ExecuteOptions::default())
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
    async fn times_out_a_slow_ssh_and_keeps_the_partial_output() {
        let fake = FakeBinary::script("printf stalling; sleep 5\n");
        let sandbox = SshSandbox::new("h", "/w").with_ssh_binary(fake.path());
        let error = sandbox
            .execute(
                "sleep 5",
                ExecuteOptions::with_timeout(Duration::from_millis(200)),
            )
            .await
            .unwrap_err();
        match error {
            SandboxError::Timeout { stdout, .. } => assert_eq!(stdout, "stalling"),
            other => panic!("expected a timeout, got {other}"),
        }
    }

    #[tokio::test]
    async fn round_trips_files_and_code_through_the_shared_posix_shell_layer() {
        let fake = FakeBinary::local_exec();
        let dir = std::env::temp_dir().join(format!("strands-ssh-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let sandbox = SshSandbox::new("h", dir.to_string_lossy().into_owned()).with_ssh_binary(fake.path());

        let path = dir.join("hello.txt").to_string_lossy().into_owned();
        sandbox.write_file(&path, b"hi there").await.unwrap();
        assert_eq!(sandbox.read_file(&path).await.unwrap(), b"hi there");

        let listing = sandbox.list_files(&dir.to_string_lossy()).await.unwrap();
        assert_eq!(listing.len(), 1);
        assert_eq!(listing[0].name, "hello.txt");

        let result = sandbox
            .execute_code("print('from python')", "python3", ExecuteOptions::default())
            .await
            .unwrap();
        if result.exit_code == 0 {
            assert_eq!(result.stdout.trim(), "from python");
        }

        sandbox.remove_file(&path).await.unwrap();
        assert!(matches!(
            sandbox.read_file(&path).await.unwrap_err(),
            SandboxError::PathNotFound(_)
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn vends_a_shell_tool() {
        let tools = SshSandbox::new("myhost", "/workspace").tools();
        assert_eq!(
            tools.iter().map(|t| t.name()).collect::<Vec<_>>(),
            [crate::tools::vended::SHELL_TOOL_NAME]
        );
        assert_eq!(
            tools[0].description(),
            format!("{SANDBOX_SHELL_DESCRIPTION} Runs on host \"myhost\". Working directory: /workspace.")
        );
    }
}
