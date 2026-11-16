//! Shared helpers for the live Bedrock tests (`tests/live_*.rs`). They mirror the TS integration
//! helpers (`strands-ts/test/integ/__fixtures__/test-helpers.ts` and the harness `toolAttempted`,
//! `toolSucceeded` and `toolResultContains`): assertions read the message trace, not the wording.
#![allow(dead_code, reason = "each live test file uses a subset of the helpers")]

use aws_config::BehaviorVersion;
use aws_credential_types::provider::ProvideCredentials as _;
use serde_json::Value;
use strands::models::bedrock::{BedrockModel, BedrockModelConfig, BedrockModelOptions};
use strands::{Agent, AgentResult, InterruptResponse, StopReason};

/// A small model the TS integration tests use.
pub(crate) const MODEL_ID: &str = "us.anthropic.claude-haiku-4-5-20251001-v1:0";
pub(crate) const REGION: &str = "us-west-2";

/// Whether the default AWS credential chain resolves; prints the skip message when it does not.
pub(crate) async fn credentials_available() -> bool {
    let config = aws_config::defaults(BehaviorVersion::latest()).load().await;
    let Some(provider) = config.credentials_provider() else {
        eprintln!("skipping: no AWS credentials provider");
        return false;
    };
    if provider.provide_credentials().await.is_err() {
        eprintln!("skipping: AWS credentials did not resolve");
        return false;
    }
    true
}

pub(crate) fn bedrock_with(model_id: &str, max_tokens: u64) -> BedrockModel {
    BedrockModel::new(BedrockModelOptions {
        config: BedrockModelConfig {
            model_id: Some(model_id.into()),
            max_tokens: Some(max_tokens),
            ..Default::default()
        },
        region: Some(REGION.into()),
        ..Default::default()
    })
}

pub(crate) fn bedrock(max_tokens: u64) -> BedrockModel {
    bedrock_with(MODEL_ID, max_tokens)
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

/// Every `toolUse` block of the trace.
pub(crate) fn tool_uses(trace: &[Value]) -> Vec<&Value> {
    blocks(trace, "toolUse").collect()
}

/// Every `toolResult` block of the trace.
pub(crate) fn tool_results(trace: &[Value]) -> Vec<&Value> {
    blocks(trace, "toolResult").collect()
}

/// The `toolResult` blocks answering calls of the tool `name`.
pub(crate) fn results_of<'a>(trace: &'a [Value], name: &str) -> Vec<&'a Value> {
    let ids: Vec<&Value> = tool_uses(trace)
        .into_iter()
        .filter(|tool_use| tool_use["name"] == name)
        .map(|tool_use| &tool_use["toolUseId"])
        .collect();
    tool_results(trace)
        .into_iter()
        .filter(|result| ids.contains(&&result["toolUseId"]))
        .collect()
}

/// The model called the tool `name` (TS `toolAttempted`).
pub(crate) fn tool_attempted(trace: &[Value], name: &str) -> bool {
    tool_uses(trace).iter().any(|tool_use| tool_use["name"] == name)
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

/// The text blocks of the last assistant message, joined and lowercased.
pub(crate) fn last_assistant_text(trace: &[Value]) -> String {
    trace
        .iter()
        .rev()
        .find(|message| message["role"] == "assistant")
        .and_then(|message| message["content"].as_array())
        .map(|content| {
            content
                .iter()
                .filter_map(|block| block["text"].as_str())
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        })
        .unwrap_or_default()
}

/// Answers every interrupt with `respond` until the agent stops for another reason (TS
/// `resumeUntilDone`).
pub(crate) async fn resume_until_done(
    agent: &mut Agent,
    mut result: AgentResult,
    respond: impl Fn(&strands::interrupt::Interrupt) -> Value,
) -> AgentResult {
    for _ in 0..10 {
        if result.stop_reason() != StopReason::Interrupt {
            return result;
        }
        let responses: Vec<InterruptResponse> = result
            .interrupts()
            .iter()
            .map(|interrupt| InterruptResponse::new(interrupt.id(), respond(interrupt)))
            .collect();
        result = agent.invoke(responses).await.expect("the resume succeeds");
    }
    panic!("the agent kept interrupting after 10 resumes");
}
