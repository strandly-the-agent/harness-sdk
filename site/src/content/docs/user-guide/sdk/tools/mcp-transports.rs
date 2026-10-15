//! Rust snippets for `mcp-transports.mdx`.

async fn stdio_transport_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:stdio_transport]
    use strands_agents::mcp::{McpClient, McpServerConfig};
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    let mut config = McpServerConfig::stdio(
        "uvx",
        ["awslabs.aws-documentation-mcp-server@latest"],
    );

    // Optional: environment and working directory for the spawned process
    config.env.insert("FASTMCP_LOG_LEVEL".into(), "ERROR".into());
    config.cwd = Some("/path/to/workspace".into());

    let mcp_client = McpClient::connect(config).await?;
    let tools = mcp_client.list_tools().await?;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).tools(tools).build()?;

    agent.invoke("What is AWS Lambda?").await?;
    // --8<-- [end:stdio_transport]
    Ok(())
}

async fn streamable_http_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:streamable_http]
    use strands_agents::mcp::{McpClient, McpServerConfig};
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    let mut config =
        McpServerConfig::streamable_http("https://api.githubcopilot.com/mcp/");

    // Authentication and any other request headers
    let token = std::env::var("MCP_PAT")?;
    config
        .headers
        .insert("Authorization".into(), format!("Bearer {token}"));

    let mcp_client = McpClient::connect(config).await?;
    let tools = mcp_client.list_tools().await?;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).tools(tools).build()?;
    // --8<-- [end:streamable_http]
    let _ = agent;
    Ok(())
}
