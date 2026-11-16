//! Ported from strands-ts/src/vended-interventions/hitl/__tests__/hitl.test.ts.
//!
//! Not ported: the `decision model classifier` cases (the decision-model classifier is out of
//! v0.1.0).

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::json;

use super::*;
use crate::agent::{Agent, InvocationContext};
use crate::interrupt::{Interrupt, InterruptSource};
use crate::models::streaming::Usage;
use crate::test_support::{mock_tool, text_result};
use crate::testing::{MockMessageModel, MockTurnOptions};
use crate::types::agent::{AgentResult, InvocationState, InvokeLimits, LocalAgent};
use crate::types::json::JsonObject;
use crate::types::messages::{ContentBlock, StopReason, TextBlock, ToolUseBlock};
use crate::types::tools::ToolUse;

const INTERRUPT_ID: &str = "hook:beforeToolCall:tool-1:strands:human-in-the-loop";

fn tool_use(name: &str, id: &str, input: JsonValue) -> ContentBlock {
    ContentBlock::ToolUse(ToolUseBlock::new(name, id, input))
}

fn text(text: &str) -> ContentBlock {
    ContentBlock::Text(TextBlock::new(text))
}

/// A model that calls `tool` once (`tool-1`), then answers `Done`.
fn one_call(tool: &str, input: JsonValue) -> Arc<MockMessageModel> {
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![tool_use(tool, "tool-1", input)]);
    model.add_turn(vec![text("Done")]);
    model
}

/// A model whose only turn is the classifier's structured answer.
fn classifier_model(requires_approval: bool, reason: &str) -> Arc<MockMessageModel> {
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![tool_use(
        "strands_structured_output",
        "inner-1",
        json!({ "requiresApproval": requires_approval, "reason": reason }),
    )]);
    model
}

fn answering(answer: &'static str) -> Ask {
    Ask::callback(move |_prompt| async move { Ok(Some(json!(answer))) })
}

/// An `ask` that counts its calls and records the prompts.
fn recording(answer: &'static str, count: &Arc<AtomicUsize>, prompts: &Arc<Mutex<Vec<String>>>) -> Ask {
    let count = Arc::clone(count);
    let prompts = Arc::clone(prompts);
    Ask::callback(move |prompt| {
        count.fetch_add(1, Ordering::SeqCst);
        prompts.lock().unwrap().push(prompt);
        async move { Ok(Some(json!(answer))) }
    })
}

struct Run {
    agent: Agent,
    executed: Arc<Mutex<Vec<String>>>,
}

impl Run {
    fn new(model: &Arc<MockMessageModel>, tools: &[&str], config: HumanInTheLoopConfig) -> Self {
        Self::with_state(model, tools, config, JsonObject::new())
    }

    fn with_state(
        model: &Arc<MockMessageModel>,
        tools: &[&str],
        config: HumanInTheLoopConfig,
        app_state: JsonObject,
    ) -> Self {
        let executed = Arc::new(Mutex::new(Vec::new()));
        let tools = tools.iter().map(|name| {
            let executed = Arc::clone(&executed);
            let name = (*name).to_owned();
            mock_tool(&name.clone(), move |context| {
                executed.lock().unwrap().push(name.clone());
                text_result(context.tool_use().tool_use_id(), "executed")
            })
        });
        let agent = Agent::builder()
            .model(Arc::clone(model))
            .tools(tools)
            .app_state(app_state)
            .intervention(HumanInTheLoop::new(config))
            .printer(false)
            .build()
            .unwrap();
        Self { agent, executed }
    }

    async fn invoke(&mut self, prompt: &str) -> AgentResult {
        self.agent.invoke(prompt).await.unwrap()
    }

    fn executed(&self) -> Vec<String> {
        self.executed.lock().unwrap().clone()
    }

    fn tool_result_text(&self) -> Option<String> {
        self.agent.messages().iter().find_map(|message| {
            message.content().iter().find_map(|block| match block {
                ContentBlock::ToolResult(result) => {
                    let value = serde_json::to_value(result).unwrap();
                    value["content"][0]["text"].as_str().map(str::to_owned)
                }
                _ => None,
            })
        })
    }

