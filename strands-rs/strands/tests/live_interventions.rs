//! Live intervention tests against Bedrock (TS checklist:
//! strands-ts/test/integ/interventions.test.ts and strands-ts/test/integ/cedar.test.node.ts).
//! The TS cases are grouped into a few agent runs; each test's doc comment names the TS cases
//! it covers. They use the default AWS credential chain and skip with a message when no
//! credentials resolve. Run with `cargo test -p strands --all-features -- --ignored live_`.
#![cfg(feature = "bedrock")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test code"
)]

mod live_support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::StreamExt as _;
use live_support::{
    bedrock, credentials_available, results_of, resume_until_done, tool_attempted, tool_result_contains,
    tool_results, tool_succeeded, trace,
};
use serde_json::{Value, json};
use strands::hooks::{
    AfterModelCallEvent, AfterToolCallEvent, BeforeInvocationEvent, BeforeModelCallEvent, BeforeToolCallEvent,
};
use strands::interventions::{
    AfterModelCallAction, AfterToolCallAction, BeforeInvocationAction, BeforeModelCallAction,
    BeforeToolCallAction, InterventionHandler, OnError, confirm, deny, guide, proceed, transform,
};
use strands::types::messages::ToolResultBlock;
use strands::types::tools::{ToolResultStatus, ToolUse};
use strands::vended_interventions::{CedarAuthorization, CedarAuthorizationConfig, CedarEntities, TypeAndId};
use strands::{Agent, InvocationState, InvokeOptions, StopReason, ToolError, tool};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/cedar");

/// Returns the current time.
#[tool]
async fn time_tool() -> Result<String, ToolError> {
    Ok("12:00".into())
}

/// Returns the current weather.
#[tool]
async fn weather_tool() -> Result<String, ToolError> {
    Ok("sunny".into())
}

/// Echoes the message back.
#[tool]
async fn echo_tool(
    /// The message to echo.
    message: String,
) -> Result<String, ToolError> {
    Ok(format!("Echo: {message}"))
}

/// Search for information. Always use this tool when asked to search.
#[tool]
async fn search(
    /// Search query
    query: String,
) -> Result<String, ToolError> {
    Ok(format!("Results for: {query}"))
}

/// Delete a database record by ID. Always use this tool when asked to delete.
#[tool]
async fn delete_record(
    /// Record ID to delete
    record_id: String,
) -> Result<String, ToolError> {
    Ok(format!("Deleted record {record_id}"))
}

type ToolPolicyFn = dyn Fn(&mut BeforeToolCallEvent) -> strands::Result<BeforeToolCallAction> + Send + Sync;

/// A handler deciding each tool call with a closure.
struct ToolPolicy {
    name: &'static str,
    on_error: OnError,
    decide: Box<ToolPolicyFn>,
}

impl ToolPolicy {
    fn new(
        name: &'static str,
        decide: impl Fn(&mut BeforeToolCallEvent) -> strands::Result<BeforeToolCallAction> + Send + Sync + 'static,
    ) -> Self {
        Self {
            name,
            on_error: OnError::Throw,
            decide: Box::new(decide),
        }
    }

    fn on_error(mut self, on_error: OnError) -> Self {
        self.on_error = on_error;
        self
    }
}

#[strands::async_trait]
impl InterventionHandler for ToolPolicy {
    fn name(&self) -> &str {
        self.name
    }

    fn on_error(&self) -> OnError {
        self.on_error
    }

    async fn before_tool_call(
        &self,
        event: &mut BeforeToolCallEvent,
    ) -> strands::Result<BeforeToolCallAction> {
        (self.decide)(event)
    }
}

/// Denies at one lifecycle point (TS `DenyInvocationHandler` and `DenyBeforeModelHandler`).
struct DenyAt {
    invocation: bool,
}

#[strands::async_trait]
impl InterventionHandler for DenyAt {
    fn name(&self) -> &str {
        "deny-at"
    }

    async fn before_invocation(
        &self,
        _event: &mut BeforeInvocationEvent<'_>,
    ) -> strands::Result<BeforeInvocationAction> {
        if self.invocation {
            return Ok(deny("Invocation blocked by policy").into());
        }
        Ok(proceed().into())
    }

    async fn before_model_call(
        &self,
        _event: &mut BeforeModelCallEvent<'_>,
    ) -> strands::Result<BeforeModelCallAction> {
        Ok(deny("Model call blocked by intervention").into())
    }
}

