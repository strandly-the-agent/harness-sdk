//! Ported from strands-ts/src/sandbox/__tests__/{default, not-a-sandbox-local-environment,
//! posix-shell, read-file-errors, stream-process}.test.node.ts (the error classes of
//! errors.test.node.ts are covered in `error/tests.rs`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::Notify;

use super::test_sandbox::{
    NotifyOnOutput, TestSandbox, instant_sleeper, local_env, manual_sleeper, never_sleeper,
};
use super::*;
use crate::error::Error;

fn env(pairs: &[(&str, &str)]) -> Option<BTreeMap<String, String>> {
    Some(
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
    )
}

fn with_env(pairs: &[(&str, &str)]) -> ExecuteOptions {
    ExecuteOptions {
        env: env(pairs),
        ..ExecuteOptions::default()
    }
}

fn with_cwd(cwd: &Path) -> ExecuteOptions {
    ExecuteOptions {
        cwd: Some(cwd.to_string_lossy().into_owned()),
        ..ExecuteOptions::default()
    }
}

fn with_timeout(seconds: f64) -> ExecuteOptions {
    ExecuteOptions {
        timeout: Some(Duration::from_secs_f64(seconds)),
        ..ExecuteOptions::default()
    }
}

fn path_str(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Whether the process can read a file it has no read permission on (root).
fn ignores_permissions(dir: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let probe = dir.join(".permission-probe");
    std::fs::write(&probe, "x").unwrap();
    std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o200)).unwrap();
    let readable = std::fs::read(&probe).is_ok();
    std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::remove_file(&probe).unwrap();
    readable
}

fn chmod(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

mod default {
    use super::*;
    use crate::Agent;
    use crate::testing::MockMessageModel;

    fn agent(sandbox: Option<SandboxSource>) -> Agent {
        Agent::builder()
            .model(Arc::new(MockMessageModel::new()) as Arc<dyn crate::models::model::Model>)
            .maybe_sandbox(sandbox)
            .printer(false)
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn returns_the_instance_registered_by_the_setup_file() {
        let dir = tempfile::tempdir().unwrap();
        let sandbox = agent(None).sandbox().unwrap();
        let result = sandbox.execute("echo host", with_cwd(dir.path())).await.unwrap();
        assert_eq!(result.stdout, "host\n");
    }

    #[test]
    fn returns_the_configured_sandbox_when_one_is_provided() {
        let sandbox: Arc<dyn Sandbox> = Arc::new(NotASandboxLocalEnvironment::new());
        let agent = agent(Some(SandboxSource::from(Arc::clone(&sandbox))));
        assert!(Arc::ptr_eq(&agent.sandbox().unwrap(), &sandbox));
    }

    #[test]
    fn falls_back_to_the_registered_default_when_unconfigured() {
        assert!(agent(None).sandbox().is_ok());
    }

    #[test]
    fn treats_sandbox_false_the_same_as_unconfigured() {
        let error = agent(Some(SandboxSource::Off)).sandbox().err().unwrap();
        assert!(matches!(error, Error::DefaultNotConfigured(_)));
        assert_eq!(
            error.to_string(),
            "No Sandbox configured. Pass a `sandbox` to the Agent to use sandbox features in this environment."
        );
    }

    struct Vending;

    impl PosixShellSandbox for Vending {
        fn execute_streaming<'a>(
            &'a self,
            _command: &'a str,
            _options: ExecuteOptions,
        ) -> crate::models::model::BoxStream<'a, crate::Result<ExecutionEvent>> {
            Box::pin(futures::stream::empty())
        }

        fn get_tools(&self) -> Vec<Arc<dyn crate::tools::Tool>> {
            vec![
                Arc::new(crate::test_support::mock_tool("vended", |c| {
                    crate::test_support::text_result(c.tool_use().tool_use_id(), "v")
                })),
                Arc::new(crate::test_support::mock_tool("mine", |c| {
                    crate::test_support::text_result(c.tool_use().tool_use_id(), "v")
                })),
            ]
        }
    }

    #[tokio::test]
    async fn registers_sandbox_vended_tools_skipping_names_the_user_registered() {
        let mut agent = Agent::builder()
            .model(Arc::new(MockMessageModel::new()) as Arc<dyn crate::models::model::Model>)
            .sandbox(SandboxSource::new(Vending))
            .tools([crate::test_support::mock_tool("mine", |c| {
                crate::test_support::text_result(c.tool_use().tool_use_id(), "user")
            })])
            .printer(false)
            .build()
            .unwrap();
        agent.initialize().await.unwrap();
        let names: Vec<String> = agent
            .tool_registry()
            .list()
            .map(|tool| tool.name().to_owned())
            .collect();
        assert_eq!(names, ["mine", "vended"]);
    }
}

mod not_a_sandbox_local_environment {
    use super::*;

    fn sandbox(dir: &Path) -> NotASandboxLocalEnvironment {
        local_env(dir, never_sleeper())
    }