    fn trusted(&self) -> Option<JsonValue> {
        self.agent.app_state().get_value(TRUSTED_TOOLS_KEY)
    }

    /// Stores an answer to the `tool-1` approval, as a loaded snapshot would.
    fn restore_interrupt_response(&self, response: &str) {
        let interrupt = Interrupt::new(INTERRUPT_ID, HumanInTheLoop::NAME)
            .with_response(json!(response))
            .with_source(InterruptSource::Hook);
        self.agent
            .local
            .interrupt_state()
            .lock()
            .interrupts
            .insert(INTERRUPT_ID.to_owned(), interrupt);
    }
}

fn config() -> HumanInTheLoopConfig {
    HumanInTheLoopConfig::default()
}

fn state(trusted: JsonValue) -> JsonObject {
    let mut state = JsonObject::new();
    state.insert(TRUSTED_TOOLS_KEY.to_owned(), trusted);
    state
}

mod default_config_interrupt_resume {
    use super::*;

    #[tokio::test]
    async fn pauses_agent_with_interrupt_on_any_tool_call() {
        let model = one_call("anyTool", json!({ "x": 1 }));
        let mut run = Run::new(&model, &["anyTool"], config());
        let result = run.invoke("Do something").await;
        assert_eq!(result.stop_reason(), StopReason::Interrupt);
        let [interrupt] = result.interrupts() else {
            panic!("one interrupt")
        };
        assert_eq!(interrupt.id(), INTERRUPT_ID);
        assert_eq!(interrupt.name(), "strands:human-in-the-loop");
        assert_eq!(
            interrupt.reason(),
            Some(&json!("Approve \"anyTool\"?\n  Input: {\"x\":1}"))
        );
        assert!(run.executed().is_empty());
    }
}

mod inline_mode_with_ask_callback {
    use super::*;

    #[tokio::test]
    async fn allows_tool_execution_when_approved() {
        let model = one_call("myTool", json!({}));
        let mut run = Run::new(
            &model,
            &["myTool"],
            HumanInTheLoopConfig {
                ask: Some(answering("yes")),
                ..config()
            },
        );
        let result = run.invoke("Run tool").await;
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(run.executed(), ["myTool"]);
    }

    #[tokio::test]
    async fn denies_tool_execution_when_rejected() {
        let model = one_call("myTool", json!({}));
        let mut run = Run::new(
            &model,
            &["myTool"],
            HumanInTheLoopConfig {
                ask: Some(answering("no")),
                ..config()
            },
        );
        let result = run.invoke("Run tool").await;
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert!(run.executed().is_empty());
    }
}

mod allowed_tools_config {
    use super::*;

    fn counting(allowed: &[&str], count: &Arc<AtomicUsize>) -> HumanInTheLoopConfig {
        HumanInTheLoopConfig {
            allowed_tools: allowed.iter().map(|tool| (*tool).to_owned()).collect(),
            ask: Some(recording("yes", count, &Arc::default())),
            ..config()
        }
    }

    #[tokio::test]
    async fn does_not_prompt_for_tools_in_allowed_tools() {
        let count = Arc::new(AtomicUsize::new(0));
        let model = one_call("readFile", json!({}));
        let mut run = Run::new(&model, &["readFile"], counting(&["readFile"], &count));
        run.invoke("Read").await;
        assert_eq!(count.load(Ordering::SeqCst), 0);
        assert_eq!(run.executed(), ["readFile"]);
    }

