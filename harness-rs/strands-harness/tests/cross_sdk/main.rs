//! The cross-SDK session check: a session written by the TypeScript harness (`createHarness`,
//! run by `session.mts` in this directory) is resumed by `create_harness` on the same `./.agent`
//! directory, and the reverse. Each direction checks that the resumed history equals what the
//! other SDK wrote, and that a further turn appends to it and is readable by the first SDK. Two
//! more tests leave an interrupt pending in one SDK and approve it from the other.
//!
//! Live against Bedrock (the default AWS credential chain). Set `STRANDS_TEST_NODE` to the node
//! binary and `STRANDS_CROSS_SDK_HARNESS_TS` to a harness-ts checkout with its dependencies
//! installed (`tsx` among them). Without them, or without credentials, the tests skip with a
//! message. Everything runs in a temporary directory, the TypeScript side with it as its working
//! directory.
#![cfg(feature = "bedrock")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stderr,
    reason = "test code"
)]

#[path = "../live_support/mod.rs"]
mod live_support;

use std::path::{Path, PathBuf};

use serde_json::Value;
use strands::{Agent, InterruptResponse, StopReason};
use strands_harness::{BuiltinToolName, HarnessAgentOptions, create_harness};

use live_support::{
    BEDROCK_MODEL, TESTING, aws_credentials_available, options, tool_result_contains, tool_succeeded, trace,
};

const MARKER: &str = "QUINCE-9038";
const READ_BACK: &str = "Use the read tool to read the file you created earlier in this conversation, \
                         then reply with the single word done.";

/// Runs `session.mts` in `dir`.
struct TypeScript {
    node: String,
    harness_ts: PathBuf,
}

impl TypeScript {
    /// `None`, with the skip message, without the node setup or AWS credentials.
    async fn from_env() -> Option<Self> {
        let (Ok(node), Ok(harness_ts)) = (
            std::env::var("STRANDS_TEST_NODE"),
            std::env::var("STRANDS_CROSS_SDK_HARNESS_TS"),
        ) else {
            eprintln!("skipping: STRANDS_TEST_NODE and STRANDS_CROSS_SDK_HARNESS_TS are not set");
            return None;
        };
        aws_credentials_available().await.then(|| Self {
            node,
            harness_ts: harness_ts.into(),
        })
    }

    async fn run(&self, dir: &Path, session_id: &str, flags: &[(&str, &str)]) {
        let status = tokio::process::Command::new(&self.node)
            .arg(self.harness_ts.join("node_modules/tsx/dist/cli.mjs"))
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cross_sdk/session.mts"))
            .arg(session_id)
            .args(flags.iter().flat_map(|(flag, value)| [flag, value]))
            .env("STRANDS_CROSS_SDK_HARNESS_TS", &self.harness_ts)
            .current_dir(dir)
            .status()
            .await
            .expect("node starts");
        assert!(status.success(), "session.mts {flags:?} ended with {status}");
    }
}

/// The Rust agent on the session, restored.
async fn rust_agent(dir: &Path, session_id: &str, interventions: Option<&str>) -> Agent {
    let tools = [BuiltinToolName::Write, BuiltinToolName::Read];
    let mut agent = create_harness(HarnessAgentOptions {
        interventions: interventions.map(Into::into),
        ..options(BEDROCK_MODEL, dir, session_id, &tools)
    })
    .await
    .expect("the harness builds");
    agent.initialize().await.expect("the session restores");
    agent
}

/// One Rust run on the session: restores it, checks the restored history against `expect`, runs
/// `prompt` as a further turn, and returns the final history.
async fn rust_run(
    dir: &Path,
    session_id: &str,
    expect: Option<&[Value]>,
    prompt: Option<&str>,
) -> Vec<Value> {
    let mut agent = rust_agent(dir, session_id, None).await;
    let restored = trace(&agent);
    if let Some(expect) = expect {
        assert_eq!(
            restored, expect,
            "the restored history is not what the other SDK wrote"
        );
    }
    if let Some(prompt) = prompt {
        let result = agent.invoke(prompt).await.expect("the turn runs");
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(trace(&agent)[..restored.len()], restored[..]);
    }
    let history = trace(&agent);
    agent.shutdown().await.expect("the agent shuts down");
    history
}

fn write_prompt(dir: &Path) -> String {
    format!(
        "{TESTING} Use the write tool to create the file {} containing exactly: {MARKER}. \
         Then reply with the single word done.",
        dir.join("out.txt").display()
    )
}

fn read_history(path: &Path) -> Vec<Value> {
    serde_json::from_slice(&std::fs::read(path).expect("the dumped history"))
        .expect("a JSON array of messages")
}

fn dump_history(path: &Path, history: &[Value]) {
    std::fs::write(path, serde_json::to_vec(history).expect("the history serializes")).expect("the dump");
}

/// The second turn read the file named only in the first SDK's part of the history.
fn assert_continued(first: &[Value], second: &[Value]) {
    assert!(tool_succeeded(first, "write"));
    assert!(second.len() > first.len());
    assert_eq!(second[..first.len()], first[..]);
    assert!(tool_result_contains(&second[first.len()..], "read", MARKER));
}

