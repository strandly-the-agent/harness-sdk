//! Snippets for `todos-and-environment.mdx`.

async fn harness_builtin_plugins() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:harness_builtin_plugins]
    use strands_harness::{create_harness, BuiltinPluginName, HarnessAgentOptions};

    // Keep todos, drop environment; an empty list for none.
    let agent = create_harness(HarnessAgentOptions {
        builtin_plugins: Some(vec![BuiltinPluginName::Todos]),
        ..Default::default()
    })
    .await?;
    // --8<-- [end:harness_builtin_plugins]

    let _ = agent;
    Ok(())
}

async fn sdk_builtin_plugins() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:sdk_builtin_plugins]
    use strands_agents::models::BedrockModel;
    use strands_agents::{Agent, FileEditorTool};

    // No notebook tool; use FileEditorTool for a plan file
    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .tool(FileEditorTool::new())
        .system_prompt("Track the task in a plan file and update it as you work.")
        .build()?;
    // --8<-- [end:sdk_builtin_plugins]

    let _ = agent;
    Ok(())
}