    #[tokio::test]
    async fn prompts_for_tools_not_in_allowed_tools() {
        let count = Arc::new(AtomicUsize::new(0));
        let model = one_call("deleteFile", json!({}));
        let mut run = Run::new(&model, &["deleteFile"], counting(&["readFile"], &count));
        run.invoke("Delete").await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn allows_all_tools_except_negated_ones_with_prefix() {
        let count = Arc::new(AtomicUsize::new(0));
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let model = Arc::new(MockMessageModel::new());
        model.add_turn(vec![tool_use("readFile", "tool-1", json!({}))]);
        model.add_turn(vec![tool_use("deleteFile", "tool-2", json!({}))]);
        model.add_turn(vec![text("Done")]);
        let mut run = Run::new(
            &model,
            &["readFile", "deleteFile"],
            HumanInTheLoopConfig {
                allowed_tools: vec!["*".to_owned(), "!deleteFile".to_owned()],
                ask: Some(recording("yes", &count, &prompts)),
                ..config()
            },
        );
        run.invoke("Read then delete").await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(prompts.lock().unwrap()[0].contains("deleteFile"));
        assert_eq!(run.executed(), ["readFile", "deleteFile"]);
    }

    #[tokio::test]
    async fn allows_all_tools_with_wildcard() {
        let count = Arc::new(AtomicUsize::new(0));
        let model = one_call("anyTool", json!({}));
        let mut run = Run::new(&model, &["anyTool"], counting(&["*"], &count));
        run.invoke("Run").await;
        assert_eq!(count.load(Ordering::SeqCst), 0);
        assert_eq!(run.executed(), ["anyTool"]);
    }
}

mod ask_callback {
    use super::*;

    #[tokio::test]
    async fn passes_tool_name_and_input_in_the_prompt() {
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let model = one_call("sendEmail", json!({ "to": "bob@example.com" }));
        let mut run = Run::new(
            &model,
            &["sendEmail"],
            HumanInTheLoopConfig {
                ask: Some(recording("yes", &Arc::default(), &prompts)),
                ..config()
            },
        );
        run.invoke("Send email").await;
        assert_eq!(
            *prompts.lock().unwrap(),
            ["Approve \"sendEmail\"?\n  Input: {\"to\":\"bob@example.com\"}"]
        );
    }

    #[tokio::test]
    async fn supports_custom_evaluate_function() {
        let model = one_call("myTool", json!({}));
        let mut run = Run::new(
            &model,
            &["myTool"],
            HumanInTheLoopConfig::builder()
                .ask(answering("approved"))
                .evaluate(|response| response == "approved")
                .build(),
        );
        run.invoke("Run tool").await;
        assert_eq!(run.executed(), ["myTool"]);
    }
}

mod trust_mode_enable_trust_true {
    use super::*;

    /// A model that calls `myTool` twice (`tool-1`, `tool-2`).
    fn two_calls() -> Arc<MockMessageModel> {
        let model = Arc::new(MockMessageModel::new());
        model.add_turn(vec![tool_use("myTool", "tool-1", json!({}))]);
        model.add_turn(vec![tool_use("myTool", "tool-2", json!({}))]);
        model.add_turn(vec![text("Done")]);
        model
    }

    fn trusting(answer: &'static str, enable_trust: bool, count: &Arc<AtomicUsize>) -> HumanInTheLoopConfig {
        HumanInTheLoopConfig {
            enable_trust,
            ask: Some(recording(answer, count, &Arc::default())),
            ..config()
        }
    }

    #[tokio::test]
    async fn trusts_a_tool_for_the_session_when_response_is_t() {
        let count = Arc::new(AtomicUsize::new(0));
        let mut run = Run::new(&two_calls(), &["myTool"], trusting("t", true, &count));
        run.invoke("Run twice").await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(run.executed(), ["myTool", "myTool"]);
        assert_eq!(run.trusted(), Some(json!(["myTool"])));
    }

    #[tokio::test]
    async fn does_not_trust_when_enable_trust_is_false_even_with_t_response() {
        let count = Arc::new(AtomicUsize::new(0));
        let mut run = Run::new(&two_calls(), &["myTool"], trusting("t", false, &count));
        run.invoke("Run twice").await;
        assert_eq!(count.load(Ordering::SeqCst), 2);
        assert!(run.executed().is_empty());
        assert_eq!(run.trusted(), None);
    }