/// Guides the first model call and the first model answer, and redacts digits in tool results
/// (TS `GuideAfterModelHandler`, the before-model guide and `TransformToolResultHandler`).
struct GuideAndRedact {
    before_model: AtomicUsize,
    after_model: AtomicUsize,
}

#[strands::async_trait]
impl InterventionHandler for GuideAndRedact {
    fn name(&self) -> &str {
        "guide-and-redact"
    }

    async fn before_model_call(
        &self,
        _event: &mut BeforeModelCallEvent<'_>,
    ) -> strands::Result<BeforeModelCallAction> {
        if self.before_model.fetch_add(1, Ordering::SeqCst) == 0 {
            return Ok(guide("Always answer in one short sentence").into());
        }
        Ok(proceed().into())
    }

    async fn after_model_call(
        &self,
        event: &mut AfterModelCallEvent<'_>,
    ) -> strands::Result<AfterModelCallAction> {
        // Guide the first answer that is not a tool call, so the model retries once.
        let end_turn = event
            .stop_data()
            .is_some_and(|data| *data.stop_reason() == StopReason::EndTurn);
        if end_turn && self.after_model.fetch_add(1, Ordering::SeqCst) == 0 {
            return Ok(guide("Please be more specific in your answer").into());
        }
        Ok(proceed().into())
    }

    async fn after_tool_call(&self, _event: &mut AfterToolCallEvent) -> strands::Result<AfterToolCallAction> {
        Ok(transform(|event: &mut AfterToolCallEvent| {
            if *event.result().status() != ToolResultStatus::Success {
                return;
            }
            let redacted = regex::Regex::new(r"\d+")
                .unwrap()
                .replace_all(
                    &serde_json::to_string(event.result().content()).unwrap(),
                    "[REDACTED]",
                )
                .into_owned();
            let result = ToolResultBlock::new(
                event.result().tool_use_id().to_owned(),
                ToolResultStatus::Success,
                vec![redacted.into()],
            );
            event.set_result(result);
        })
        .into())
    }
}

fn count_results(messages: &[Value], status: &str) -> usize {
    tool_results(messages)
        .iter()
        .filter(|result| result["status"] == status)
        .count()
}

/// TS cases: `deny on beforeToolCall only blocks the specified tool`, `handlers execute in
/// registration order and first deny short-circuits`, `proceed from first handler allows second
/// handler to evaluate`, `intervention handler can inspect tool name to apply per-tool policies`,
/// `transform on beforeToolCall modifies tool input`.
#[tokio::test]
#[ignore = "live bedrock"]
async fn live_deny_blocks_only_the_specified_tool_and_transform_modifies_tool_input() {
    if !credentials_available().await {
        return;
    }
    let later_calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&later_calls);
    let mut agent = Agent::builder()
        .model(bedrock(500))
        .system_prompt(
            "When asked about time and weather, use BOTH time_tool AND weather_tool. Always use both tools. \
             When asked to echo something, call echo_tool with that message.",
        )
        .tools(strands::tool_list![TimeTool, WeatherTool, EchoTool])
        .intervention(ToolPolicy::new("deny-specific-tool", |event| {
            if event.tool_use().name() == "time_tool" {
                return Ok(deny("Tool \"time_tool\" is not allowed").into());
            }
            Ok(proceed().into())
        }))
        .intervention(ToolPolicy::new("transform-tool-input", move |event| {
            seen.fetch_add(1, Ordering::SeqCst);
            if event.tool_use().name() != "echo_tool" {
                return Ok(proceed().into());
            }
            Ok(transform(|event: &mut BeforeToolCallEvent| {
                let message = event.tool_use().input()["message"]
                    .as_str()
                    .unwrap_or("hello")
                    .to_owned();
                let tool_use = ToolUse::new(
                    event.tool_use().name().to_owned(),
                    event.tool_use().tool_use_id().to_owned(),
                    json!({ "message": format!("[TRANSFORMED] {message}") }),
                );
                event.set_tool_use(tool_use);
            })
            .into())
        }))
        .printer(false)
        .build()
        .unwrap();

    let result = agent
        .invoke("What is the time and weather? Also echo the message \"hello world\".")
        .await
        .unwrap();

    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    let messages = trace(&agent);
    assert!(tool_attempted(&messages, "time_tool"));
    let denied = results_of(&messages, "time_tool");
    assert!(denied.iter().all(|result| result["status"] == "error"));
    assert!(tool_result_contains(
        &messages,
        "time_tool",
        "DENIED: Tool \\\"time_tool\\\" is not allowed"
    ));
    assert!(tool_succeeded(&messages, "weather_tool"));
    assert!(tool_result_contains(&messages, "weather_tool", "sunny"));
    assert!(tool_result_contains(
        &messages,
        "echo_tool",
        "Echo: [TRANSFORMED]"
    ));
    // The deny short-circuits: the second handler saw every call except the denied ones.
    let calls = live_support::tool_uses(&messages).len();
    assert_eq!(later_calls.load(Ordering::SeqCst), calls - denied.len());
    agent.shutdown().await.unwrap();
}

