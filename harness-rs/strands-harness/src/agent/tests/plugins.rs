//! Ports the `createHarness` cases of `harness-ts/test/plugins/todos.test.ts` and
//! `harness-ts/test/plugins/environment.test.ts`. TS reads the agent's plugin list; an `Agent`
//! here has none to read, so these cases look at what each plugin adds: the `todo_write` tool
//! and the `<environment>` block of the first model request.

use strands::types::messages::TextBlock;

use super::*;

/// Runs one turn and returns the messages of the model request, as debug text.
async fn first_request(builtin_plugins: Option<Vec<BuiltinPluginName>>) -> String {
    let dir = tempfile::tempdir().unwrap();
    let (model, mut options) = shared_mock();
    model.add_turn(vec![TextBlock::new("hello").into()]);
    options.builtin_plugins = builtin_plugins;
    options.builtin_tools = names(&[]);
    options.context_manager = Some(ContextManagerStrategy::Off);
    options.session = SessionOption::Off;
    options.memory = MemoryOption::Off;
    let mut agent = create_harness_with(options, seams(&dir)).await.unwrap();
    agent.invoke("Hi").await.unwrap();
    let requests = model.requests();
    assert_eq!(requests.len(), 1);
    format!("{:?}", requests[0].messages)
}

/// The agent's tool names once its plugins have registered theirs.
async fn initialized_tool_names(options: HarnessAgentOptions) -> Vec<String> {
    let mut agent = harness(options).await.unwrap();
    agent.initialize().await.unwrap();
    tool_names(&agent)
}

/// `plugins/todos.test.ts`.
#[tokio::test]
async fn is_enabled_by_default_and_contributes_the_todo_write_tool() {
    let names = initialized_tool_names(mock_options()).await;
    assert_eq!(names.iter().filter(|name| *name == "todo_write").count(), 1);
}

/// `plugins/todos.test.ts`.
#[tokio::test]
async fn is_disabled_when_not_selected() {
    let names = initialized_tool_names(HarnessAgentOptions {
        builtin_plugins: Some(Vec::new()),
        ..mock_options()
    })
    .await;
    assert!(!names.is_empty());
    assert!(!names.iter().any(|name| name == "todo_write"), "{names:?}");
}

/// `plugins/environment.test.ts`.
#[tokio::test]
async fn is_enabled_by_default_and_opts_out_with_an_empty_list() {
    assert!(first_request(None).await.contains("<environment>"));
    assert!(!first_request(Some(Vec::new())).await.contains("<environment>"));
}