    #[tokio::test]
    async fn t_response_also_approves_the_current_tool_call() {
        let count = Arc::new(AtomicUsize::new(0));
        let model = one_call("myTool", json!({}));
        let mut run = Run::new(&model, &["myTool"], trusting("t", true, &count));
        run.invoke("Run tool").await;
        assert_eq!(run.executed(), ["myTool"]);
    }

    /// TS `trusts when response is "%s"` (`trust`, `T`, `TRUST`).
    #[tokio::test]
    async fn trusts_when_response_is() {
        for answer in ["trust", "T", "TRUST"] {
            let count = Arc::new(AtomicUsize::new(0));
            let mut run = Run::new(&two_calls(), &["myTool"], trusting(answer, true, &count));
            run.invoke("Run twice").await;
            assert_eq!(count.load(Ordering::SeqCst), 1, "{answer}");
            assert_eq!(run.executed(), ["myTool", "myTool"], "{answer}");
        }
    }

    #[tokio::test]
    async fn supports_custom_evaluate_trust_function() {
        let count = Arc::new(AtomicUsize::new(0));
        let config = HumanInTheLoopConfig::builder()
            .enable_trust(true)
            .evaluate_trust(|response| response == "always")
            .ask(recording("always", &count, &Arc::default()))
            .build();
        let mut run = Run::new(&two_calls(), &["myTool"], config);
        run.invoke("Run twice").await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(run.executed(), ["myTool", "myTool"]);
    }

    #[tokio::test]
    async fn negated_tools_cannot_be_trusted_even_with_t_response() {
        let count = Arc::new(AtomicUsize::new(0));
        let config = HumanInTheLoopConfig {
            allowed_tools: vec!["!myTool".to_owned()],
            ..trusting("t", true, &count)
        };
        let mut run = Run::new(&two_calls(), &["myTool"], config);
        run.invoke("Run twice").await;
        assert_eq!(count.load(Ordering::SeqCst), 2);
        assert!(run.executed().is_empty());
        assert_eq!(run.trusted(), None);
    }

    #[tokio::test]
    async fn ignores_trusted_tools_in_agent_state_when_enable_trust_is_false() {
        let count = Arc::new(AtomicUsize::new(0));
        let model = one_call("myTool", json!({}));
        let mut run = Run::with_state(
            &model,
            &["myTool"],
            trusting("n", false, &count),
            state(json!(["myTool"])),
        );
        run.invoke("Run tool").await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(run.executed().is_empty());
    }

    /// TS `ignores a trusted tools value that is %s`.
    #[tokio::test]
    async fn ignores_a_trusted_tools_value_that_is() {
        let malformed = [
            ("a string", json!("myTool")),
            ("a number", json!(42)),
            ("an object", json!({ "myTool": true })),
            ("a list with non-string entries", json!(["myTool", 1])),
        ];
        for (label, trusted) in malformed {
            let count = Arc::new(AtomicUsize::new(0));
            let model = one_call("myTool", json!({}));
            let mut run = Run::with_state(&model, &["myTool"], trusting("n", true, &count), state(trusted));
            run.invoke("Run tool").await;
            assert_eq!(count.load(Ordering::SeqCst), 1, "{label}");
            assert!(run.executed().is_empty(), "{label}");
        }
    }

    #[tokio::test]
    async fn replaces_a_malformed_trusted_tools_value_when_a_tool_is_trusted() {
        let count = Arc::new(AtomicUsize::new(0));
        let model = one_call("myTool", json!({}));
        let mut run = Run::with_state(
            &model,
            &["myTool"],
            trusting("t", true, &count),
            state(json!({ "myTool": true })),
        );
        run.invoke("Run tool").await;
        assert_eq!(run.trusted(), Some(json!(["myTool"])));
    }
}

mod inline_ask_denials {
    use super::*;