    #[tokio::test]
    async fn runs_a_command_on_the_host() {
        let dir = tempfile::tempdir().unwrap();
        let result = sandbox(dir.path())
            .execute("echo hello", ExecuteOptions::default())
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "hello\n");
    }

    #[tokio::test]
    async fn respects_the_cwd_option() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let result = sandbox(dir.path())
            .execute("pwd", with_cwd(other.path()))
            .await
            .unwrap();
        assert_eq!(
            std::fs::canonicalize(result.stdout.trim()).unwrap(),
            std::fs::canonicalize(other.path()).unwrap()
        );
    }

    #[tokio::test]
    async fn runs_in_the_base_directory_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let result = sandbox(dir.path())
            .execute("pwd", ExecuteOptions::default())
            .await
            .unwrap();
        assert_eq!(
            std::fs::canonicalize(result.stdout.trim()).unwrap(),
            std::fs::canonicalize(dir.path()).unwrap()
        );
    }

    #[tokio::test]
    async fn reports_a_non_zero_exit_code_and_stderr_from_a_failing_command() {
        let dir = tempfile::tempdir().unwrap();
        let result = sandbox(dir.path())
            .execute("echo oops >&2; exit 3", ExecuteOptions::default())
            .await
            .unwrap();
        assert_eq!(result.exit_code, 3);
        assert_eq!(result.stderr, "oops\n");
    }

    #[tokio::test]
    async fn applies_the_env_option() {
        let dir = tempfile::tempdir().unwrap();
        let result = sandbox(dir.path())
            .execute("echo \"$GREETING\"", with_env(&[("GREETING", "hi")]))
            .await
            .unwrap();
        assert_eq!(result.stdout, "hi\n");
    }

    #[tokio::test]
    async fn runs_code_through_the_named_interpreter() {
        let dir = tempfile::tempdir().unwrap();
        let result = sandbox(dir.path())
            .execute_code("echo $((2 + 2))", "sh", with_cwd(dir.path()))
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "4");
    }

    #[tokio::test]
    async fn rejects_a_language_with_invalid_characters() {
        let dir = tempfile::tempdir().unwrap();
        let error = sandbox(dir.path())
            .execute_code("x", "../../bin/sh", ExecuteOptions::default())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("invalid characters"), "{error}");
    }

    #[tokio::test]
    async fn execute_code_applies_the_env_option() {
        let dir = tempfile::tempdir().unwrap();
        let result = sandbox(dir.path())
            .execute_code("echo \"$GREETING\"", "sh", with_env(&[("GREETING", "hi")]))
            .await
            .unwrap();
        assert_eq!(result.stdout.trim(), "hi");
    }

    #[tokio::test]
    async fn reports_a_missing_interpreter_as_exit_127() {
        let dir = tempfile::tempdir().unwrap();
        let result = sandbox(dir.path())
            .execute_code(
                "x",
                "strands-definitely-not-an-interpreter",
                ExecuteOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(result.exit_code, 127);
        assert!(result.stderr.contains("not found"), "{}", result.stderr);
    }

    #[tokio::test]
    async fn keeps_stdin_open_when_no_input_is_given() {
        let dir = tempfile::tempdir().unwrap();
        let error = local_env(dir.path(), instant_sleeper())
            .execute("cat", with_timeout(1.0))
            .await
            .unwrap_err();
        assert!(error.to_string().to_lowercase().contains("timed out"), "{error}");
    }

    #[tokio::test]
    async fn text_file_roundtrip_via_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let file = path_str(&dir.path().join("note.txt"));
        let sandbox = sandbox(dir.path());
        sandbox.write_text(&file, "hello host").await.unwrap();
        assert_eq!(sandbox.read_text(&file).await.unwrap(), "hello host");
    }

    #[tokio::test]
    async fn binary_roundtrip_preserves_all_byte_values() {
        let dir = tempfile::tempdir().unwrap();
        let file = path_str(&dir.path().join("all-bytes.bin"));
        let bytes: Vec<u8> = (0..=255).collect();
        let sandbox = sandbox(dir.path());
        sandbox.write_file(&file, &bytes).await.unwrap();
        assert_eq!(sandbox.read_file(&file).await.unwrap(), bytes);
    }

    #[tokio::test]
    async fn creates_missing_parent_directories_on_write() {
        let dir = tempfile::tempdir().unwrap();
        let file = path_str(&dir.path().join("deep/nested/file.txt"));
        let sandbox = sandbox(dir.path());
        sandbox.write_text(&file, "deep").await.unwrap();
        assert_eq!(sandbox.read_text(&file).await.unwrap(), "deep");
    }

    #[tokio::test]
    async fn keeps_the_original_fs_error_as_the_cause_of_a_typed_read_error() {
        let dir = tempfile::tempdir().unwrap();
        let sandbox = sandbox(dir.path());
        let missing = sandbox
            .read_file(&path_str(&dir.path().join("nope.txt")))
            .await
            .unwrap_err();
        let Error::SandboxPathNotFound(missing) = missing else {
            panic!("expected a path-not-found error");
        };
        let cause = std::error::Error::source(&missing).unwrap();
        assert!(
            cause
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
        );

        let directory = sandbox.read_file(&path_str(dir.path())).await.unwrap_err();
        let Error::SandboxIsADirectory(directory) = directory else {
            panic!("expected an is-a-directory error");
        };
        let cause = std::error::Error::source(&directory).unwrap();
        assert!(
            cause
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::IsADirectory)
        );
    }

    #[tokio::test]
    async fn treats_a_backslash_in_a_filename_as_a_literal_character() {
        let dir = tempfile::tempdir().unwrap();
        let file = path_str(&dir.path().join("weird\\name.txt"));
        let sandbox = sandbox(dir.path());
        sandbox.write_text(&file, "ok").await.unwrap();
        assert_eq!(sandbox.read_text(&file).await.unwrap(), "ok");
        let entries = sandbox.list_files(&path_str(dir.path())).await.unwrap();
        assert!(entries.iter().any(|entry| entry.name == "weird\\name.txt"));
    }

    #[tokio::test]
    async fn removes_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = path_str(&dir.path().join("delete-me.txt"));
        let sandbox = sandbox(dir.path());
        sandbox.write_text(&file, "bye").await.unwrap();
        sandbox.remove_file(&file).await.unwrap();
        assert!(sandbox.read_file(&file).await.is_err());
    }

    #[tokio::test]
    async fn throws_on_a_nonexistent_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = path_str(&dir.path().join("nope.txt"));
        assert!(sandbox(dir.path()).remove_file(&file).await.is_err());
    }

    #[tokio::test]
    async fn returns_entries_sorted_by_name_with_is_dir_and_size_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let sandbox = sandbox(dir.path());
        for (name, content) in [("c.txt", "cc"), ("a.txt", "a"), ("b.txt", "bbb")] {
            sandbox
                .write_text(&path_str(&dir.path().join(name)), content)
                .await
                .unwrap();
        }
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let files = sandbox.list_files(&path_str(dir.path())).await.unwrap();
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["a.txt", "b.txt", "c.txt", "sub"]);
        assert_eq!(files[0].is_dir, Some(false));
        assert_eq!(files[0].size, Some(1));
        assert_eq!(files[3].is_dir, Some(true));
    }

    #[tokio::test]
    async fn throws_sandbox_path_not_found_error_on_a_nonexistent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let error = sandbox(dir.path())
            .list_files(&path_str(&dir.path().join("no-such-dir")))
            .await
            .unwrap_err();
        assert!(matches!(error, Error::SandboxPathNotFound(_)));
    }

    #[tokio::test]
    async fn throws_sandbox_path_not_found_error_when_a_path_component_is_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let sandbox = sandbox(dir.path());
        sandbox
            .write_text(&path_str(&dir.path().join("file.txt")), "x")
            .await
            .unwrap();
        let error = sandbox
            .list_files(&path_str(&dir.path().join("file.txt").join("nested")))
            .await
            .unwrap_err();
        assert!(matches!(error, Error::SandboxPathNotFound(_)));
    }

    #[tokio::test]
    async fn writes_absolute_paths_as_is() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let file = other.path().join("abs.txt");
        sandbox(dir.path())
            .write_text(&path_str(&file), "absolute")
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(file).unwrap(), "absolute");
    }

    #[tokio::test]
    async fn resolves_relative_paths_against_process_cwd() {
        let dir = tempfile::tempdir().unwrap();
        sandbox(dir.path())
            .write_text("rel-probe.txt", "relative")
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("rel-probe.txt")).unwrap(),
            "relative"
        );
    }

    #[test]
    fn builds_the_posix_command_with_cwd_and_env_prefix() {
        let spec = crate::sandbox::not_a_sandbox_local_environment::command_spec(
            false,
            "echo hi",
            "/work dir",
            env(&[("A", "1")]).as_ref(),
            Vec::new,
        )
        .unwrap();
        assert_eq!(spec.program, "sh");
        assert_eq!(spec.args, ["-c", "cd '/work dir' && export A='1' && echo hi"]);
        assert!(spec.env.is_none());
        assert!(spec.cwd.is_none());
    }

    fn host() -> Vec<(String, String)> {
        vec![
            ("Path".to_owned(), "C:\\bin".to_owned()),
            ("HOME".to_owned(), "h".to_owned()),
        ]
    }

    #[test]
    fn builds_the_powershell_command_with_cwd_and_env_on_the_process() {
        let spec = crate::sandbox::not_a_sandbox_local_environment::command_spec(
            true,
            "Write-Output hi",
            "C:\\work",
            env(&[("PATH", "C:\\new")]).as_ref(),
            host,
        )
        .unwrap();
        assert_eq!(spec.program, "powershell.exe");
        assert_eq!(
            spec.args[..3],
            ["-NoProfile", "-NonInteractive", "-EncodedCommand"]
        );
        assert_eq!(spec.cwd.as_deref(), Some(Path::new("C:\\work")));
        assert_eq!(
            spec.env.unwrap(),
            [
                ("HOME".to_owned(), "h".to_owned()),
                ("PATH".to_owned(), "C:\\new".to_owned())
            ]
        );
        assert_eq!(
            spec.enoent_message.as_deref(),
            Some("PowerShell not found: powershell.exe is required to run commands on Windows")
        );
    }

    #[test]
    fn encodes_the_powershell_script_as_utf16le_base64() {
        use base64::Engine;
        let args = crate::sandbox::not_a_sandbox_local_environment::powershell_args("Write-Output \"€\"");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&args[3])
            .unwrap();
        let units: Vec<u16> = bytes
            .chunks(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        assert_eq!(
            String::from_utf16(&units).unwrap(),
            "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8\n\
             $OutputEncoding = [System.Text.UTF8Encoding]::new($false)\n\
             Write-Output \"€\"\n\
             $ok = $?\n\
             if (-not $ok) { if ($LASTEXITCODE) { exit $LASTEXITCODE } else { exit 1 } }"
        );
    }

    #[test]
    fn rejects_invalid_env_names_on_windows() {
        let error = crate::sandbox::not_a_sandbox_local_environment::command_spec(
            true,
            "x",
            "C:\\",
            env(&[("BAD-KEY", "v")]).as_ref(),
            host,
        )
        .err()
        .unwrap();
        assert_eq!(error.to_string(), "Invalid environment variable name: BAD-KEY");
    }

    #[test]
    fn spawns_the_interpreter_with_the_code_on_stdin_on_windows() {
        let spec = crate::sandbox::not_a_sandbox_local_environment::code_spec(
            true, "print(1)", "python", "C:\\work", None, host,
        )
        .unwrap();
        assert_eq!(spec.program, "python");
        assert!(spec.args.is_empty());
        assert_eq!(spec.input.as_deref(), Some("print(1)"));
        assert_eq!(
            spec.enoent_message.as_deref(),
            Some("Language interpreter not found: python")
        );
    }

    #[test]
    fn overrides_a_host_variable_whose_name_differs_only_by_case() {
        let merged = crate::sandbox::not_a_sandbox_local_environment::host_env_with(
            vec![("Strands_Case_Probe".to_owned(), "host".to_owned())],
            env(&[("STRANDS_CASE_PROBE", "override")]).as_ref(),
        );
        assert_eq!(merged, [("STRANDS_CASE_PROBE".to_owned(), "override".to_owned())]);
    }
}

