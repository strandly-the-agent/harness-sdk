//! Ports `harness-ts/test/interventions.test.ts`.
//!
//! TS reads the resolved handler's private fields; `HumanInTheLoop` exposes none here, so the
//! cases assert on the `HumanInTheLoopConfig` the string parses to and on the resolved handler.

use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use strands::Agent;
use strands::testing::MockMessageModel;
use strands::tools::{FunctionTool, ToolError};
use strands::types::messages::{StopReason, TextBlock, ToolUseBlock};

use super::*;

fn resolve(value: impl Into<InterventionsOption>) -> Result<Vec<Arc<dyn InterventionHandler>>> {
    resolve_interventions(Some(&value.into()), None)
}

fn names(handlers: &[Arc<dyn InterventionHandler>]) -> Vec<&str> {
    handlers.iter().map(|handler| handler.name()).collect()
}

fn hitl(text: &str, ask: Option<&Ask>) -> HumanInTheLoopConfig {
    match parse(text, ask) {
        Parsed::HumanInTheLoop(config) => config,
        other => panic!("not a human-in-the-loop preset: {other:?}"),
    }
}

fn cedar_file(dir: &tempfile::TempDir) -> String {
    let path = dir.path().join("agent.cedar");
    std::fs::write(&path, "permit(principal, action == Action::\"read\", resource);").unwrap();
    path.to_str().unwrap().to_owned()
}

#[test]
fn registers_nothing_for_undefined_or_off() {
    assert!(resolve_interventions(None, None).unwrap().is_empty());
    assert!(resolve("off").unwrap().is_empty());
    assert!(resolve(["off"]).unwrap().is_empty());
}

#[test]
fn gates_every_tool_for_ask() {
    assert_eq!(names(&resolve("ask").unwrap()), [HumanInTheLoop::NAME]);
    let config = hitl("ask", None);
    assert!(config.allowed_tools.is_empty());
    assert!(config.classifier.is_none());
}

#[test]
fn uses_the_llm_classifier_for_smart() {
    assert_eq!(names(&resolve("smart").unwrap()), [HumanInTheLoop::NAME]);
    let config = hitl("smart", None);
    // No allow-list: the classifier judges every call.
    assert!(config.allowed_tools.is_empty());
    assert!(matches!(config.classifier, Some(Classifier::Enabled(true))));
}

#[test]
fn turns_a_natural_language_string_into_the_classifier_prompt() {
    let policy = "Read-only overall, but writes under ./out are fine";
    assert_eq!(names(&resolve(policy).unwrap()), [HumanInTheLoop::NAME]);
    let config = hitl(policy, None);
    assert!(config.allowed_tools.is_empty());
    let Some(Classifier::Llm(classifier)) = config.classifier else {
        panic!("an LLM classifier config");
    };
    assert_eq!(classifier.system_prompt.as_deref(), Some(policy));
    assert!(classifier.model.is_none());
}

#[test]
fn loads_a_cedar_file_path_as_a_cedar_handler_when_the_dependency_is_installed() {
    let dir = tempfile::tempdir().unwrap();
    let handlers = resolve(cedar_file(&dir)).unwrap();
    assert_eq!(names(&handlers), ["cedar-authorization"]);
}

#[test]
fn does_not_treat_a_non_cedar_file_path_as_cedar() {
    // Detection is by the `.cedar` suffix only, not by whether the file exists.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("policy.txt");
    std::fs::write(&path, "be careful").unwrap();
    let path = path.to_str().unwrap();
    assert_eq!(names(&resolve(path).unwrap()), [HumanInTheLoop::NAME]);
    assert!(matches!(hitl(path, None).classifier, Some(Classifier::Llm(_))));
}

#[test]
fn passes_a_handler_instance_through_untouched() {
    let handler: Arc<dyn InterventionHandler> = Arc::new(HumanInTheLoop::new(HumanInTheLoopConfig {
        ask: Some(Ask::Stdio),
        enable_trust: true,
        ..HumanInTheLoopConfig::default()
    }));
    let resolved = resolve(Arc::clone(&handler)).unwrap();
    assert_eq!(resolved.len(), 1);
    assert!(Arc::ptr_eq(&resolved[0], &handler));
}

#[test]
fn throws_when_two_human_approval_handlers_collide() {
    let error = resolve(["ask", "smart"]).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Two interventions share the handler name 'strands:human-in-the-loop', but an agent registers at most one per name. Layer different kinds (e.g. a Cedar policy plus one human-approval preset), not two of the same kind."
    );
    assert!(
        matches!(error, HarnessError::InterventionNameCollision { name } if name == HumanInTheLoop::NAME)
    );
}

#[test]
fn threads_the_ask_mode_into_the_handler() {
    assert!(hitl("ask", None).ask.is_none());
    for text in ["ask", "smart", "only read files"] {
        assert!(
            matches!(hitl(text, Some(&Ask::Stdio)).ask, Some(Ask::Stdio)),
            "{text}"
        );
    }
}