    /// TS `denies without pausing when the human %s` (`rejects the call`, `returns no answer`).
    #[tokio::test]
    async fn denies_without_pausing_when_the_human() {
        let cases = [
            (
                answering("n"),
                "DENIED: Tool \"myTool\" denied: the human rejected the call.",
            ),
            (
                Ask::callback(|_prompt| async { Ok(None) }),
                "DENIED: Tool \"myTool\" denied: approval callback returned no response.",
            ),
        ];
        for (ask, expected) in cases {
            let model = one_call("myTool", json!({}));
            let mut run = Run::new(
                &model,
                &["myTool"],
                HumanInTheLoopConfig {
                    ask: Some(ask),
                    ..config()
                },
            );
            let result = run.invoke("Run tool").await;
            assert_eq!(result.stop_reason(), StopReason::EndTurn);
            assert!(run.executed().is_empty());
            assert_eq!(run.tool_result_text().as_deref(), Some(expected));
        }
    }
}

mod restored_interrupt_responses {
    use super::*;

    #[tokio::test]
    async fn uses_the_live_ask_answer_over_a_restored_answer() {
        let count = Arc::new(AtomicUsize::new(0));
        let model = one_call("myTool", json!({}));
        let mut run = Run::new(
            &model,
            &["myTool"],
            HumanInTheLoopConfig {
                ask: Some(recording("n", &count, &Arc::default())),
                ..config()
            },
        );
        run.restore_interrupt_response("y");
        run.invoke("Run tool").await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(run.executed().is_empty());
    }

    #[tokio::test]
    async fn applies_a_restored_answer_when_no_ask_callback_is_configured() {
        let model = one_call("myTool", json!({}));
        let mut run = Run::new(&model, &["myTool"], config());
        run.restore_interrupt_response("y");
        let result = run.invoke("Run tool").await;
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(run.executed(), ["myTool"]);
    }
}

mod classifier_mode {
    use super::*;

    fn llm(model: &Arc<MockMessageModel>) -> Option<Classifier> {
        Some(Classifier::Llm(LlmClassifierConfig {
            model: Some(Arc::clone(model) as Arc<dyn crate::models::model::Model>),
            system_prompt: None,
        }))
    }

    #[tokio::test]
    async fn classifier_true_uses_built_in_llm_risk_classifier() {
        let classifier = classifier_model(true, "destructive operation");
        let model = one_call("deleteFile", json!({ "path": "/data" }));
        let mut run = Run::new(
            &model,
            &["deleteFile"],
            HumanInTheLoopConfig {
                classifier: llm(&classifier),
                ..config()
            },
        );
        let result = run.invoke("Delete the file").await;
        assert_eq!(result.stop_reason(), StopReason::Interrupt);
        assert_eq!(
            result.interrupts()[0].reason(),
            Some(&json!(
                "Approve \"deleteFile\" — destructive operation?\n  Input: {\"path\":\"/data\"}"
            ))
        );
        assert!(run.executed().is_empty());
        let request = &classifier.requests()[0];
        let request = format!("{request:?}");
        assert!(
            request.contains("Should this tool call require human approval?"),
            "{request}"
        );
        assert!(request.contains("Tool: deleteFile"), "{request}");
        // The structured-output tool and the interrupt are the TS ones (conformance fixtures).
        let fixture = |text: &str| serde_json::from_str::<JsonValue>(text).unwrap();
        assert_eq!(
            serde_json::to_value(&classifier.requests()[0].tool_specs).unwrap(),
            fixture(include_str!(
                "../../../../conformance/fixtures/tools/hitl_classifier_specs.json"
            ))
        );
        assert_eq!(
            serde_json::to_value(result.interrupts()).unwrap(),
            fixture(include_str!(
                "../../../../conformance/fixtures/interrupts/hitl_approval.json"
            ))
        );
    }