/// TS cases: `deny on beforeInvocation cancels the invocation`, `deny on beforeInvocation
/// prevents any model or tool interaction`, `deny on beforeModelCall prevents model from being
/// called`. No model call happens, so these also pass without model access.
#[tokio::test]
#[ignore = "live bedrock"]
async fn live_deny_on_before_invocation_and_before_model_call_prevents_the_model_call() {
    if !credentials_available().await {
        return;
    }
    for invocation in [true, false] {
        let mut agent = Agent::builder()
            .model(bedrock(200))
            .tools([TimeTool])
            .intervention(DenyAt { invocation })
            .printer(false)
            .build()
            .unwrap();
        let result = agent.invoke("What time is it?").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        let last = serde_json::to_value(result.last_message()).unwrap();
        assert!(last["content"][0]["text"].as_str().unwrap().contains("DENIED"));
        assert_eq!(result.metrics().accumulated_usage().output_tokens(), 0);
        assert!(tool_results(&trace(&agent)).is_empty());
        agent.shutdown().await.unwrap();
    }
}

/// TS cases: `confirm pauses agent execution and resumes with approval`, `confirm with denial
/// blocks tool execution`, `confirm interrupt includes handler name and prompt as reason`,
/// `confirm interrupt works via stream API`, `confirm accepts whitespace-padded "  yes  "`,
/// `confirm rejects boolean false`.
#[tokio::test]
#[ignore = "live bedrock"]
async fn live_confirm_pauses_agent_execution_and_resumes_with_approval_or_denial() {
    if !credentials_available().await {
        return;
    }
    for (answer, approved) in [(json!("  yes  "), true), (json!(false), false)] {
        let mut agent = Agent::builder()
            .model(bedrock(300))
            .system_prompt("Use the time_tool to answer time questions.")
            .tools([TimeTool])
            .intervention(ToolPolicy::new("confirm-tool", |event| {
                Ok(confirm(format!("Approve use of {}?", event.tool_use().name())).into())
            }))
            .printer(false)
            .build()
            .unwrap();
        let mut paused = None;
        {
            let mut stream = agent.stream("What time is it?");
            while let Some(event) = stream.next().await {
                if let strands::AgentStreamEvent::AgentResult(result) = event.unwrap() {
                    paused = Some(result);
                }
            }
        }
        let paused = paused.expect("the stream ends with the result");
        assert_eq!(paused.stop_reason(), StopReason::Interrupt);
        let interrupt = paused.interrupts().first().expect("an interrupt");
        assert_eq!(interrupt.name(), "confirm-tool");
        assert_eq!(interrupt.reason(), Some(&json!("Approve use of time_tool?")));
        assert!(tool_results(&trace(&agent)).is_empty());

        let done = resume_until_done(&mut agent, paused, |_| answer.clone()).await;
        assert_eq!(done.stop_reason(), StopReason::EndTurn);
        let messages = trace(&agent);
        if approved {
            assert!(tool_result_contains(&messages, "time_tool", "12:00"));
        } else {
            assert!(!tool_succeeded(&messages, "time_tool"));
            assert!(count_results(&messages, "error") >= 1);
        }
        agent.shutdown().await.unwrap();
    }
}

