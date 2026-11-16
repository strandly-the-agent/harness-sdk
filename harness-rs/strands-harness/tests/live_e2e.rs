//! Live end-to-end tests of `create_harness`, one per model provider, mirroring
//! harness-ts/test/integration/e2e.test.ts: a built-in tool call, an MCP tool, an interrupt
//! resumed through the result's interrupts, and a session resumed by a second `create_harness`.
//!
//! An integration test rather than a crate-internal module: with an absolute `session.dir` of
//! `<temp>/.agent/sessions` the public API gives the layout of the default `./.agent/sessions`,
//! so no crate-internal base directory is needed. Every path the model is given is absolute and
//! inside a temporary directory.
//!
//! Bedrock and Bedrock Mantle use the default AWS credential chain; Anthropic and OpenAI read
//! `ANTHROPIC_API_KEY` and `OPENAI_API_KEY`. The MCP fixture is
//! `harness-ts/test/tools/echo-mcp-server.mjs`: set `STRANDS_TEST_NODE` to the node binary and
//! `STRANDS_TEST_NODE_MODULES` to a `node_modules` with `@modelcontextprotocol/sdk` and `zod`.
//! A test whose credentials or node setup is missing skips with a message.
#![cfg(all(
    feature = "bedrock",
    feature = "anthropic",
    feature = "openai",
    feature = "mcp"
))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    reason = "test code"
)]

mod live_support;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use strands::StopReason;
use strands_harness::{BuiltinToolName, HarnessAgentOptions, McpServersOption, create_harness};

use live_support::{
    BEDROCK_MODEL, TESTING, approve_until_done, aws_credentials_available, options, session_dir,
    tool_attempted, tool_result_contains, tool_succeeded, tool_uses, trace,
};

/// Served in `us-east-1`, the region the TS integration fixtures use for Bedrock Mantle; the
/// harness takes the region from `AWS_REGION`.
const MANTLE_MODEL: &str = "bedrock-mantle/openai.gpt-oss-120b";
const ANTHROPIC_MODEL: &str = "anthropic/claude-haiku-4-5";
const OPENAI_MODEL: &str = "openai/gpt-5.4-mini";

const MARKER: &str = "MARMALADE-4271";

/// The `mcp_servers` entry for the echo fixture, copied next to a `node_modules` link in `dir`.
/// `None`, with the skip message, without the node setup.
fn echo_server(dir: &Path) -> Option<McpServersOption> {
    let (Ok(node), Ok(modules)) = (
        std::env::var("STRANDS_TEST_NODE"),
        std::env::var("STRANDS_TEST_NODE_MODULES"),
    ) else {
        eprintln!("skipping: STRANDS_TEST_NODE and STRANDS_TEST_NODE_MODULES are not set");
        return None;
    };
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../harness-ts/test/tools/echo-mcp-server.mjs");
    let server = dir.join("mcp").join("echo-mcp-server.mjs");
    std::fs::create_dir(dir.join("mcp")).expect("the fixture directory");
    std::fs::copy(fixture, &server).expect("copy the fixture");
    #[cfg(unix)]
    std::os::unix::fs::symlink(modules, dir.join("mcp").join("node_modules")).expect("link node_modules");
    #[cfg(not(unix))]
    {
        let _ = modules;
        eprintln!("skipping: the stdio fixture links node_modules, which these tests do on Unix only");
        return None;
    }
    let Value::Object(servers) = json!({ "fixture": { "command": node, "args": [server] } }) else {
        unreachable!("an object literal");
    };
    Some(McpServersOption::Servers(servers))
}