    #[tokio::test]
    async fn classifier_true_evaluates_with_agent_aux_model_when_no_classifier_model_is_configured() {
        let aux = classifier_model(true, "destructive operation");
        let model = one_call("deleteFile", json!({ "path": "/data" }));
        let tool = mock_tool("deleteFile", |context| {
            text_result(context.tool_use().tool_use_id(), "deleted")
        });
        let hitl = HumanInTheLoop::new(HumanInTheLoopConfig::builder().classifier(true).build());
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .aux_model(Arc::clone(&aux))
            .tools([tool])
            .intervention(hitl)
            .printer(false)
            .build()
            .unwrap();
        let result = agent.invoke("Delete the file").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::Interrupt);
        assert_eq!(aux.requests().len(), 1);
        assert_eq!(model.requests().len(), 1);
    }

    #[tokio::test]
    async fn classifier_true_allows_tool_when_llm_says_no_approval_needed() {
        let classifier = classifier_model(false, "read-only operation");
        let model = one_call("readFile", json!({ "path": "/tmp/x" }));
        let mut run = Run::new(
            &model,
            &["readFile"],
            HumanInTheLoopConfig {
                classifier: llm(&classifier),
                ..config()
            },
        );
        let result = run.invoke("Read the file").await;
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(run.executed(), ["readFile"]);
    }

    #[tokio::test]
    async fn allowed_tools_bypasses_classifier() {
        let classifier = classifier_model(true, "never asked");
        let model = one_call("readFile", json!({}));
        let mut run = Run::new(
            &model,
            &["readFile"],
            HumanInTheLoopConfig {
                allowed_tools: vec!["readFile".to_owned()],
                classifier: llm(&classifier),
                ..config()
            },
        );
        let result = run.invoke("Read").await;
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(run.executed(), ["readFile"]);
        assert!(classifier.requests().is_empty());
    }

    fn function(reason: &'static str, only: Option<&'static str>) -> Option<Classifier> {
        let classifier: HumanInTheLoopClassifier = Arc::new(move |event| {
            let requires = only.is_none_or(|tool| event.tool_use().name() == tool);
            Box::pin(async move { Ok(ClassifierResult::new(requires).with_reason(reason)) })
        });
        Some(Classifier::Function(classifier))
    }

    #[tokio::test]
    async fn custom_classifier_function_works() {
        let model = one_call("deploy", json!({ "env": "prod" }));
        let mut run = Run::new(
            &model,
            &["deploy"],
            HumanInTheLoopConfig {
                classifier: function("deployment requires approval", Some("deploy")),
                ..config()
            },
        );
        let result = run.invoke("Deploy").await;
        assert_eq!(result.stop_reason(), StopReason::Interrupt);
        assert!(run.executed().is_empty());
    }

    #[tokio::test]
    async fn includes_classifier_reason_in_prompt() {
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let model = one_call("sendEmail", json!({ "to": "all@co.com" }));
        let mut run = Run::new(
            &model,
            &["sendEmail"],
            HumanInTheLoopConfig {
                classifier: function("external communication", None),
                ask: Some(recording("no", &Arc::default(), &prompts)),
                ..config()
            },
        );
        run.invoke("Send email").await;
        assert_eq!(
            *prompts.lock().unwrap(),
            ["Approve \"sendEmail\" — external communication?\n  Input: {\"to\":\"all@co.com\"}"]
        );
    }