/// TS cases: `confirm with preemptive approval does not pause agent`, `confirm with preemptive
/// denial blocks tool without pausing`, `confirm with custom evaluate uses custom approval
/// logic`, `guide on beforeToolCall cancels tool with feedback for model`, `accumulates feedback
/// from multiple guide handlers into one cancellation`.
#[tokio::test]
#[ignore = "live bedrock"]
async fn live_preemptive_confirm_does_not_pause_and_guides_accumulate_into_one_cancellation() {
    if !credentials_available().await {
        return;
    }
    let guide_once = |name: &'static str, feedback: &'static str| {
        let guided = AtomicUsize::new(0);
        ToolPolicy::new(name, move |event| {
            if event.tool_use().name() == "weather_tool" && guided.fetch_add(1, Ordering::SeqCst) == 0 {
                return Ok(guide(feedback).into());
            }
            Ok(proceed().into())
        })
    };
    let mut agent = Agent::builder()
        .model(bedrock(500))
        .system_prompt(
            "When asked about time and weather, use BOTH time_tool AND weather_tool. \
             If a tool call is cancelled with guidance, call that tool again.",
        )
        .tools(strands::tool_list![TimeTool, WeatherTool])
        .intervention(ToolPolicy::new("preemptive-confirm", |event| {
            if event.tool_use().name() != "time_tool" {
                return Ok(proceed().into());
            }
            Ok(confirm("Approve time_tool?")
                .with_response(json!("magic-word"))
                .with_evaluate(|response| response == &json!("magic-word"))
                .into())
        }))
        .intervention(guide_once("guide-one", "Please use a different approach"))
        .intervention(guide_once("guide-two", "Also double-check the location"))
        .printer(false)
        .build()
        .unwrap();

    let result = agent.invoke("What is the time and weather?").await.unwrap();

    // The preemptive answer approves the call without an interrupt.
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert!(result.interrupts().is_empty());
    let messages = trace(&agent);
    assert!(tool_result_contains(&messages, "time_tool", "12:00"));
    assert!(tool_result_contains(
        &messages,
        "weather_tool",
        "GUIDANCE: [guide-one] Please use a different approach\\n[guide-two] Also double-check the location"
    ));
    agent.shutdown().await.unwrap();
}

/// TS cases: `guide on afterModelCall triggers retry with feedback`, `guide on beforeModelCall
/// injects feedback as user message`, `transform on afterToolCall modifies tool output`,
/// `transform on afterToolCall can redact sensitive data before model sees it`, `handler can
/// implement multiple lifecycle methods`, `interventions work correctly when using
/// agent.stream()`.
#[tokio::test]
#[ignore = "live bedrock"]
async fn live_guides_on_model_calls_inject_feedback_and_after_tool_transform_redacts_output() {
    if !credentials_available().await {
        return;
    }
    let mut agent = Agent::builder()
        .model(bedrock(400))
        .system_prompt("Use the time_tool to answer time questions.")
        .tools([TimeTool])
        .intervention(GuideAndRedact {
            before_model: AtomicUsize::new(0),
            after_model: AtomicUsize::new(0),
        })
        .printer(false)
        .build()
        .unwrap();
    let mut model_calls = 0;
    let mut stop = None;
    {
        let mut stream = agent.stream("What time is it?");
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                strands::AgentStreamEvent::BeforeModelCall(_) => model_calls += 1,
                strands::AgentStreamEvent::AgentResult(result) => stop = Some(result.stop_reason()),
                _ => {}
            }
        }
    }
    assert_eq!(stop, Some(StopReason::EndTurn));
    let messages = trace(&agent);
    let user_texts: Vec<&str> = messages
        .iter()
        .filter(|message| message["role"] == "user")
        .filter_map(|message| message["content"][0]["text"].as_str())
        .collect();
    assert!(user_texts.contains(&"[guide-and-redact] Always answer in one short sentence"));
    assert!(user_texts.contains(&"[guide-and-redact] Please be more specific in your answer"));
    // One call for the tool use, one guided answer and its retry.
    assert!(model_calls >= 3);
    assert!(tool_result_contains(
        &messages,
        "time_tool",
        "[REDACTED]:[REDACTED]"
    ));
    assert!(!tool_result_contains(&messages, "time_tool", "12:00"));
    agent.shutdown().await.unwrap();
}