/// One provider's run: two gated tool calls (the built-in `write` and the MCP `fixture_echo`)
/// approved through interrupts, then a second agent on the same session id that reads the file
/// whose path only the stored conversation names.
async fn tools_interrupts_and_session_resume(model: &str) {
    let dir = tempfile::tempdir().expect("temp dir");
    let Some(mcp_servers) = echo_server(dir.path()) else {
        return;
    };
    let out = dir.path().join("out.txt");

    let mut first = create_harness(HarnessAgentOptions {
        mcp_servers: Some(mcp_servers),
        interventions: Some("ask".into()),
        ..options(model, dir.path(), "live-e2e", &[BuiltinToolName::Write])
    })
    .await
    .expect("the harness builds");
    let paused = first
        .invoke(format!(
            "{TESTING} Step 1: use the write tool to create the file {} containing exactly: {MARKER}. \
             Step 2: after that, use the fixture_echo tool with the text pong-77. \
             Call one tool at a time, then reply with the single word done.",
            out.display()
        ))
        .await
        .expect("the first turn runs");

    // `ask` gates every tool call: the turn pauses before `write` runs. The approval prompt
    // carries the call's input; the tool use joins the history once its result is in.
    assert_eq!(paused.stop_reason(), StopReason::Interrupt);
    let reason = paused.interrupts().first().expect("an interrupt").reason();
    let reason = reason.and_then(Value::as_str).expect("the approval prompt");
    assert!(reason.starts_with("Approve \"write\""), "{reason}");
    assert!(reason.contains(&*out.to_string_lossy()), "{reason}");
    assert!(!out.exists(), "write ran before it was approved");
    assert!(!tool_attempted(&trace(&first), "write"));

    let (done, answered) = approve_until_done(&mut first, paused).await;
    assert_eq!(done.stop_reason(), StopReason::EndTurn);
    assert!(
        answered >= 2,
        "{answered} interrupts answered, expected one per tool call"
    );
    let usage = done.metrics().accumulated_usage();
    assert!(usage.input_tokens() > 0 && usage.output_tokens() > 0);
    let messages = trace(&first);
    assert!(tool_succeeded(&messages, "write"));
    assert_eq!(
        tool_uses(&messages, "write")[0]["input"]["path"],
        json!(out.to_string_lossy())
    );
    assert!(
        std::fs::read_to_string(&out)
            .expect("the written file")
            .contains(MARKER)
    );
    assert!(tool_succeeded(&messages, "fixture_echo"));
    assert_eq!(
        tool_uses(&messages, "fixture_echo")[0]["input"]["text"],
        "pong-77"
    );
    assert!(tool_result_contains(&messages, "fixture_echo", "echo:pong-77"));
    assert!(
        session_dir(dir.path())
            .join("live-e2e")
            .read_dir()
            .is_ok_and(|mut entries| entries.next().is_some()),
        "no session folder under {:?}",
        session_dir(dir.path())
    );
    first.shutdown().await.expect("the first agent shuts down");

    let mut second = create_harness(options(model, dir.path(), "live-e2e", &[BuiltinToolName::Read]))
        .await
        .expect("the second harness builds");
    second.initialize().await.expect("the session restores");
    assert_eq!(trace(&second), messages, "the restored conversation");
    let resumed = second
        .invoke(format!(
            "{TESTING} Use the read tool to read the file you created earlier in this conversation, \
             then reply with the single word done."
        ))
        .await
        .expect("the resumed turn runs");
    assert_eq!(resumed.stop_reason(), StopReason::EndTurn);
    let resumed_messages = trace(&second);
    assert!(resumed_messages.len() > messages.len());
    assert_eq!(resumed_messages[..messages.len()], messages[..]);
    assert_eq!(
        tool_uses(&resumed_messages, "read")[0]["input"]["path"],
        json!(out.to_string_lossy())
    );
    assert!(tool_result_contains(&resumed_messages, "read", MARKER));
    second.shutdown().await.expect("the second agent shuts down");
}

fn key_set(name: &str) -> bool {
    let set = std::env::var(name).is_ok_and(|value| !value.is_empty());
    if !set {
        eprintln!("skipping: {name} is not set");
    }
    set
}

#[tokio::test]
#[ignore = "live bedrock"]
async fn live_bedrock_tool_call_mcp_tool_interrupt_resume_and_session_resume() {
    if aws_credentials_available().await {
        tools_interrupts_and_session_resume(BEDROCK_MODEL).await;
    }
}

#[tokio::test]
#[ignore = "live bedrock-mantle"]
async fn live_bedrock_mantle_tool_call_mcp_tool_interrupt_resume_and_session_resume() {
    if aws_credentials_available().await {
        tools_interrupts_and_session_resume(MANTLE_MODEL).await;
    }
}

#[tokio::test]
#[ignore = "live anthropic"]
async fn live_anthropic_tool_call_mcp_tool_interrupt_resume_and_session_resume() {
    if key_set("ANTHROPIC_API_KEY") {
        tools_interrupts_and_session_resume(ANTHROPIC_MODEL).await;
    }
}

#[tokio::test]
#[ignore = "live openai"]
async fn live_openai_tool_call_mcp_tool_interrupt_resume_and_session_resume() {
    if key_set("OPENAI_API_KEY") {
        tools_interrupts_and_session_resume(OPENAI_MODEL).await;
    }
}
