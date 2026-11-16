//! Ports `harness-ts/test/agent.test.ts`.

use std::sync::Arc;

use strands::models::Model;
use strands::testing::MockMessageModel;

use super::*;

fn seams(dir: &tempfile::TempDir) -> Seams {
    Seams {
        base_dir: Some(dir.path().to_owned()),
        env: Arc::new(|_| None),
    }
}

fn mock_options() -> HarnessAgentOptions {
    let model: Arc<dyn Model> = Arc::new(MockMessageModel::new());
    HarnessAgentOptions {
        model: Some(ModelSource::Model(model)),
        ..Default::default()
    }
}

fn tool_names(agent: &Agent) -> Vec<String> {
    agent.tools().map(|tool| tool.name().to_owned()).collect()
}

#[test]
fn create_harness_returns_a_send_future() {
    fn assert_send<T: Send>(_: &T) {}
    let future = create_harness(HarnessAgentOptions::default());
    assert_send(&future);
}

#[tokio::test]
async fn builds_an_agent_with_the_default_tools_on_a_model_instance() {
    let dir = tempfile::tempdir().unwrap();
    let agent = create_harness_with(mock_options(), seams(&dir)).await.unwrap();
    let names = tool_names(&agent);
    for name in [
        "shell",
        "read",
        "write",
        "edit",
        "web_fetch",
        "programmatic_tool_caller",
        "subagent",
    ] {
        assert!(
            names.iter().any(|tool| tool == name),
            "{name} missing from {names:?}"
        );
    }
    assert!(!names.iter().any(|tool| tool == "web_search"), "{names:?}");
}
