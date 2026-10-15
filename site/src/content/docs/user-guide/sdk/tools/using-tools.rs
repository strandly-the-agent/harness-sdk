//! Rust snippets for `using-tools.mdx`.

async fn adding_tools_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:adding_tools]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::tools::vended::FileEditorTool;
    use strands_agents::Agent;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).tool(FileEditorTool::new()).build()?;

    // Agent will use the fileEditor tool when appropriate
    agent.invoke("Show me the contents of a single file in this directory").await?;
    // --8<-- [end:adding_tools]
    Ok(())
}

async fn inspect_tools_scope() -> Result<(), Box<dyn std::error::Error>> {
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).build()?;

    // --8<-- [start:inspect_tools]
    // Tool names
    for tool in agent.tools() {
        println!("{}", tool.name());
    }

    // Full specs, including descriptions and input schemas
    let specs = agent.with_tool_registry(|registry| registry.specs());
    println!("{}", serde_json::to_string_pretty(&specs)?);
    // --8<-- [end:inspect_tools]
    Ok(())
}

async fn natural_language_invocation_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:natural_language_invocation]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::tools::vended::FileEditorTool;
    use strands_agents::Agent;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).tool(FileEditorTool::new()).build()?;

    // Agent decides when to use tools based on the request
    agent.invoke("Please read /tmp/ideas.md").await?;
    // --8<-- [end:natural_language_invocation]
    Ok(())
}

async fn direct_invocation_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:direct_invocation]
    use serde_json::json;
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::tools::vended::FileEditorTool;
    use strands_agents::{Agent, CallOptions};

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).tool(FileEditorTool::new()).build()?;

    // Call a tool by name. Returns a ToolResultBlock with `status`
    // (Success | Error) and `content` blocks.
    let input = json!({ "command": "view", "path": "/tmp/ideas.md" });
    let result = agent.tool("fileEditor")?.call(input.clone()).await?;
    println!("{:?} {:?}", result.status, result.content);

    // Skip recording the call in the message history.
    let options = CallOptions::default().with_record_direct_tool_call(false);
    agent.tool("fileEditor")?.call_with(input, options).await?;
    // --8<-- [end:direct_invocation]
    Ok(())
}