    // No TS case: TS tests a failing classifier with the decision model only, which is out of v0.1.
    #[tokio::test]
    async fn requires_approval_when_the_classifier_fails() {
        let failed = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&failed);
        let classifier: HumanInTheLoopClassifier = Arc::new(move |_event| {
            flag.store(true, Ordering::SeqCst);
            Box::pin(async { Err(Error::other("classifier down")) })
        });
        let model = one_call("deploy", json!({}));
        let mut run = Run::new(
            &model,
            &["deploy"],
            HumanInTheLoopConfig {
                classifier: Some(Classifier::Function(classifier)),
                ..config()
            },
        );
        let result = run.invoke("Deploy").await;
        assert_eq!(result.stop_reason(), StopReason::Interrupt);
        assert!(failed.load(Ordering::SeqCst));
    }

    mod request_limits {
        use super::*;

        #[tokio::test]
        async fn adds_the_classifier_usage_to_the_request_total_and_runs_it_past_a_spent_limit() {
            let classifier = Arc::new(MockMessageModel::new());
            classifier.add_turn_with(
                vec![tool_use(
                    "strands_structured_output",
                    "inner-1",
                    json!({ "requiresApproval": false, "reason": "read-only operation" }),
                )],
                MockTurnOptions {
                    usage: Some(Usage::new(12, 3, 15)),
                    ..MockTurnOptions::default()
                },
            );
            let hitl = HumanInTheLoop::new(HumanInTheLoopConfig {
                classifier: llm(&classifier),
                ..config()
            });
            let limits = InvokeLimits {
                turns: NonZeroU32::new(1),
                ..InvokeLimits::default()
            };
            let parent = InvocationContext::new(Some(limits), None, false);
            parent.set_turns_taken(5);
            let mut agent = LocalAgent::new(Arc::new(MockMessageModel::new()));
            agent.set_invocation_context(Some(parent.clone()));
            let mut event = BeforeToolCallEvent::new(
                agent.tool_agent(),
                ToolUse::new("readFile", "tool-1", json!({ "path": "/tmp/x" })),
                None,
                InvocationState::new(),
            );

            let action = hitl.before_tool_call(&mut event).await.unwrap();

            assert!(matches!(action, BeforeToolCallAction::Proceed(proceed) if proceed.reason.is_none()));
            let usage = parent.usage();
            assert_eq!(
                (
                    usage.turns_taken(),
                    usage.tokens().input_tokens(),
                    usage.tokens().output_tokens(),
                    usage.tokens().total_tokens()
                ),
                (5, 12, 3, 15)
            );
        }
    }
}

/// Rust-only cases for what TS gets from the JS runtime.
mod rust_only {
    use super::*;
    use crate::test_support::WarnRecorder;

