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
    clippy::indexing_slicing,
    clippy::print_stderr,
    reason = "test code"
)]

mod live_support;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use strands::StopReason;
use strands::testing::tool_context;
use strands::types::tools::ToolUse;
use strands_harness::{
    BuiltinPluginName, BuiltinToolName, Effort, HarnessAgentOptions, McpServersOption, SessionConfig,
    SessionOption, create_harness, exa_web_search,
};

use live_support::{
    BEDROCK_MODEL, TESTING, answer_until_done, aws_credentials_available, options, session_dir,
    tool_attempted, tool_result_contains, tool_succeeded, tool_uses, trace,
};

/// In the region of `AWS_REGION`. Effort is `low` rather than `off`: `off` is sent as
/// `reasoning.effort: "none"` on Bedrock Mantle, as in TS, and this model answers that with
/// `server_error`.
const MANTLE_MODEL: (&str, Effort) = ("bedrock-mantle/openai.gpt-oss-120b", Effort::Low);
const ANTHROPIC_MODEL: (&str, Effort) = ("anthropic/claude-haiku-4-5", Effort::Off);
const OPENAI_MODEL: (&str, Effort) = ("openai/gpt-5.4-mini", Effort::Off);

const MARKER: &str = "MARMALADE-4271";

/// The TS multi-step test's system prompt (`TEST_INSTRUCTIONS`): no step may be skipped, merged
/// or substituted.
const TEST_INSTRUCTIONS: &str = "You are running inside an automated integration test whose only purpose is to \
    confirm that every tool works. You MUST complete every numbered step, one at a time and in order. You may NOT \
    skip a step, combine or batch steps into parallel tool calls, or substitute one tool for another (do not use \
    one tool to do another step's job). For each step, call the exact tool it names, wait for the result, then \
    move on. Do nothing that isn't asked.";

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
        panic!("an object literal");
    };
    Some(McpServersOption::Servers(servers))
}

