//! Rust snippets for `vended-tools.mdx`.

async fn file_editor_example_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:file_editor_example]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::tools::vended::FileEditorTool;
    use strands_agents::Agent;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).tool(FileEditorTool::new()).build()?;

    // Create, view, and edit files
    agent.invoke(r#"Create a file /tmp/config.json with {"debug": false}"#).await?;
    agent
        .invoke(r#"Replace "debug": false with "debug": true in /tmp/config.json"#)
        .await?;
    agent.invoke("View lines 1-10 of /tmp/config.json").await?;
    // --8<-- [end:file_editor_example]
    Ok(())
}

async fn http_request_example_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:http_request_example]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::tools::vended::HttpRequestTool;
    use strands_agents::Agent;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).tool(HttpRequestTool::new()).build()?;

    // Make API requests
    agent.invoke("Get data from https://api.example.com/users").await?;
    agent
        .invoke(r#"Post {"name": "John"} to https://api.example.com/users"#)
        .await?;
    // --8<-- [end:http_request_example]
    Ok(())
}

async fn shell_example_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:shell_example]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::tools::vended::ShellTool;
    use strands_agents::Agent;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).tool(ShellTool::new()).build()?;

    // List files and create a new file
    agent.invoke("List all files in the current directory").await?;
    agent
        .invoke(r#"Create a new file called notes.txt with "Hello World""#)
        .await?;
    // --8<-- [end:shell_example]
    Ok(())
}

async fn sleep_example_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:sleep_example]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::tools::vended::SleepTool;
    use strands_agents::Agent;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).tool(SleepTool::new()).build()?;
    agent.invoke("Pause for two seconds, then continue.").await?;
    // --8<-- [end:sleep_example]
    Ok(())
}

async fn sleep_custom_example_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:sleep_custom_example]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::tools::vended::{SleepConfig, SleepTool};
    use strands_agents::Agent;

    // `SleepConfig::new` also re-derives the model-facing description from the cap.
    let short_sleep = SleepTool::with_config(SleepConfig::new(5.0))?;
    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).tool(short_sleep).build()?;
    // --8<-- [end:sleep_custom_example]
    let _ = agent;
    Ok(())
}

async fn handoff_to_user_example_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:handoff_to_user_example]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::tools::vended::{HandoffToUserTool, HANDOFF_INTERRUPT_NAME};
    use strands_agents::Agent;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .system_prompt("Before deleting any files, call handoff_to_user to confirm with the user.")
        .tool(HandoffToUserTool::new())
        .build()?;

    let result = agent.invoke("Delete all .tmp files in /workspace.").await?;
    if let Some(interrupt) = result.interrupts.iter().find(|i| i.name == HANDOFF_INTERRUPT_NAME) {
        println!("{:?}", interrupt.reason);
    }
    // --8<-- [end:handoff_to_user_example]
    Ok(())
}

async fn mcp_router_example_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:mcp_router_example]
    use strands_agents::mcp::McpServerConfig;
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::tools::vended::{McpRouterConfig, McpRouterTool};
    use strands_agents::Agent;

    let mut config = McpRouterConfig::new([
        (
            "files".to_owned(),
            McpServerConfig {
                command: Some("npx".to_owned()),
                args: vec![
                    "-y".to_owned(),
                    "@modelcontextprotocol/server-filesystem".to_owned(),
                    "/tmp".to_owned(),
                ],
                ..Default::default()
            },
        ),
        (
            "my-api".to_owned(),
            McpServerConfig {
                url: Some("https://mcp.example.com/mcp".to_owned()),
                ..Default::default()
            },
        ),
    ]);
    config.max_connections = 5;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .tool(McpRouterTool::with_config(config)?)
        .build()?;
    agent
        .invoke(
            "Connect to 'files', list its tools, call the read_file tool on \
             /tmp/hello.txt, then disconnect.",
        )
        .await?;
    // --8<-- [end:mcp_router_example]
    Ok(())
}