    /// The tool input as a model streams it: floats that `JSON.parse` would hold as integers.
    fn float_input() -> JsonValue {
        serde_json::from_str(r#"{"n":1.0,"big":1e21,"small":1.5e-7,"neg":-0.0,"x":100000000000000000000}"#)
            .unwrap()
    }

    async fn before_tool_call(hitl: &HumanInTheLoop, tool: &str, input: JsonValue) -> BeforeToolCallAction {
        let agent = LocalAgent::new(Arc::new(MockMessageModel::new()));
        let mut event = BeforeToolCallEvent::new(
            agent.tool_agent(),
            ToolUse::new(tool, "tool-1", input),
            None,
            InvocationState::new(),
        );
        hitl.before_tool_call(&mut event).await.unwrap()
    }

    // No TS case: `JSON.stringify` formats the numbers in TS.
    #[tokio::test]
    async fn the_approval_prompt_writes_numbers_as_json_stringify_does() {
        let hitl = HumanInTheLoop::new(config());
        let BeforeToolCallAction::Confirm(confirm) = before_tool_call(&hitl, "deploy", float_input()).await
        else {
            panic!("expected a confirm");
        };
        assert_eq!(
            confirm.prompt,
            "Approve \"deploy\"?\n  Input: {\"n\":1,\"big\":1e+21,\"small\":1.5e-7,\"neg\":0,\"x\":100000000000000000000}"
        );
    }

    // No TS case: `JSON.stringify(input, null, 2)` formats the numbers in TS.
    #[tokio::test]
    async fn the_llm_classifier_prompt_writes_numbers_as_json_stringify_does() {
        let classifier = classifier_model(false, "safe");
        let hitl = HumanInTheLoop::new(HumanInTheLoopConfig {
            classifier: Some(Classifier::Llm(LlmClassifierConfig {
                model: Some(Arc::clone(&classifier) as Arc<dyn crate::models::model::Model>),
                system_prompt: None,
            })),
            ..config()
        });
        let input = serde_json::from_str(r#"{"n":1.0,"neg":-0.0,"x":100000000000000000000}"#).unwrap();
        before_tool_call(&hitl, "deploy", input).await;
        let messages = serde_json::to_value(&classifier.requests()[0].messages).unwrap();
        assert_eq!(
            messages[0]["content"][0]["text"],
            "Should this tool call require human approval?\n\nTool: deploy\nInput: {\n  \"n\": 1,\n  \"neg\": 0,\n  \"x\": 100000000000000000000\n}"
        );
    }

    // No TS case: `String.prototype.trim` strips U+FEFF and keeps U+0085, unlike `str::trim`.
    #[test]
    fn a_trust_answer_is_trimmed_as_js_trims() {
        assert!(is_trust_response(&json!("\u{feff} Trust \n")));
        assert!(!is_trust_response(&json!("t\u{85}")));
    }

    // No TS case: TS passes the error to the logger as a second argument; here it is a field.
    #[test]
    fn a_failing_classifier_warns_with_the_ts_message() {
        let hitl = HumanInTheLoop::new(HumanInTheLoopConfig {
            classifier: Some(Classifier::function(|_event| async { Err(Error::other("boom")) })),
            ..config()
        });
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let (action, warnings) =
            WarnRecorder::capture(|| runtime.block_on(before_tool_call(&hitl, "deploy", json!({}))));
        assert!(matches!(action, BeforeToolCallAction::Confirm(_)));
        assert_eq!(
            warnings,
            ["tool=<deploy> | classifier failed, defaulting to approval required"]
        );
    }

    fn line(text: &str) -> StdinRead {
        let (answer, read) = oneshot::channel();
        answer.send(Ok(text.to_owned())).unwrap();
        read
    }

    // No TS case: a TS prompt can't be dropped while it waits for stdin.
    #[tokio::test]
    async fn a_dropped_prompt_leaves_its_read_for_the_next_prompt() {
        let queue = tokio::sync::Mutex::new(None);
        let (answer, read) = oneshot::channel();
        let shown = AtomicUsize::new(0);
        let show = || {
            shown.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };

        // The first prompt starts a read and is dropped before the human answers.
        let first = ask_in_turn(&queue, show, move || read);
        assert!(futures::poll!(Box::pin(first)).is_pending());
        // The second prompt adopts that read: one line, one answer, no second reader.
        let second = ask_in_turn(&queue, show, || {
            panic!("a second read would swallow the next answer")
        });
        let mut second = Box::pin(second);
        assert!(futures::poll!(&mut second).is_pending());
        answer.send(Ok("n\n".to_owned())).unwrap();
        assert_eq!(second.await.unwrap(), "n\n");

        assert_eq!(shown.load(Ordering::SeqCst), 2);
        // The read is used up: the next prompt starts its own.
        assert_eq!(ask_in_turn(&queue, show, || line("y\n")).await.unwrap(), "y\n");
    }

    // No TS case: TS queues per handler on one event loop; here stdin is shared by threads.
    #[tokio::test]
    async fn a_prompt_waits_for_the_one_before_it() {
        let queue = tokio::sync::Mutex::new(None);
        let (answer, read) = oneshot::channel();
        let shown = Mutex::new(Vec::new());
        let show = |name: &'static str| {
            let shown = &shown;
            move || {
                shown.lock().unwrap().push(name);
                Ok(())
            }
        };
        let mut first = Box::pin(ask_in_turn(&queue, show("first"), move || read));
        let mut second = Box::pin(ask_in_turn(&queue, show("second"), || line("no\n")));
        assert!(futures::poll!(&mut first).is_pending());
        assert!(futures::poll!(&mut second).is_pending());
        assert_eq!(*shown.lock().unwrap(), ["first"]);
        answer.send(Ok("yes\n".to_owned())).unwrap();
        assert_eq!(first.await.unwrap(), "yes\n");
        assert_eq!(second.await.unwrap(), "no\n");
        assert_eq!(*shown.lock().unwrap(), ["first", "second"]);
    }
}