mod read_file_errors {
    use super::*;

    fn backends(dir: &Path) -> Vec<(&'static str, Box<dyn Sandbox>)> {
        vec![
            (
                "NotASandboxLocalEnvironment",
                Box::new(local_env(dir, never_sleeper())),
            ),
            ("PosixShellSandbox", Box::new(TestSandbox::new(dir))),
        ]
    }

    fn setup() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file.txt"), "x").unwrap();
        dir
    }

    #[tokio::test]
    async fn throws_sandbox_path_not_found_error_for_a_missing_file() {
        let dir = setup();
        let target = path_str(&dir.path().join("nope.txt"));
        for (backend, sandbox) in backends(dir.path()) {
            let error = sandbox.read_file(&target).await.unwrap_err();
            assert!(
                matches!(&error, Error::SandboxPathNotFound(e) if e.path() == target),
                "{backend}: {error}"
            );
        }
    }

    #[tokio::test]
    async fn throws_sandbox_path_not_found_error_when_an_ancestor_is_a_file() {
        let dir = setup();
        let target = path_str(&dir.path().join("file.txt/child.txt"));
        for (backend, sandbox) in backends(dir.path()) {
            let error = sandbox.read_file(&target).await.unwrap_err();
            assert!(
                matches!(&error, Error::SandboxPathNotFound(e) if e.path() == target),
                "{backend}: {error}"
            );
        }
    }

    #[tokio::test]
    async fn throws_sandbox_is_a_directory_error_for_a_directory() {
        let dir = setup();
        let target = path_str(dir.path());
        for (backend, sandbox) in backends(dir.path()) {
            let error = sandbox.read_file(&target).await.unwrap_err();
            assert!(
                matches!(&error, Error::SandboxIsADirectory(e) if e.path() == target),
                "{backend}: {error}"
            );
        }
    }

    #[tokio::test]
    async fn throws_sandbox_permission_error_for_an_unreadable_file() {
        let dir = setup();
        if ignores_permissions(dir.path()) {
            return;
        }
        let target = path_str(&dir.path().join("file.txt"));
        chmod(&dir.path().join("file.txt"), 0o200);
        for (backend, sandbox) in backends(dir.path()) {
            let error = sandbox.read_file(&target).await.unwrap_err();
            assert!(
                matches!(&error, Error::SandboxPermission(e) if e.path() == target),
                "{backend}: {error}"
            );
        }
    }

    #[tokio::test]
    async fn throws_sandbox_permission_error_for_an_unreadable_directory() {
        let dir = setup();
        if ignores_permissions(dir.path()) {
            return;
        }
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        chmod(&locked, 0o000);
        let target = path_str(&locked);
        for (backend, sandbox) in backends(dir.path()) {
            let error = sandbox.read_file(&target).await.unwrap_err();
            let ok = matches!(&error, Error::SandboxPermission(e) if e.path() == target)
                // Native reads open a directory before reading it, so the local backend sees EISDIR.
                || (backend == "NotASandboxLocalEnvironment" && matches!(&error, Error::SandboxIsADirectory(_)));
            assert!(ok, "{backend}: {error}");
        }
        chmod(&locked, 0o700);
    }

    #[tokio::test]
    async fn throws_sandbox_permission_error_under_an_unsearchable_directory() {
        let dir = setup();
        if ignores_permissions(dir.path()) {
            return;
        }
        let inner = dir.path().join("locked/inner");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(inner.join("secret.txt"), "x").unwrap();
        chmod(&dir.path().join("locked"), 0o600);
        let target = path_str(&inner.join("secret.txt"));
        for (backend, sandbox) in backends(dir.path()) {
            let error = sandbox.read_file(&target).await.unwrap_err();
            assert!(
                matches!(&error, Error::SandboxPermission(e) if e.path() == target),
                "{backend}: {error}"
            );
        }
        chmod(&dir.path().join("locked"), 0o700);
    }
}

