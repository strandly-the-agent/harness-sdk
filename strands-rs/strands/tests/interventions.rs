//! Interventions through the public API, as a downstream crate uses them: a custom handler, the
//! vended `HumanInTheLoop` and `CedarAuthorization`, wired with `Agent::builder()`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test code"
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use strands::hooks::{BeforeModelCallEvent, BeforeToolCallEvent};
use strands::interventions::{
    BeforeModelCallAction, BeforeToolCallAction, Evaluate, InterventionHandler, deny, guide, proceed,
    transform,
};
use strands::testing::MockMessageModel;
use strands::tools::FunctionTool;
use strands::types::json::JsonValue;
use strands::types::messages::{ContentBlock, Role, ToolUseBlock};
use strands::vended_interventions::cedar::{
    CedarAuthorization, CedarAuthorizationConfig, CedarAuthorizationConfigBuilder, ContextEnricherInput,
    ToolDefinition, TypeAndId, cedar_authorization_config_builder,
};
use strands::vended_interventions::hitl::{
    Ask, AskFn, Classifier, ClassifierResult, HumanInTheLoop, HumanInTheLoopConfig,
    HumanInTheLoopConfigBuilder, LlmClassifierConfig, LlmClassifierConfigBuilder,
    human_in_the_loop_config_builder, llm_classifier_config_builder,
};
use strands::{Agent, InterruptResponse, StopReason, ToolError};

/// A model that calls `tool` once with `input`, then answers `Done`.
fn one_call(tool: &str, input: JsonValue) -> Arc<MockMessageModel> {
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![ToolUseBlock::new(tool, "tool-1", input).into()]);
    model.add_turn(vec![strands::types::messages::TextBlock::new("Done").into()]);
    model
}

fn counting_tool(name: &str, calls: &Arc<AtomicUsize>) -> FunctionTool {
    let calls = Arc::clone(calls);
    FunctionTool::new(name, "Counts its calls.", move |_input: JsonValue, _context| {
        calls.fetch_add(1, Ordering::SeqCst);
        async move { Ok::<_, ToolError>("ok") }
    })
}

fn tool_result_text(agent: &Agent) -> Option<String> {
    agent.messages().iter().find_map(|message| {
        message.content().iter().find_map(|block| match block {
            ContentBlock::ToolResult(result) => {
                let value = serde_json::to_value(result).ok()?;
                value["content"][0]["text"].as_str().map(str::to_owned)
            }
            _ => None,
        })
    })
}

/// Denies `shell`, rewrites every other tool call's cancel state, and guides each model call.
struct Policy;

#[strands::async_trait]
impl InterventionHandler for Policy {
    fn name(&self) -> &str {
        "policy"
    }

    async fn before_tool_call(
        &self,
        event: &mut BeforeToolCallEvent,
    ) -> strands::Result<BeforeToolCallAction> {
        if event.tool_use().name() == "shell" {
            return Ok(deny("shell is disabled").into());
        }
        Ok(transform(|event: &mut BeforeToolCallEvent| event.clear_cancel()).into())
    }

    async fn before_model_call(
        &self,
        event: &mut BeforeModelCallEvent<'_>,
    ) -> strands::Result<BeforeModelCallAction> {
        if event.agent().messages().len() == 1 {
            return Ok(guide("stay on topic").into());
        }
        Ok(proceed().into())
    }
}

// No TS case: public-API wiring of a custom handler, as a downstream crate writes it.
#[tokio::test]
async fn a_custom_handler_denies_a_tool_and_guides_the_model() {
    let calls = Arc::new(AtomicUsize::new(0));
    let model = one_call("shell", json!({ "command": "ls" }));
    let mut agent = Agent::builder()
        .model(Arc::clone(&model))
        .tools([counting_tool("shell", &calls)])
        .intervention(Policy)
        .printer(false)
        .build()
        .unwrap();

    let result = agent.invoke("List files").await.unwrap();

    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        tool_result_text(&agent).as_deref(),
        Some("DENIED: shell is disabled")
    );
    let guidance = &agent.messages()[1];
    assert_eq!(guidance.role(), Role::User);
    assert_eq!(
        serde_json::to_value(guidance.content()).unwrap(),
        json!([{ "text": "[policy] stay on topic" }])
    );
}