/// One provider's run: two gated tool calls (the built-in `write` and the MCP `fixture_echo`)
/// approved through interrupts, then a second agent on the same session id that reads the file
/// whose path only the stored conversation names.
async fn tools_interrupts_and_session_resume(model: (&str, Effort)) {
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

    let (done, answered) = answer_until_done(&mut first, paused, json!("yes")).await;
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

/// The TS `buildAgent` on Bedrock: no session, and these built-in tools only.
fn bedrock(dir: &Path, tools: &[BuiltinToolName]) -> HarnessAgentOptions {
    HarnessAgentOptions {
        session: SessionOption::Off,
        ..options(BEDROCK_MODEL, dir, "unused", tools)
    }
}

/// TS case `uses the shell tool`. The command changes into the temporary directory itself.
#[tokio::test]
#[ignore = "live bedrock"]
async fn live_bedrock_uses_the_shell_tool() {
    if !aws_credentials_available().await {
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let mut agent = create_harness(bedrock(dir.path(), &[BuiltinToolName::Shell]))
        .await
        .expect("the harness builds");
    let result = agent
        .invoke(format!(
            "{TESTING} Use the shell tool to run exactly this command and report its output: \
             `cd {} && echo shell-5512 > shell.txt && cat shell.txt`",
            dir.path().display()
        ))
        .await
        .expect("the turn runs");
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    let messages = trace(&agent);
    assert!(tool_succeeded(&messages, "shell"));
    assert!(tool_result_contains(&messages, "shell", "shell-5512"));
    let written = std::fs::read_to_string(dir.path().join("shell.txt")).expect("the file the command wrote");
    assert_eq!(written.trim(), "shell-5512");
    agent.shutdown().await.expect("the agent shuts down");
}

/// TS cases `uses the file/web tools, the todos plugin, and the subagent in one turn` and `uses
/// the todo_write tool from the todos plugin`. The environment plugin stays off: it reads the
/// process's working directory.
#[tokio::test]
#[ignore = "live bedrock"]
async fn live_bedrock_uses_the_file_and_web_tools_the_todos_plugin_and_the_subagent_in_one_turn() {
    if !aws_credentials_available().await {
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let (data, output, edited) = (
        dir.path().join("data.txt"),
        dir.path().join("out.txt"),
        dir.path().join("edit.txt"),
    );
    std::fs::write(&data, "The secret code is 4271.\n").expect("the data file");
    std::fs::write(&edited, "alpha\n").expect("the file to edit");
    // Shell is left out so the file tools are the only way to do the read, write and edit steps.
    let tools = [
        BuiltinToolName::Read,
        BuiltinToolName::Write,
        BuiltinToolName::Edit,
        BuiltinToolName::WebFetch,
        BuiltinToolName::Subagent,
    ];
    let mut agent = create_harness(HarnessAgentOptions {
        builtin_plugins: Some(vec![BuiltinPluginName::Todos]),
        instructions: Some(TEST_INSTRUCTIONS.to_owned()),
        ..bedrock(dir.path(), &tools)
    })
    .await
    .expect("the harness builds");
    // The every-turn todo injection and the background delivery run together.
    let todo = json!([{
        "content": "Track the delegated calculation",
        "activeForm": "Tracking the delegated calculation",
        "status": "in_progress",
    }]);
    agent.app_state().set("todos", todo).expect("the todo list");
    let result = agent
        .invoke(format!(
            "This is an automated integration test. There are 6 steps below. You MUST perform every single \
             one, in order, each with its own separate tool call — do not skip any, do not do them in \
             parallel, do not use a different tool than the one named. After all 6, stop.\n\
             Step 1 — call the `read` tool on {data}, then tell me the secret code it contains.\n\
             Step 2 — call the `write` tool to create {output} whose entire contents are that secret code.\n\
             Step 3 — call the `edit` tool to replace alpha with omega in {edited}.\n\
             Step 4 — call the `web_fetch` tool on https://example.com and report its title.\n\
             Step 5 — call the `todo_write` tool to record a two-item todo list.\n\
             Step 6 — call the `subagent` tool, asking it to compute 111 + 222, and report the result.\n\
             That is all 6 steps. Each of the 6 tools must be called exactly once.",
            data = data.display(),
            output = output.display(),
            edited = edited.display(),
        ))
        .await
        .expect("the turn runs");
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    let messages = trace(&agent);
    for name in ["read", "write", "edit", "web_fetch", "todo_write", "subagent"] {
        assert!(
            tool_succeeded(&messages, name),
            "{name} was not used successfully"
        );
    }
    // Each tool's effect: read returned the file's contents, write and edit landed on disk, and
    // the subagent's answer, delivered from the background, reached the conversation.
    assert!(tool_result_contains(&messages, "read", "4271"));
    assert!(
        std::fs::read_to_string(&output)
            .expect("the written file")
            .contains("4271")
    );
    assert!(
        std::fs::read_to_string(&edited)
            .expect("the edited file")
            .contains("omega")
    );
    assert!(json!(messages).to_string().contains("333"));
    agent.shutdown().await.expect("the agent shuts down");
}

/// harness-ts/test/integration/caching.test.ts, `reads grow across turns`: with the default
/// caching, the cumulative cache reads the model reports are non-zero and grow on every turn.
/// The environment plugin stays off: it reads the process's working directory.
#[tokio::test]
#[ignore = "live bedrock"]
async fn live_bedrock_cache_reads_grow_across_turns() {
    if !aws_credentials_available().await {
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    // Stable and long enough to pass the model's minimum cacheable prefix.
    let stable_prefix = format!(
        "{}\n",
        "You are an automated integration test agent operating under a fixed protocol. ".repeat(8)
    )
    .repeat(40);
    let tools = [
        BuiltinToolName::Shell,
        BuiltinToolName::Read,
        BuiltinToolName::Write,
        BuiltinToolName::Edit,
        BuiltinToolName::WebFetch,
        BuiltinToolName::ProgrammaticToolCaller,
    ];
    let mut agent = create_harness(HarnessAgentOptions {
        instructions: Some(stable_prefix),
        builtin_plugins: Some(vec![BuiltinPluginName::Todos]),
        ..bedrock(dir.path(), &tools)
    })
    .await
    .expect("the harness builds");
    let todo = json!([{
        "content": "Answer each question",
        "activeForm": "Answering each question",
        "status": "in_progress",
    }]);
    agent.app_state().set("todos", todo).expect("the todo list");

    let mut reads = Vec::new();
    for word in ["apple", "banana", "cherry"] {
        agent
            .invoke(format!(
                "{TESTING} Reply with exactly the word: {word}. Nothing else."
            ))
            .await
            .expect("the turn runs");
        let usage = agent.metrics();
        reads.push(usage.accumulated_usage().cache_read_input_tokens().unwrap_or(0));
    }
    assert!(reads[2] > 0, "the prompt cache was never read back: {reads:?}");
    assert!(
        reads.windows(2).all(|pair| pair[1] > pair[0]),
        "cumulative cache reads did not grow every turn: {reads:?}"
    );
    agent.shutdown().await.expect("the agent shuts down");
}

/// TS case `blocks a denied tool call via interventions`, with the denial given as the answer
/// to the interrupt.
#[tokio::test]
#[ignore = "live bedrock"]
async fn live_bedrock_blocks_a_denied_tool_call_via_interventions() {
    if !aws_credentials_available().await {
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let blocked = dir.path().join("blocked.txt");
    let mut agent = create_harness(HarnessAgentOptions {
        interventions: Some("ask".into()),
        ..bedrock(dir.path(), &[BuiltinToolName::Write])
    })
    .await
    .expect("the harness builds");
    let paused = agent
        .invoke(format!(
            "{TESTING} Use the write tool to create the file {} containing: nope.",
            blocked.display()
        ))
        .await
        .expect("the turn runs");
    assert_eq!(paused.stop_reason(), StopReason::Interrupt);
    let (done, answered) = answer_until_done(&mut agent, paused, json!(false)).await;
    assert_eq!(done.stop_reason(), StopReason::EndTurn);
    assert!(answered >= 1);
    // The write was attempted (the gate ran, the model did not skip it) but denied.
    let messages = trace(&agent);
    assert!(tool_attempted(&messages, "write"));
    assert!(!tool_succeeded(&messages, "write"));
    assert!(tool_result_contains(
        &messages,
        "write",
        "CONFIRMATION_FAILED: Approve"
    ));
    assert!(!blocked.exists());
    agent.shutdown().await.expect("the agent shuts down");
}

/// TS case `persists a session with a minted id`.
#[tokio::test]
#[ignore = "live bedrock"]
async fn live_bedrock_persists_a_session_with_a_minted_id() {
    if !aws_credentials_available().await {
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let sessions = dir.path().join("sessions");
    let mut agent = create_harness(HarnessAgentOptions {
        session: SessionOption::Config(SessionConfig {
            id: None,
            dir: Some(sessions.to_string_lossy().into_owned()),
        }),
        ..bedrock(dir.path(), &[])
    })
    .await
    .expect("the harness builds");
    agent
        .invoke(format!("{TESTING} Say hi in one word."))
        .await
        .expect("the turn runs");
    agent.shutdown().await.expect("the agent shuts down");

    // Each session is a top-level folder named by its id, with the snapshot further down.
    let minted: Vec<PathBuf> = sessions
        .read_dir()
        .expect("the session dir")
        .map(|entry| entry.expect("an entry").path())
        .collect();
    assert_eq!(minted.len(), 1, "{minted:?}");
    let id = minted[0].file_name().expect("a folder name").to_string_lossy();
    assert!(
        id.len() == 8 && id.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
        "{id}"
    );
    assert!(holds_file(&minted[0], "snapshot_latest.json"));
}

fn holds_file(dir: &Path, name: &str) -> bool {
    dir.read_dir().is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            let path = entry.path();
            if path.is_dir() {
                holds_file(&path, name)
            } else {
                entry.file_name() == name
            }
        })
    })
}

/// harness-ts/test/integration/web-search.test.ts, `returns parsed results`: the default
/// `web_search` backend against Exa's hosted MCP server, through the public tool. Keyless;
/// `EXA_API_KEY` is picked up when present.
#[tokio::test]
#[ignore = "live exa"]
async fn live_exa_web_search_returns_parsed_results() {
    let input = json!({ "query": "Strands Agents SDK documentation", "max_results": 3 });
    let context = tool_context(ToolUse::new("web_search", "tool-1", input.clone()));
    let output = exa_web_search()
        .invoke(input, context)
        .await
        .expect("the tool runs");
    let text = output.as_str().expect("a text result");
    if text.contains("rate limit") {
        eprintln!("skipping: Exa's keyless rate limit was hit");
        return;
    }
    let sources: Vec<&str> = text
        .lines()
        .filter(|line| line.starts_with(|c: char| c.is_ascii_digit()))
        .collect();
    assert!(text.starts_with("Sources:\n1. ") && !sources.is_empty(), "{text}");
    for source in sources {
        let (title, url) = source.rsplit_once(" — ").expect("a title and a URL");
        assert!(
            title.split_once(". ").is_some_and(|(_, title)| !title.is_empty()),
            "{source}"
        );
        assert!(url.starts_with("http"), "{source}");
    }
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