// Rust-only cases.

fn tool_call_model() -> Arc<MockMessageModel> {
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![ToolUseBlock::new("probe", "tool-1", json!({})).into()]);
    model.add_turn(vec![TextBlock::new("Done").into()]);
    model
}

fn agent_with(model: &Arc<MockMessageModel>, ask: Option<&Ask>, ran: &Arc<AtomicUsize>) -> Agent {
    let ran = Arc::clone(ran);
    let probe = FunctionTool::new("probe", "A probe.", move |_input, _context| {
        ran.fetch_add(1, Ordering::SeqCst);
        async { Ok::<_, ToolError>("ran") }
    });
    Agent::builder()
        .model(Arc::clone(model))
        .printer(false)
        .tool(probe)
        .interventions(resolve_interventions(Some(&"ask".into()), ask).unwrap())
        .build()
        .unwrap()
}

#[tokio::test]
async fn ask_without_a_callback_interrupts_on_the_first_gated_call() {
    let (model, ran) = (tool_call_model(), Arc::new(AtomicUsize::new(0)));
    let result = agent_with(&model, None, &ran).invoke("go").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Interrupt);
    let [interrupt] = result.interrupts() else {
        panic!("one interrupt");
    };
    assert_eq!(interrupt.name(), HumanInTheLoop::NAME);
    assert_eq!(ran.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn ask_with_a_callback_asks_it_instead_of_interrupting() {
    let (model, ran) = (tool_call_model(), Arc::new(AtomicUsize::new(0)));
    let asked = Arc::new(AtomicUsize::new(0));
    let ask = {
        let asked = Arc::clone(&asked);
        Ask::callback(move |_prompt| {
            asked.fetch_add(1, Ordering::SeqCst);
            async { Ok(Some(json!("y"))) }
        })
    };
    let result = agent_with(&model, Some(&ask), &ran).invoke("go").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!((asked.load(Ordering::SeqCst), ran.load(Ordering::SeqCst)), (1, 1));
}

#[test]
fn presets_and_the_cedar_suffix_are_matched_on_the_trimmed_string() {
    assert!(matches!(parse("  off\n", None), Parsed::Off));
    assert!(hitl(" ask ", None).classifier.is_none());
    assert!(matches!(
        hitl("\tsmart ", None).classifier,
        Some(Classifier::Enabled(true))
    ));
    assert!(
        matches!(parse(" policies/agent.cedar \n", None), Parsed::CedarFile(path) if path == "policies/agent.cedar")
    );
    // JS `trim`: U+FEFF and U+00A0 are white space, U+0085 is not.
    assert!(matches!(parse("\u{feff}off\u{a0}", None), Parsed::Off));
    assert!(matches!(parse("off\u{85}", None), Parsed::HumanInTheLoop(_)));
    // Not presets: matching is exact, and a policy keeps its own whitespace.
    for text in ["Ask", "OFF", "ask me first", "agent.cedar.txt"] {
        let Some(Classifier::Llm(classifier)) = hitl(text, None).classifier else {
            panic!("a natural-language policy: {text}");
        };
        assert_eq!(classifier.system_prompt.as_deref(), Some(text));
    }
    let Some(Classifier::Llm(classifier)) = hitl(" be careful ", None).classifier else {
        panic!("a natural-language policy");
    };
    assert_eq!(classifier.system_prompt.as_deref(), Some(" be careful "));
}

#[test]
fn a_cedar_policy_and_one_human_approval_preset_coexist_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let handlers = resolve([cedar_file(&dir), "off".to_owned(), "smart".to_owned()]).unwrap();
    assert_eq!(names(&handlers), ["cedar-authorization", HumanInTheLoop::NAME]);
}

#[test]
fn two_handler_instances_with_one_name_collide() {
    let values = vec![
        InterventionValue::handler(HumanInTheLoop::default()),
        InterventionValue::from("ask"),
    ];
    let error = resolve(values).unwrap_err();
    assert!(error.to_string().contains("at most one per name"));
}

#[test]
fn a_missing_cedar_file_is_an_sdk_error() {
    let error = resolve("/nonexistent-h12plug/agent.cedar").unwrap_err();
    assert!(matches!(error, HarnessError::Sdk(_)));
    assert_eq!(
        error.to_string(),
        "Cedar policy file not found: /nonexistent-h12plug/agent.cedar"
    );
}

#[test]
fn the_collision_text_is_the_one_in_the_ts_source() {
    let Some(source) = crate::test_support::ts_source("interventions.ts") else {
        return;
    };
    let message = HarnessError::InterventionNameCollision {
        name: "${handler.name}".to_owned(),
    }
    .to_string();
    let joined = source
        .replace("' +\n          '", "")
        .replace("` +\n          '", "");
    assert!(joined.contains(&message), "{message}");
}