// No TS case: the TS duplicate-name case constructs the internal registry; this goes through the builder.
#[tokio::test]
async fn duplicate_handler_names_fail_the_build() {
    let error = Agent::builder()
        .model(Arc::new(MockMessageModel::new()))
        .intervention(Policy)
        .intervention(Policy)
        .build()
        .unwrap_err();
    assert_eq!(error.to_string(), "Duplicate intervention handler name: 'policy'");
}

// No TS case: TS tests read the private `_interventionRegistry.handlers`; Rust has the accessor.
#[test]
fn the_agent_lists_its_intervention_handlers_in_registration_order() {
    let bare = Agent::builder()
        .model(Arc::new(MockMessageModel::new()))
        .printer(false)
        .build()
        .unwrap();
    assert_eq!(bare.interventions().len(), 0);

    let hitl: Arc<dyn InterventionHandler> = Arc::new(HumanInTheLoop::new(HumanInTheLoopConfig::default()));
    let agent = Agent::builder()
        .model(Arc::new(MockMessageModel::new()))
        .intervention(Policy)
        .interventions([Arc::clone(&hitl)])
        .printer(false)
        .build()
        .unwrap();
    let names: Vec<&str> = agent.interventions().map(|handler| handler.name()).collect();
    assert_eq!(names, ["policy", HumanInTheLoop::NAME]);
    assert!(Arc::ptr_eq(agent.interventions().last().unwrap(), &hitl));
}

