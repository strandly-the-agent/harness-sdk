//! Shared helpers for the live harness tests (`tests/live_e2e.rs`, `tests/cross_sdk/main.rs`).
//! They mirror the TS integration helpers of `harness-ts/test/integration/harness.ts`
//! (`toolAttempted`, `toolSucceeded`, `toolResultContains`): assertions read the message trace
//! and side effects, not the wording.
#![allow(dead_code, reason = "each live test file uses a subset of the helpers")]

use std::path::{Path, PathBuf};

use aws_config::BehaviorVersion;
use aws_credential_types::provider::ProvideCredentials as _;
use serde_json::Value;
use strands::{Agent, AgentResult, InterruptResponse, StopReason};
use strands_harness::{
    BuiltinToolName, BuiltinToolsOption, Effort, HarnessAgentOptions, MemoryOption, SessionConfig,
    SessionOption, SkillsOption,
};

/// The small model the TS harness integration tests use (`INTEG_MODEL`), with thinking off as
/// there.
pub(crate) const BEDROCK_MODEL: (&str, Effort) = (
    "bedrock/global.anthropic.claude-haiku-4-5-20251001-v1:0",
    Effort::Off,
);

/// The TS integration tests' system prompt for a single-purpose turn (`TESTING`).
pub(crate) const TESTING: &str =
    "You are an automated integration test. Do exactly what is asked and nothing else.";

/// Whether the default AWS credential chain resolves; prints the skip message when it does not.
#[allow(clippy::print_stderr, reason = "the skip message")]
pub(crate) async fn aws_credentials_available() -> bool {
    let config = aws_config::defaults(BehaviorVersion::latest()).load().await;
    let resolved = match config.credentials_provider() {
        Some(provider) => provider.provide_credentials().await.is_ok(),
        None => false,
    };
    if !resolved {
        eprintln!("skipping: AWS credentials did not resolve");
    }
    resolved
}

/// The TS `buildAgent` defaults (no plugins) with skills and memory off too, so
/// nothing is read from or written to the process's working directory, and the session kept
/// under `<dir>/.agent/sessions`: the default `./.agent/sessions` of a process running in `dir`.
pub(crate) fn options(
    (model, effort): (&str, Effort),
    dir: &Path,
    session_id: &str,
    tools: &[BuiltinToolName],
) -> HarnessAgentOptions {
    HarnessAgentOptions {
        model: Some(model.into()),
        effort,
        builtin_tools: Some(BuiltinToolsOption::Names(tools.to_vec())),
        builtin_plugins: Some(Vec::new()),
        skills: SkillsOption::Off,
        memory: MemoryOption::Off,
        session: SessionOption::Config(SessionConfig {
            id: Some(session_id.to_owned()),
            dir: Some(session_dir(dir).to_string_lossy().into_owned()),
        }),
        ..Default::default()
    }
}

pub(crate) fn session_dir(dir: &Path) -> PathBuf {
    dir.join(".agent").join("sessions")
}

/// The agent's messages in their wire shape.
pub(crate) fn trace(agent: &Agent) -> Vec<Value> {
    agent
        .messages()
        .iter()
        .map(|message| serde_json::to_value(message).expect("a message serializes"))
        .collect()
}

fn blocks<'a>(trace: &'a [Value], key: &'a str) -> impl Iterator<Item = &'a Value> {
    trace
        .iter()
        .filter_map(|message| message["content"].as_array())
        .flatten()
        .filter_map(move |block| block.get(key))
}

/// Every `toolUse` block of the tool `name`.
pub(crate) fn tool_uses<'a>(trace: &'a [Value], name: &str) -> Vec<&'a Value> {
    blocks(trace, "toolUse")
        .filter(|tool_use| tool_use["name"] == name)
        .collect()
}

/// The `toolResult` blocks answering calls of the tool `name`.
pub(crate) fn results_of<'a>(trace: &'a [Value], name: &str) -> Vec<&'a Value> {
    let ids: Vec<&Value> = tool_uses(trace, name)
        .into_iter()
        .map(|tool_use| &tool_use["toolUseId"])
        .collect();
    blocks(trace, "toolResult")
        .filter(|result| ids.contains(&&result["toolUseId"]))
        .collect()
}

/// The model called the tool `name` (TS `toolAttempted`).
pub(crate) fn tool_attempted(trace: &[Value], name: &str) -> bool {
    !tool_uses(trace, name).is_empty()
}

/// A call of the tool `name` returned a `success` result (TS `toolSucceeded`).
pub(crate) fn tool_succeeded(trace: &[Value], name: &str) -> bool {
    results_of(trace, name)
        .iter()
        .any(|result| result["status"] == "success")
}

/// A result of the tool `name` contains `needle` (TS `toolResultContains`).
pub(crate) fn tool_result_contains(trace: &[Value], name: &str, needle: &str) -> bool {
    results_of(trace, name)
        .iter()
        .any(|result| result["content"].to_string().contains(needle))
}

/// Approves every interrupt until the agent stops for another reason; returns the final result
/// and the interrupts answered on the way.
pub(crate) async fn approve_until_done(agent: &mut Agent, mut result: AgentResult) -> (AgentResult, usize) {
    let mut answered = 0;
    for _ in 0..10 {
        if result.stop_reason() != StopReason::Interrupt {
            return (result, answered);
        }
        let responses: Vec<InterruptResponse> = result
            .interrupts()
            .iter()
            .map(|interrupt| InterruptResponse::new(interrupt.id(), Value::from("yes")))
            .collect();
        answered += responses.len();
        result = agent.invoke(responses).await.expect("the resume succeeds");
    }
    panic!("the agent kept interrupting after 10 resumes");
}
