//! Live sandbox integration tests for the Docker and SSH backends. They need a real container /
//! remote host, so they are `#[ignore]`d; run with:
//!
//! ```text
//! docker run -d --name strands-sandbox-test alpine sleep 3600
//! STRANDS_TEST_DOCKER_CONTAINER=strands-sandbox-test \
//!   cargo test --all-features --test integ_sandboxes -- --ignored
//!
//! STRANDS_TEST_SSH_HOST=example.com STRANDS_TEST_SSH_USER=ubuntu \
//!   STRANDS_TEST_SSH_KEY=~/.ssh/id_ed25519 STRANDS_TEST_SSH_PORT=22 \
//!   STRANDS_TEST_SSH_ALLOW_UNKNOWN_HOSTS=1 \
//!   cargo test --all-features --test integ_sandboxes -- --ignored
//! ```

use strands_agents::sandbox::{DockerSandbox, ExecuteOptions, Sandbox, SandboxError, SshSandbox};

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn on_path(binary: &str) -> bool {
    std::process::Command::new(binary)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

/// `None` unless `docker` is installed and `STRANDS_TEST_DOCKER_CONTAINER` names a running
/// container, so live tests skip instead of failing.
fn docker() -> Option<DockerSandbox> {
    if !on_path("docker") {
        eprintln!("skipping: docker is not on PATH");
        return None;
    }
    let Some(container) = var("STRANDS_TEST_DOCKER_CONTAINER") else {
        eprintln!("skipping: STRANDS_TEST_DOCKER_CONTAINER is not set");
        return None;
    };
    Some(DockerSandbox::new(container).with_working_dir("/tmp"))
}

/// `None` unless `ssh` is installed and `STRANDS_TEST_SSH_HOST` is set. `_USER`, `_PORT`, `_KEY`
/// and `_ALLOW_UNKNOWN_HOSTS` are optional.
fn ssh() -> Option<SshSandbox> {
    if !on_path("ssh") {
        eprintln!("skipping: ssh is not on PATH");
        return None;
    }
    let Some(host) = var("STRANDS_TEST_SSH_HOST") else {
        eprintln!("skipping: STRANDS_TEST_SSH_HOST is not set");
        return None;
    };
    let destination = match var("STRANDS_TEST_SSH_USER") {
        Some(user) => format!("{user}@{host}"),
        None => host,
    };
    let mut sandbox = SshSandbox::new(destination, "/tmp");
    if let Some(port) = var("STRANDS_TEST_SSH_PORT").and_then(|p| p.parse().ok()) {
        sandbox = sandbox.with_port(port);
    }
    if let Some(key) = var("STRANDS_TEST_SSH_KEY") {
        sandbox = sandbox.with_identity_file(key);
    }
    // Default `accept-new` would record the test host in the runner's `known_hosts`; opt out when
    // the target's key is not pinned anywhere.
    if var("STRANDS_TEST_SSH_ALLOW_UNKNOWN_HOSTS").is_some() {
        sandbox = sandbox.with_allow_unknown_hosts(true);
    }
    Some(sandbox)
}

/// Every `Sandbox` method against a real backend: command, env, cwd, code, and file round-trip.
async fn exercise(sandbox: &dyn Sandbox) {
    let result = sandbox
        .execute("echo hello", ExecuteOptions::default())
        .await
        .expect("execute");
    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    assert_eq!(result.stdout.trim(), "hello");

    let failed = sandbox
        .execute("exit 3", ExecuteOptions::default())
        .await
        .expect("execute");
    assert_eq!(failed.exit_code, 3);

    let env = sandbox
        .execute(
            "echo $STRANDS_LIVE",
            ExecuteOptions {
                env: Some([("STRANDS_LIVE".to_owned(), "yes".to_owned())].into()),
                ..ExecuteOptions::default()
            },
        )
        .await
        .expect("execute with env");
    assert_eq!(env.stdout.trim(), "yes");

    let cwd = sandbox
        .execute(
            "pwd",
            ExecuteOptions {
                cwd: Some("/".into()),
                ..ExecuteOptions::default()
            },
        )
        .await
        .expect("execute with cwd");
    assert_eq!(cwd.stdout.trim(), "/");

    let path = format!("/tmp/strands-live-{}.txt", std::process::id());
    sandbox
        .write_file(&path, b"live bytes")
        .await
        .expect("write_file");
    assert_eq!(sandbox.read_file(&path).await.expect("read_file"), b"live bytes");
    let listing = sandbox.list_files("/tmp").await.expect("list_files");
    assert!(listing.iter().any(|f| path.ends_with(&f.name)), "{listing:?}");
    sandbox.remove_file(&path).await.expect("remove_file");
    assert!(matches!(
        sandbox.read_file(&path).await.unwrap_err(),
        SandboxError::PathNotFound(_)
    ));

    let code = sandbox
        .execute_code("echo from sh", "sh", ExecuteOptions::default())
        .await
        .expect("execute_code");
    assert_eq!(code.stdout.trim(), "from sh");
}

#[tokio::test]
#[ignore = "live docker (STRANDS_TEST_DOCKER_CONTAINER)"]
async fn docker_runs_commands_and_file_operations_in_the_container() {
    let Some(sandbox) = docker() else { return };
    exercise(&sandbox).await;
}

#[tokio::test]
#[ignore = "live docker (STRANDS_TEST_DOCKER_CONTAINER)"]
async fn docker_reports_a_missing_container_as_a_failure() {
    let Some(_) = docker() else { return };
    let sandbox = DockerSandbox::new("strands-no-such-container");
    let result = sandbox
        .execute("echo hi", ExecuteOptions::default())
        .await
        .expect("execute");
    assert_ne!(result.exit_code, 0);
    assert!(!result.stderr.is_empty());
}

#[tokio::test]
#[ignore = "live ssh (STRANDS_TEST_SSH_HOST)"]
async fn ssh_runs_commands_and_file_operations_on_the_remote_host() {
    let Some(sandbox) = ssh() else { return };
    exercise(&sandbox).await;
}

#[tokio::test]
#[ignore = "live ssh (STRANDS_TEST_SSH_HOST)"]
async fn ssh_refuses_an_unknown_host_without_interactive_prompts() {
    let Some(_) = ssh() else { return };
    let sandbox = SshSandbox::new("strands-no-such-host.invalid", "/tmp");
    let result = sandbox
        .execute("echo hi", ExecuteOptions::default())
        .await
        .expect("execute");
    assert_ne!(result.exit_code, 0);
    assert!(!result.stderr.is_empty(), "BatchMode should fail, not prompt");
}