#[tokio::test]
#[ignore = "live bedrock"]
async fn live_cross_sdk_rust_resumes_a_session_written_by_typescript() {
    let Some(typescript) = TypeScript::from_env().await else {
        return;
    };
    let dir = tempfile::tempdir().expect("temp dir");
    let (ts_dump, rust_dump) = (dir.path().join("ts-1.json"), dir.path().join("rust-2.json"));

    let flags = [
        ("--prompt", &*write_prompt(dir.path())),
        ("--dump", &*ts_dump.to_string_lossy()),
    ];
    typescript.run(dir.path(), "cross-ts", &flags).await;
    let written = read_history(&ts_dump);

    let prompt = format!("{TESTING} {READ_BACK}");
    let continued = rust_run(dir.path(), "cross-ts", Some(&written), Some(&prompt)).await;
    assert_continued(&written, &continued);

    dump_history(&rust_dump, &continued);
    let flags = [("--expect", &*rust_dump.to_string_lossy())];
    typescript.run(dir.path(), "cross-ts", &flags).await;
}

#[tokio::test]
#[ignore = "live bedrock"]
async fn live_cross_sdk_typescript_resumes_a_session_written_by_rust() {
    let Some(typescript) = TypeScript::from_env().await else {
        return;
    };
    let dir = tempfile::tempdir().expect("temp dir");
    let (rust_dump, ts_dump) = (dir.path().join("rust-1.json"), dir.path().join("ts-2.json"));

    let written = rust_run(dir.path(), "cross-rs", None, Some(&write_prompt(dir.path()))).await;
    dump_history(&rust_dump, &written);

    let prompt = format!("{TESTING} {READ_BACK}");
    let flags = [
        ("--expect", &*rust_dump.to_string_lossy()),
        ("--prompt", &*prompt),
        ("--dump", &*ts_dump.to_string_lossy()),
    ];
    typescript.run(dir.path(), "cross-rs", &flags).await;
    let continued = read_history(&ts_dump);
    assert_continued(&written, &continued);

    rust_run(dir.path(), "cross-rs", Some(&continued), None).await;
}

/// The write the approval released ran once, in the SDK that resumed.
fn assert_approved(dir: &Path, history: &[Value]) {
    assert!(tool_succeeded(history, "write"));
    let written = std::fs::read_to_string(dir.join("out.txt")).expect("the written file");
    assert!(written.contains(MARKER));
}

#[tokio::test]
#[ignore = "live bedrock"]
async fn live_cross_sdk_rust_approves_an_interrupt_pending_in_a_typescript_session() {
    let Some(typescript) = TypeScript::from_env().await else {
        return;
    };
    let dir = tempfile::tempdir().expect("temp dir");
    let (ids, ts_dump, rust_dump) = (
        dir.path().join("interrupts.json"),
        dir.path().join("ts-1.json"),
        dir.path().join("rust-2.json"),
    );

    let flags = [
        ("--interventions", "ask"),
        ("--prompt", &*write_prompt(dir.path())),
        ("--pause", &*ids.to_string_lossy()),
        ("--dump", &*ts_dump.to_string_lossy()),
    ];
    typescript.run(dir.path(), "pending-ts", &flags).await;
    assert!(
        !dir.path().join("out.txt").exists(),
        "write ran before it was approved"
    );
    let ids: Vec<String> =
        serde_json::from_slice(&std::fs::read(&ids).expect("the interrupt ids")).expect("ids");
    assert!(!ids.is_empty());

    let mut agent = rust_agent(dir.path(), "pending-ts", Some("ask")).await;
    assert_eq!(trace(&agent), read_history(&ts_dump));
    let responses: Vec<InterruptResponse> = ids
        .iter()
        .map(|id| InterruptResponse::new(id, Value::from("yes")))
        .collect();
    let result = agent.invoke(responses).await.expect("the resume runs");
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    let history = trace(&agent);
    agent.shutdown().await.expect("the agent shuts down");
    assert_approved(dir.path(), &history);

    dump_history(&rust_dump, &history);
    let flags = [
        ("--interventions", "ask"),
        ("--expect", &*rust_dump.to_string_lossy()),
    ];
    typescript.run(dir.path(), "pending-ts", &flags).await;
}

#[tokio::test]
#[ignore = "live bedrock"]
async fn live_cross_sdk_typescript_approves_an_interrupt_pending_in_a_rust_session() {
    let Some(typescript) = TypeScript::from_env().await else {
        return;
    };
    let dir = tempfile::tempdir().expect("temp dir");
    let (ids, rust_dump, ts_dump) = (
        dir.path().join("interrupts.json"),
        dir.path().join("rust-1.json"),
        dir.path().join("ts-2.json"),
    );

    let mut agent = rust_agent(dir.path(), "pending-rs", Some("ask")).await;
    let paused = agent
        .invoke(write_prompt(dir.path()))
        .await
        .expect("the turn runs");
    assert_eq!(paused.stop_reason(), StopReason::Interrupt);
    let pending: Vec<&str> = paused
        .interrupts()
        .iter()
        .map(strands::interrupt::Interrupt::id)
        .collect();
    std::fs::write(&ids, serde_json::to_vec(&pending).expect("the ids serialize"))
        .expect("the interrupt ids");
    dump_history(&rust_dump, &trace(&agent));
    agent.shutdown().await.expect("the agent shuts down");
    assert!(
        !dir.path().join("out.txt").exists(),
        "write ran before it was approved"
    );

    let flags = [
        ("--interventions", "ask"),
        ("--expect", &*rust_dump.to_string_lossy()),
        ("--approve", &*ids.to_string_lossy()),
        ("--dump", &*ts_dump.to_string_lossy()),
    ];
    typescript.run(dir.path(), "pending-rs", &flags).await;
    let history = read_history(&ts_dump);
    assert_approved(dir.path(), &history);

    let agent = rust_agent(dir.path(), "pending-rs", Some("ask")).await;
    assert_eq!(trace(&agent), history);
}