/// TS cases: `onError=throw propagates handler errors`, `onError=proceed swallows error and
/// allows tool to run`, `onError=deny fails closed and blocks the tool`, `onError=deny
/// short-circuits before later handlers run`.
#[tokio::test]
#[ignore = "live bedrock"]
async fn live_on_error_policies_throw_proceed_and_deny() {
    if !credentials_available().await {
        return;
    }
    let run = |on_error: OnError| async move {
        let later = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&later);
        let mut agent = Agent::builder()
            .model(bedrock(300))
            .system_prompt("Use the time_tool to answer time questions.")
            .tools([TimeTool])
            .intervention(
                ToolPolicy::new("error-handler", |_| {
                    Err(strands::Error::other("Handler exploded"))
                })
                .on_error(on_error),
            )
            .intervention(ToolPolicy::new("later", move |_| {
                seen.fetch_add(1, Ordering::SeqCst);
                Ok(proceed().into())
            }))
            .printer(false)
            .build()
            .unwrap();
        let result = agent.invoke("What time is it?").await;
        let messages = trace(&agent);
        agent.shutdown().await.unwrap();
        (result, messages, later.load(Ordering::SeqCst))
    };

    let (result, _, later) = run(OnError::Throw).await;
    assert_eq!(result.unwrap_err().to_string(), "Handler exploded");
    assert_eq!(later, 0);

    let (result, messages, later) = run(OnError::Proceed).await;
    assert_eq!(result.unwrap().stop_reason(), StopReason::EndTurn);
    assert!(tool_result_contains(&messages, "time_tool", "12:00"));
    assert!(later >= 1);

    let (result, messages, later) = run(OnError::Deny).await;
    assert_eq!(result.unwrap().stop_reason(), StopReason::EndTurn);
    assert!(tool_result_contains(
        &messages,
        "time_tool",
        "DENIED: Handler threw: Handler exploded"
    ));
    assert!(!tool_succeeded(&messages, "time_tool"));
    assert_eq!(later, 0);
}

#[derive(Clone)]
struct UserId(&'static str);

fn cedar(policies: &str) -> CedarAuthorization {
    CedarAuthorization::new(
        CedarAuthorizationConfig::builder()
            .policies(format!("{FIXTURES}/{policies}"))
            .entities(CedarEntities::File(format!("{FIXTURES}/entities.json").into()))
            .principal_resolver(|state: &InvocationState| {
                Ok(state.get::<UserId>().map(|UserId(id)| TypeAndId::new("User", id)))
            })
            .build(),
    )
    .unwrap()
}

fn as_user(user: Option<&'static str>) -> InvokeOptions {
    let invocation_state = InvocationState::new();
    if let Some(user) = user {
        invocation_state.insert(UserId(user));
    }
    InvokeOptions {
        invocation_state,
        ..InvokeOptions::default()
    }
}

#[tokio::test]
#[ignore = "live bedrock"]
async fn live_allows_tool_execution_when_policy_permits() {
    if !credentials_available().await {
        return;
    }
    let mut agent = Agent::builder()
        .model(bedrock(300))
        .tools([Search])
        .intervention(cedar("test.cedar"))
        .printer(false)
        .build()
        .unwrap();
    let result = agent
        .invoke_with("Search for \"cedar policy\"", as_user(Some("alice")))
        .await
        .unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    let messages = trace(&agent);
    assert!(tool_succeeded(&messages, "search"));
    assert!(tool_result_contains(&messages, "search", "Results for:"));
    agent.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "live bedrock"]
async fn live_denies_tool_execution_when_policy_forbids() {
    if !credentials_available().await {
        return;
    }
    let mut agent = Agent::builder()
        .model(bedrock(300))
        .tools(strands::tool_list![Search, DeleteRecord])
        .intervention(cedar("permit-search-deny-delete.cedar"))
        .printer(false)
        .build()
        .unwrap();
    let result = agent
        .invoke_with("Delete record 42", as_user(Some("alice")))
        .await
        .unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    let messages = trace(&agent);
    assert!(tool_attempted(&messages, "delete_record"));
    assert!(!tool_succeeded(&messages, "delete_record"));
    assert!(tool_result_contains(
        &messages,
        "delete_record",
        "DENIED: Access denied by Cedar policy"
    ));
    agent.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "live bedrock"]
async fn live_denies_all_tools_when_no_principal_identity_fail_closed() {
    if !credentials_available().await {
        return;
    }
    let mut agent = Agent::builder()
        .model(bedrock(300))
        .tools([Search])
        .intervention(cedar("permit-all.cedar"))
        .printer(false)
        .build()
        .unwrap();
    let result = agent
        .invoke_with("Search for \"fail closed\"", as_user(None))
        .await
        .unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    let messages = trace(&agent);
    assert!(tool_attempted(&messages, "search"));
    assert!(!tool_succeeded(&messages, "search"));
    assert!(tool_result_contains(&messages, "search", "DENIED"));
    agent.shutdown().await.unwrap();
}