mod stream_process {
    use super::super::stream_process::{ProcessLimits, ProcessSpec, Utf8Decoder, stream_process};
    use super::*;

    fn limits() -> ProcessLimits {
        ProcessLimits {
            timeout: None,
            cancel_signal: None,
            sleeper: never_sleeper(),
        }
    }

    fn sh(script: &str) -> ProcessSpec {
        ProcessSpec {
            program: "sh".to_owned(),
            args: vec!["-c".to_owned(), script.to_owned()],
            ..ProcessSpec::default()
        }
    }

    async fn run(spec: ProcessSpec) -> ExecutionResult {
        let mut stream = stream_process(spec, limits());
        while let Some(event) = stream.next().await {
            if let ExecutionEvent::ExecutionResult(result) = event.unwrap() {
                return result;
            }
        }
        panic!("no result");
    }

    #[tokio::test]
    async fn runs_the_child_in_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let result = run(ProcessSpec {
            cwd: Some(dir.path().to_path_buf()),
            ..sh("pwd")
        })
        .await;
        assert_eq!(
            std::fs::canonicalize(result.stdout.trim()).unwrap(),
            std::fs::canonicalize(dir.path()).unwrap()
        );
    }

    #[tokio::test]
    async fn replaces_the_environment_with_env() {
        let path = std::env::var("PATH").unwrap_or_default();
        let result = run(ProcessSpec {
            env: Some(vec![
                ("GREETING".to_owned(), "hi".to_owned()),
                ("PATH".to_owned(), path),
            ]),
            ..sh("echo \"$GREETING:$HOME\"")
        })
        .await;
        assert_eq!(result.stdout, "hi:\n");
    }

    #[tokio::test]
    async fn feeds_input_to_stdin_and_closes_it() {
        let result = run(ProcessSpec {
            program: "cat".to_owned(),
            input: Some("hello from stdin".to_owned()),
            ..ProcessSpec::default()
        })
        .await;
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "hello from stdin");
    }

    #[tokio::test]
    async fn does_not_fail_when_the_child_exits_without_reading_its_input() {
        let result = run(ProcessSpec {
            input: Some("x".repeat(1 << 20)),
            ..sh("exit 0")
        })
        .await;
        assert_eq!(result.exit_code, 0);
    }

    #[tokio::test]
    async fn leaves_stdin_open_when_no_input_is_given() {
        let mut stream = stream_process(
            sh("cat"),
            ProcessLimits {
                timeout: Some(Duration::from_secs(1)),
                cancel_signal: None,
                sleeper: instant_sleeper(),
            },
        );
        let error = loop {
            match stream.next().await.unwrap() {
                Ok(ExecutionEvent::ExecutionResult(_)) => panic!("cat saw EOF"),
                Ok(_) => {}
                Err(error) => break error,
            }
        };
        assert!(matches!(error, Error::SandboxTimeout(_)));
    }

    #[tokio::test]
    async fn returns_enoent_message_when_spawned_binary_does_not_exist() {
        let result = run(ProcessSpec {
            program: "nonexistent_binary_xyz_12345".to_owned(),
            enoent_message: Some("binary not found".to_owned()),
            ..ProcessSpec::default()
        })
        .await;
        assert_eq!(result, ExecutionResult::new(127, "", "binary not found"));
    }

    #[tokio::test]
    async fn fails_when_a_spawned_binary_does_not_exist_without_an_enoent_message() {
        let mut stream = stream_process(
            ProcessSpec {
                program: "nonexistent_binary_xyz_12345".to_owned(),
                ..ProcessSpec::default()
            },
            limits(),
        );
        assert!(stream.next().await.unwrap().is_err());
    }

    #[test]
    fn decodes_multibyte_utf_8_sequences_split_across_chunk_boundaries() {
        let bytes = "café☕".as_bytes();
        let mut decoder = Utf8Decoder::default();
        let first = decoder.decode(&bytes[..4]);
        let second = decoder.decode(&bytes[4..]);
        assert_eq!(first, "caf");
        assert_eq!(format!("{first}{second}{}", decoder.finish()), "café☕");
    }

    #[test]
    fn replaces_invalid_utf8_with_the_replacement_character() {
        let mut decoder = Utf8Decoder::default();
        assert_eq!(decoder.decode(b"a\xffb"), "a\u{FFFD}b");
        assert_eq!(decoder.decode(b"\xe2\x98"), "");
        assert_eq!(decoder.finish(), "\u{FFFD}");
    }
}