// No TS case: public-API smoke test of `HumanInTheLoop`; the TS cases are ported in `hitl/tests.rs`.
#[tokio::test]
async fn human_in_the_loop_interrupts_and_resumes_with_the_answer() {
    let calls = Arc::new(AtomicUsize::new(0));
    let model = one_call("deploy", json!({ "env": "prod" }));
    let mut agent = Agent::builder()
        .model(Arc::clone(&model))
        .tools([counting_tool("deploy", &calls)])
        .intervention(HumanInTheLoop::new(HumanInTheLoopConfig::default()))
        .printer(false)
        .build()
        .unwrap();

    let paused = agent.invoke("Deploy").await.unwrap();
    assert_eq!(paused.stop_reason(), StopReason::Interrupt);
    let [interrupt] = paused.interrupts() else {
        panic!("one interrupt")
    };
    assert_eq!(interrupt.name(), "strands:human-in-the-loop");
    assert_eq!(
        interrupt.reason(),
        Some(&json!("Approve \"deploy\"?\n  Input: {\"env\":\"prod\"}"))
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let responses = vec![InterruptResponse::new(interrupt.id(), json!("yes"))];
    let resumed = agent.invoke(responses).await.unwrap();
    assert_eq!(resumed.stop_reason(), StopReason::EndTurn);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

// No TS case: public-API smoke test of `Ask::callback` and trust; the TS cases are ported in `hitl/tests.rs`.
#[tokio::test]
async fn human_in_the_loop_asks_inline_and_remembers_trust() {
    let calls = Arc::new(AtomicUsize::new(0));
    let asked = Arc::new(AtomicUsize::new(0));
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![ToolUseBlock::new("deploy", "tool-1", json!({})).into()]);
    model.add_turn(vec![ToolUseBlock::new("deploy", "tool-2", json!({})).into()]);
    model.add_turn(vec![strands::types::messages::TextBlock::new("Done").into()]);
    let count = Arc::clone(&asked);
    let hitl = HumanInTheLoop::new(
        HumanInTheLoopConfig::builder()
            .enable_trust(true)
            .ask(Ask::callback(move |_prompt| {
                count.fetch_add(1, Ordering::SeqCst);
                async { Ok(Some(json!("t"))) }
            }))
            .build(),
    );
    let mut agent = Agent::builder()
        .model(model)
        .tools([counting_tool("deploy", &calls)])
        .intervention(hitl)
        .printer(false)
        .build()
        .unwrap();

    agent.invoke("Deploy twice").await.unwrap();

    assert_eq!(asked.load(Ordering::SeqCst), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        agent.app_state().get_value("hitl:trustedTools"),
        Some(json!(["deploy"]))
    );
}

// No TS case: public-API smoke test of `CedarAuthorization`; the TS cases are ported in `cedar/tests.rs`.
#[tokio::test]
async fn cedar_authorization_permits_and_denies_by_policy() {
    let cedar = |policies: &str| {
        CedarAuthorization::new(CedarAuthorizationConfig::builder().policies(policies).build()).unwrap()
    };
    let run = |handler: CedarAuthorization| async move {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut agent = Agent::builder()
            .model(one_call("search", json!({ "query": "rust" })))
            .tools([counting_tool("search", &calls)])
            .intervention(handler)
            .printer(false)
            .build()
            .unwrap();
        agent.invoke("Search").await.unwrap();
        let counts = agent.app_state().get_value("cedar-authorization");
        (calls.load(Ordering::SeqCst), tool_result_text(&agent), counts)
    };

    let permit =
        r#"permit(principal, action == Action::"search", resource) when { context.input.query == "rust" };"#;
    let (calls, _, counts) = run(cedar(permit)).await;
    assert_eq!(calls, 1);
    assert_eq!(counts, Some(json!({ "search": 1 })));

    let forbid =
        r#"permit(principal, action, resource); forbid(principal, action == Action::"search", resource);"#;
    let (calls, result, counts) = run(cedar(forbid)).await;
    assert_eq!(calls, 0);
    assert_eq!(
        result.as_deref(),
        Some("DENIED: Access denied by Cedar policy: policy1")
    );
    assert_eq!(counts, Some(json!({ "search": 0 })));
}

fn trusting<S>(
    builder: HumanInTheLoopConfigBuilder<S>,
) -> HumanInTheLoopConfigBuilder<human_in_the_loop_config_builder::SetEnableTrust<S>>
where
    S: human_in_the_loop_config_builder::State,
    S::EnableTrust: human_in_the_loop_config_builder::IsUnset,
{
    builder.enable_trust(true)
}

fn for_alice<S>(
    builder: CedarAuthorizationConfigBuilder<S>,
) -> CedarAuthorizationConfigBuilder<cedar_authorization_config_builder::SetPrincipal<S>>
where
    S: cedar_authorization_config_builder::State,
    S::Principal: cedar_authorization_config_builder::IsUnset,
{
    builder.principal(TypeAndId::new("User", "alice"))
}

fn with_prompt<S>(
    builder: LlmClassifierConfigBuilder<S>,
) -> LlmClassifierConfigBuilder<llm_classifier_config_builder::SetSystemPrompt<S>>
where
    S: llm_classifier_config_builder::State,
    S::SystemPrompt: llm_classifier_config_builder::IsUnset,
{
    builder.system_prompt("Is this risky?")
}

// No TS case: the Rust-only builders, aliases and constructors a downstream crate names.
#[tokio::test]
async fn helpers_can_name_the_vended_builders_aliases_and_constructors() {
    let approve: Evaluate = Arc::new(|response| response == "ok");
    let ask: AskFn = Arc::new(|_prompt| Box::pin(async { Ok(Some(json!("ok"))) }));
    let classifier = Classifier::function(|event| {
        let risky = event.tool_use().name() == "deploy";
        async move { Ok(ClassifierResult::new(risky)) }
    });
    let mut hitl = trusting(HumanInTheLoopConfig::builder().classifier(classifier)).build();
    hitl.evaluate = Some(approve);
    hitl.ask = Some(Ask::Callback(ask));
    let llm = with_prompt(LlmClassifierConfig::builder()).build();
    assert_eq!(llm.system_prompt.as_deref(), Some("Is this risky?"));

    let schema = json!({ "type": "object", "properties": { "query": { "type": "string" } } });
    let cedar = for_alice(
        CedarAuthorizationConfig::builder()
            .policies(r#"permit(principal == User::"alice", action == Action::"deploy", resource);"#)
            .tools(vec![
                ToolDefinition::new("deploy", schema).with_description("Deploys"),
            ]),
    )
    .build();

    let calls = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::builder()
        .model(one_call("deploy", json!({ "query": "prod" })))
        .tools([counting_tool("deploy", &calls)])
        .intervention(CedarAuthorization::new(cedar).unwrap())
        .intervention(HumanInTheLoop::new(hitl))
        .printer(false)
        .build()
        .unwrap();
    agent.invoke("Deploy").await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let state = strands::types::agent::InvocationState::new();
    let input = json!({});
    let enricher_input = ContextEnricherInput::new("deploy", &input, &state);
    assert_eq!(enricher_input.tool_name, "deploy");
}