mod posix_shell {
    use super::*;

    fn setup() -> (tempfile::TempDir, Box<dyn Sandbox>) {
        let dir = tempfile::tempdir().unwrap();
        let sandbox = shell_sandbox(TestSandbox::new(dir.path()));
        (dir, sandbox)
    }

    fn shell_sandbox(sandbox: TestSandbox) -> Box<dyn Sandbox> {
        Box::new(sandbox)
    }

    #[tokio::test]
    async fn runs_a_command() {
        let (_dir, sandbox) = setup();
        let result = sandbox
            .execute("echo hello", ExecuteOptions::default())
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "hello\n");
    }

    #[tokio::test]
    async fn runs_in_working_dir() {
        let (dir, sandbox) = setup();
        let result = sandbox.execute("pwd", ExecuteOptions::default()).await.unwrap();
        assert_eq!(
            std::fs::canonicalize(result.stdout.trim()).unwrap(),
            std::fs::canonicalize(dir.path()).unwrap()
        );
    }

    #[tokio::test]
    async fn respects_cwd_option() {
        let (_dir, sandbox) = setup();
        let result = sandbox.execute("pwd", with_cwd(Path::new("/tmp"))).await.unwrap();
        assert!(result.stdout.trim().ends_with("/tmp"), "{}", result.stdout);
    }

    #[tokio::test]
    async fn passes_env_vars_to_the_command() {
        let (_dir, sandbox) = setup();
        let result = sandbox
            .execute("printenv FOO", with_env(&[("FOO", "hello"), ("BAR", "world")]))
            .await
            .unwrap();
        assert_eq!(result.stdout.trim(), "hello");
    }

    #[tokio::test]
    async fn shell_quotes_env_values_so_they_are_not_evaluated() {
        let (_dir, sandbox) = setup();
        let result = sandbox
            .execute("printenv FOO", with_env(&[("FOO", "$(whoami)")]))
            .await
            .unwrap();
        assert_eq!(result.stdout.trim(), "$(whoami)");
    }

    #[tokio::test]
    async fn rejects_invalid_env_var_names() {
        let (_dir, sandbox) = setup();
        let error = sandbox
            .execute("echo hi", with_env(&[("BAD-KEY", "v")]))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Invalid environment variable name"));
    }

    #[test]
    fn returns_an_empty_string_when_there_are_no_env_vars() {
        assert_eq!(build_shell_env_prefix(None).unwrap(), "");
        assert_eq!(build_shell_env_prefix(Some(&BTreeMap::new())).unwrap(), "");
    }

    #[test]
    fn uses_export_so_vars_are_inherited_across_a_pipeline_with_a_fail_fast_separator() {
        assert_eq!(
            build_shell_env_prefix(env(&[("FOO", "bar"), ("BAZ", "qux")]).as_ref()).unwrap(),
            "export BAZ='qux' FOO='bar' && "
        );
    }

    #[test]
    fn shell_quotes_values_so_they_are_not_evaluated() {
        assert_eq!(
            build_shell_env_prefix(env(&[("FOO", "$(whoami)")]).as_ref()).unwrap(),
            "export FOO='$(whoami)' && "
        );
    }

    #[test]
    fn throws_on_invalid_env_var_names() {
        let error = build_shell_env_prefix(env(&[("BAD-KEY", "v")]).as_ref()).unwrap_err();
        assert_eq!(error.to_string(), "Invalid environment variable name: BAD-KEY");
    }

    #[tokio::test]
    async fn runs_python_code_through_shell() {
        let (_dir, sandbox) = setup();
        let result = sandbox
            .execute_code("print(2 + 2)", "python3", ExecuteOptions::default())
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "4\n");
    }

    #[tokio::test]
    async fn handles_code_with_special_characters() {
        let (_dir, sandbox) = setup();
        let result = sandbox
            .execute_code("print('hello \"world\"')", "python3", ExecuteOptions::default())
            .await
            .unwrap();
        assert_eq!(result.stdout, "hello \"world\"\n");
    }

    #[tokio::test]
    async fn handles_code_with_single_quotes() {
        let (_dir, sandbox) = setup();
        let result = sandbox
            .execute_code("print(\"it's working\")", "python3", ExecuteOptions::default())
            .await
            .unwrap();
        assert_eq!(result.stdout, "it's working\n");
    }

    #[tokio::test]
    async fn passes_env_vars_through_to_the_interpreter() {
        let (_dir, sandbox) = setup();
        let result = sandbox
            .execute_code(
                "import os; print(os.environ[\"FOO\"])",
                "python3",
                with_env(&[("FOO", "from-env")]),
            )
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "from-env");
    }

    #[tokio::test]
    async fn passes_env_values_to_the_interpreter_literally_without_shell_evaluation() {
        let (_dir, sandbox) = setup();
        let result = sandbox
            .execute_code(
                "import os; print(os.environ[\"FOO\"])",
                "python3",
                with_env(&[("FOO", "$(whoami)")]),
            )
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "$(whoami)");
    }

    async fn language_error(language: &str) -> String {
        let (_dir, sandbox) = setup();
        sandbox
            .execute_code("x", language, ExecuteOptions::default())
            .await
            .unwrap_err()
            .to_string()
    }

    #[tokio::test]
    async fn rejects_path_traversal() {
        assert!(
            language_error("../../../bin/sh")
                .await
                .contains("invalid characters")
        );
    }

    #[tokio::test]
    async fn rejects_shell_metacharacters() {
        assert!(
            language_error("python;rm -rf /")
                .await
                .contains("invalid characters")
        );
    }

    #[tokio::test]
    async fn rejects_spaces() {
        assert_eq!(
            language_error("python -c").await,
            "language parameter contains invalid characters: python -c"
        );
    }

    #[tokio::test]
    async fn allows_valid_interpreters() {
        let (_dir, sandbox) = setup();
        let result = sandbox
            .execute_code("print(\"safe\")", "python3", ExecuteOptions::default())
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);
    }

    #[tokio::test]
    async fn allows_dots_and_hyphens() {
        let (_dir, sandbox) = setup();
        let result = sandbox
            .execute_code("x", "fake-lang.99", ExecuteOptions::default())
            .await
            .unwrap();
        assert_eq!(result.exit_code, 127);
    }

    #[tokio::test]
    async fn text_file_roundtrip() {
        let (_dir, sandbox) = setup();
        sandbox.write_text("test.txt", "hello shell").await.unwrap();
        assert_eq!(sandbox.read_text("test.txt").await.unwrap(), "hello shell");
    }

    #[tokio::test]
    async fn binary_file_roundtrip() {
        let (_dir, sandbox) = setup();
        let bytes = [0u8, 1, 2, 127, 128, 254, 255];
        sandbox.write_file("binary.bin", &bytes).await.unwrap();
        assert_eq!(sandbox.read_file("binary.bin").await.unwrap(), bytes);
    }

    #[tokio::test]
    async fn all_256_byte_values_roundtrip() {
        let (_dir, sandbox) = setup();
        let bytes: Vec<u8> = (0..=255).collect();
        sandbox.write_file("all-bytes.bin", &bytes).await.unwrap();
        assert_eq!(sandbox.read_file("all-bytes.bin").await.unwrap(), bytes);
    }

    #[tokio::test]
    async fn creates_parent_directories() {
        let (_dir, sandbox) = setup();
        sandbox.write_text("deep/nested/file.txt", "deep").await.unwrap();
        assert_eq!(sandbox.read_text("deep/nested/file.txt").await.unwrap(), "deep");
    }

    #[tokio::test]
    async fn handles_unicode_content() {
        let (_dir, sandbox) = setup();
        let content = "日本語 🚀 émojis";
        sandbox.write_text("unicode.txt", content).await.unwrap();
        assert_eq!(sandbox.read_text("unicode.txt").await.unwrap(), content);
    }

    #[tokio::test]
    async fn handles_shell_metacharacters_in_content() {
        let (_dir, sandbox) = setup();
        let content = "$(rm -rf /) `whoami` && || $HOME";
        sandbox.write_text("meta.txt", content).await.unwrap();
        assert_eq!(sandbox.read_text("meta.txt").await.unwrap(), content);
    }

    #[tokio::test]
    async fn throws_sandbox_path_not_found_error_on_nonexistent_file() {
        let (_dir, sandbox) = setup();
        let error = sandbox.read_file("nope.txt").await.unwrap_err();
        let Error::SandboxPathNotFound(error) = error else {
            panic!("expected a path-not-found error");
        };
        assert_eq!(error.to_string(), "Path not found: nope.txt");
        assert_eq!(error.code(), "ENOENT");
    }

    #[tokio::test]
    async fn throws_sandbox_is_a_directory_error_not_sandbox_path_not_found_error_on_a_directory() {
        let (dir, sandbox) = setup();
        std::fs::create_dir(dir.path().join("subdir")).unwrap();
        let error = sandbox.read_file("subdir").await.unwrap_err();
        assert!(
            matches!(&error, Error::SandboxIsADirectory(e) if e.path() == "subdir"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn throws_sandbox_permission_error_not_sandbox_path_not_found_error_for_a_file_under_an_unsearchable_directory()
     {
        let (dir, sandbox) = setup();
        if ignores_permissions(dir.path()) {
            return;
        }
        std::fs::create_dir(dir.path().join("locked")).unwrap();
        std::fs::write(dir.path().join("locked/secret.txt"), "x").unwrap();
        chmod(&dir.path().join("locked"), 0o600);
        let error = sandbox.read_file("locked/secret.txt").await.unwrap_err();
        chmod(&dir.path().join("locked"), 0o700);
        assert!(
            matches!(&error, Error::SandboxPermission(e) if e.path() == "locked/secret.txt"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn throws_sandbox_permission_error_for_a_file_under_an_unsearchable_ancestor() {
        let (dir, sandbox) = setup();
        if ignores_permissions(dir.path()) {
            return;
        }
        std::fs::create_dir_all(dir.path().join("locked/inner")).unwrap();
        std::fs::write(dir.path().join("locked/inner/secret.txt"), "x").unwrap();
        chmod(&dir.path().join("locked"), 0o600);
        let error = sandbox.read_file("locked/inner/secret.txt").await.unwrap_err();
        chmod(&dir.path().join("locked"), 0o700);
        assert!(matches!(&error, Error::SandboxPermission(_)), "{error}");
    }

    #[tokio::test]
    async fn throws_sandbox_path_not_found_error_when_an_ancestor_is_missing_or_a_file() {
        let (dir, sandbox) = setup();
        std::fs::write(dir.path().join("file.txt"), "x").unwrap();
        let absolute = path_str(&dir.path().join("missing/b.txt"));
        for path in ["missing/a/b.txt", "file.txt/b.txt", absolute.as_str()] {
            let error = sandbox.read_file(path).await.unwrap_err();
            assert!(matches!(error, Error::SandboxPathNotFound(_)), "{path}: {error}");
        }
    }

    #[tokio::test]
    async fn throws_sandbox_permission_error_for_an_unreadable_file() {
        let (dir, sandbox) = setup();
        if ignores_permissions(dir.path()) {
            return;
        }
        std::fs::write(dir.path().join("secret.txt"), "x").unwrap();
        chmod(&dir.path().join("secret.txt"), 0o200);
        let error = sandbox.read_file("secret.txt").await.unwrap_err();
        assert!(
            matches!(&error, Error::SandboxPermission(e) if e.path() == "secret.txt"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn does_not_report_a_failed_cd_as_a_missing_file() {
        let (dir, _sandbox) = setup();
        std::fs::write(dir.path().join("real.txt"), "x").unwrap();
        let bad_cwd = shell_sandbox(TestSandbox::new(dir.path().join("missing-workdir")));
        let error = bad_cwd
            .read_file(&path_str(&dir.path().join("real.txt")))
            .await
            .unwrap_err();
        assert!(!matches!(error, Error::SandboxPathNotFound(_)), "{error}");
    }

    #[tokio::test]
    async fn treats_a_backslash_in_a_filename_as_a_literal_character() {
        let (_dir, sandbox) = setup();
        sandbox.write_text("weird\\name.txt", "ok").await.unwrap();
        assert_eq!(sandbox.read_text("weird\\name.txt").await.unwrap(), "ok");
        let entries = sandbox.list_files(".").await.unwrap();
        assert!(entries.iter().any(|entry| entry.name == "weird\\name.txt"));
    }

    #[tokio::test]
    async fn removes_a_file() {
        let (_dir, sandbox) = setup();
        sandbox.write_text("delete-me.txt", "bye").await.unwrap();
        sandbox.remove_file("delete-me.txt").await.unwrap();
        assert!(sandbox.read_file("delete-me.txt").await.is_err());
    }

    #[tokio::test]
    async fn throws_on_nonexistent_file() {
        let (_dir, sandbox) = setup();
        assert!(sandbox.remove_file("nope.txt").await.is_err());
    }

    #[tokio::test]
    async fn lists_directory_contents() {
        let (_dir, sandbox) = setup();
        sandbox.write_text("a.txt", "a").await.unwrap();
        sandbox.write_text("b.txt", "b").await.unwrap();
        let names: Vec<String> = sandbox
            .list_files(".")
            .await
            .unwrap()
            .into_iter()
            .map(|f| f.name)
            .collect();
        assert!(names.contains(&"a.txt".to_owned()));
        assert!(names.contains(&"b.txt".to_owned()));
    }

    #[tokio::test]
    async fn identifies_directories() {
        let (_dir, sandbox) = setup();
        sandbox
            .execute("mkdir -p subdir", ExecuteOptions::default())
            .await
            .unwrap();
        let files = sandbox.list_files(".").await.unwrap();
        let subdir = files.iter().find(|f| f.name == "subdir").unwrap();
        assert_eq!(subdir.is_dir, Some(true));
    }

    #[tokio::test]
    async fn excludes_dot_and_dot_dot_entries() {
        let (_dir, sandbox) = setup();
        sandbox.write_text("file.txt", "").await.unwrap();
        let names: Vec<String> = sandbox
            .list_files(".")
            .await
            .unwrap()
            .into_iter()
            .map(|f| f.name)
            .collect();
        assert!(!names.contains(&".".to_owned()));
        assert!(!names.contains(&"..".to_owned()));
    }

    #[tokio::test]
    async fn throws_sandbox_path_not_found_error_on_nonexistent_directory() {
        let (dir, sandbox) = setup();
        let error = sandbox
            .list_files(&path_str(&dir.path().join("nonexistent-dir-xyz")))
            .await
            .unwrap_err();
        assert!(matches!(error, Error::SandboxPathNotFound(_)));
    }

    #[tokio::test]
    async fn throws_sandbox_path_not_found_error_when_path_is_a_file_not_a_directory() {
        let (_dir, sandbox) = setup();
        sandbox.write_text("not-a-dir.txt", "hello").await.unwrap();
        let error = sandbox.list_files("not-a-dir.txt").await.unwrap_err();
        assert!(matches!(error, Error::SandboxPathNotFound(_)));
    }

    #[tokio::test]
    async fn handles_paths_with_spaces() {
        let (_dir, sandbox) = setup();
        sandbox
            .execute("mkdir -p \"with spaces\"", ExecuteOptions::default())
            .await
            .unwrap();
        sandbox
            .write_text("with spaces/file.txt", "spaced")
            .await
            .unwrap();
        assert_eq!(sandbox.read_text("with spaces/file.txt").await.unwrap(), "spaced");
    }

    #[tokio::test]
    async fn handles_paths_with_single_quotes() {
        let (_dir, sandbox) = setup();
        sandbox
            .execute("mkdir -p \"it's\"", ExecuteOptions::default())
            .await
            .unwrap();
        sandbox.write_text("it's/file.txt", "quoted").await.unwrap();
        assert_eq!(sandbox.read_text("it's/file.txt").await.unwrap(), "quoted");
    }

    #[tokio::test]
    async fn kills_process_on_timeout() {
        let (dir, _sandbox) = setup();
        let sandbox = shell_sandbox(TestSandbox::new(dir.path()).with_sleeper(instant_sleeper()));
        let error = sandbox.execute("sleep 60", with_timeout(0.2)).await.unwrap_err();
        assert_eq!(error.to_string(), "Execution timed out after 0.2 seconds");
    }

    #[tokio::test]
    async fn reports_the_output_captured_before_the_kill_on_timeout() {
        let (dir, _sandbox) = setup();
        let notify = Arc::new(Notify::new());
        let delays = Arc::new(Mutex::new(Vec::new()));
        let sandbox = NotifyOnOutput {
            inner: TestSandbox::new(dir.path())
                .with_sleeper(manual_sleeper(Arc::clone(&notify), Arc::clone(&delays))),
            notify,
            expected: vec![(StreamType::Stdout, "partial\n"), (StreamType::Stderr, "warn\n")],
        };
        let error = sandbox
            .execute("echo partial; echo warn >&2; sleep 5", with_timeout(0.3))
            .await
            .unwrap_err();
        let Error::SandboxTimeout(error) = error else {
            panic!("expected a timeout");
        };
        assert_eq!(error.stdout(), "partial\n");
        assert_eq!(error.stderr(), "warn\n");
        assert_eq!(delays.lock().unwrap()[0], Duration::from_secs_f64(0.3));
    }

    #[tokio::test]
    async fn does_not_timeout_fast_commands() {
        let (_dir, sandbox) = setup();
        let result = sandbox.execute("echo fast", with_timeout(5.0)).await.unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "fast\n");
    }

    #[tokio::test]
    async fn kills_the_whole_process_group_on_timeout() {
        let (dir, _sandbox) = setup();
        let pid_file = dir.path().join("pid");
        let notify = Arc::new(Notify::new());
        let delays = Arc::new(Mutex::new(Vec::new()));
        let sandbox = NotifyOnOutput {
            inner: TestSandbox::new(dir.path()).with_sleeper(manual_sleeper(Arc::clone(&notify), delays)),
            notify,
            expected: vec![(StreamType::Stdout, "started")],
        };
        let command = format!(
            "sleep 100 & echo $! > '{}'; echo started; sleep 100",
            pid_file.display()
        );
        let error = sandbox.execute(&command, with_timeout(1.0)).await.unwrap_err();
        assert!(matches!(error, Error::SandboxTimeout(_)));
        let pid = std::fs::read_to_string(&pid_file).unwrap().trim().to_owned();
        assert!(
            wait_until_gone(&pid).await,
            "background sleep {pid} survived the timeout"
        );
    }

    /// Polls until `pid` no longer runs; process exit after a kill is asynchronous.
    async fn wait_until_gone(pid: &str) -> bool {
        for _ in 0..500 {
            let alive = std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .map(|stat| !stat.contains(") Z "))
                .unwrap_or(false);
            let alive = alive
                || (!Path::new("/proc").exists()
                    && std::process::Command::new("kill")
                        .args(["-0", pid])
                        .status()
                        .is_ok_and(|s| s.success()));
            if !alive {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    #[tokio::test]
    async fn kills_process_when_signal_is_aborted() {
        let (_dir, sandbox) = setup();
        let (signal, handle) = crate::CancelSignal::new();
        let options = ExecuteOptions {
            cancel_signal: Some(signal),
            ..ExecuteOptions::default()
        };
        let mut stream = sandbox.execute_streaming("echo started; sleep 60", options);
        let first = stream.next().await.unwrap().unwrap();
        assert!(matches!(first, ExecutionEvent::StreamChunk(_)));
        handle.cancel(crate::CancelOptions::default());
        let error = stream.next().await.unwrap().unwrap_err();
        assert_eq!(error.to_string(), "Execution aborted");
    }

    #[tokio::test]
    async fn rejects_immediately_if_signal_is_already_aborted() {
        let (_dir, sandbox) = setup();
        let (signal, handle) = crate::CancelSignal::new();
        handle.cancel(crate::CancelOptions::default());
        let options = ExecuteOptions {
            cancel_signal: Some(signal),
            ..ExecuteOptions::default()
        };
        let error = sandbox.execute("sleep 60", options).await.unwrap_err();
        assert!(matches!(error, Error::SandboxAbort(_)));
    }

    #[tokio::test]
    async fn handles_multiple_concurrent_commands() {
        let (_dir, sandbox) = setup();
        let (one, two, three) = tokio::join!(
            sandbox.execute("echo one", ExecuteOptions::default()),
            sandbox.execute("echo two", ExecuteOptions::default()),
            sandbox.execute("echo three", ExecuteOptions::default()),
        );
        let mut outputs: Vec<String> = [one, two, three]
            .into_iter()
            .map(|r| r.unwrap().stdout.trim().to_owned())
            .collect();
        outputs.sort();
        assert_eq!(outputs, ["one", "three", "two"]);
    }

    #[tokio::test]
    async fn handles_concurrent_file_writes_to_different_files() {
        let (_dir, sandbox) = setup();
        let (a, b, c) = tokio::join!(
            sandbox.write_text("a.txt", "aaa"),
            sandbox.write_text("b.txt", "bbb"),
            sandbox.write_text("c.txt", "ccc"),
        );
        a.unwrap();
        b.unwrap();
        c.unwrap();
        let (a, b, c) = tokio::join!(
            sandbox.read_text("a.txt"),
            sandbox.read_text("b.txt"),
            sandbox.read_text("c.txt"),
        );
        assert_eq!(
            (a.unwrap(), b.unwrap(), c.unwrap()),
            ("aaa".into(), "bbb".into(), "ccc".into())
        );
    }

    #[tokio::test]
    async fn yields_stream_chunks_then_execution_result() {
        let (_dir, sandbox) = setup();
        let events: Vec<ExecutionEvent> = sandbox
            .execute_streaming("echo hello", ExecuteOptions::default())
            .map(|event| event.unwrap())
            .collect()
            .await;
        let chunks = events
            .iter()
            .filter(|e| matches!(e, ExecutionEvent::StreamChunk(_)))
            .count();
        let results = events
            .iter()
            .filter(|e| matches!(e, ExecutionEvent::ExecutionResult(_)))
            .count();
        assert!(chunks > 0);
        assert_eq!(results, 1);
        assert!(matches!(events.last(), Some(ExecutionEvent::ExecutionResult(_))));
    }

    #[tokio::test]
    async fn returns_exit_code_127_when_command_is_not_found() {
        let (_dir, sandbox) = setup();
        let result = sandbox
            .execute("nonexistent_binary_xyz_12345", ExecuteOptions::default())
            .await
            .unwrap();
        assert_eq!(result.exit_code, 127);
        assert!(result.stderr.contains("not found"));
    }

    #[tokio::test]
    async fn maps_signal_termination_to_128_signal_number() {
        let (_dir, sandbox) = setup();
        let result = sandbox
            .execute("sh -c 'kill -9 $$'", ExecuteOptions::default())
            .await
            .unwrap();
        assert_eq!(result.exit_code, 137);
    }

    #[tokio::test]
    async fn dropping_the_stream_kills_the_process_group() {
        let (dir, sandbox) = setup();
        let pid_file = dir.path().join("pid");
        let command = format!("echo $$ > '{}'; echo ready; sleep 100", pid_file.display());
        let mut stream = sandbox.execute_streaming(&command, ExecuteOptions::default());
        assert!(matches!(
            stream.next().await.unwrap().unwrap(),
            ExecutionEvent::StreamChunk(_)
        ));
        drop(stream);
        let pid = std::fs::read_to_string(&pid_file).unwrap().trim().to_owned();
        assert!(wait_until_gone(&pid).await, "shell {pid} survived the drop");
    }
}
