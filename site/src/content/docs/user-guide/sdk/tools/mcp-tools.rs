//! Rust snippets for `mcp-tools.mdx`.

async fn quick_start_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:quick_start]
    use strands_agents::mcp::{McpClient, McpServerConfig};
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    // Create MCP client with stdio transport
    let mcp_client = McpClient::connect(McpServerConfig::stdio(
        "uvx",
        ["awslabs.aws-documentation-mcp-server@latest"],
    ))
    .await?;

    // Hand the server's tools to the agent
    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .tools(mcp_client.list_tools().await?)
        .build()?;

    agent.invoke("What is AWS Lambda?").await?;
    // --8<-- [end:quick_start]
    Ok(())
}

async fn explicit_tools_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:explicit_tools]
    use strands_agents::mcp::{McpClient, McpServerConfig};
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    let mcp_client = McpClient::connect(McpServerConfig::stdio(
        "uvx",
        ["awslabs.aws-documentation-mcp-server@latest"],
    ))
    .await?;

    // Explicit tool listing - the client owns the connection
    let tools = mcp_client.list_tools().await?;
    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).tools(tools).build()?;

    agent.invoke("What is AWS Lambda?").await?;

    // Close the connection when the agent is done with it
    mcp_client.disconnect().await?;
    // --8<-- [end:explicit_tools]
    Ok(())
}

async fn multiple_servers_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:multiple_servers]
    use strands_agents::mcp::{McpClient, McpServerConfig};
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    let local_client = McpClient::connect(McpServerConfig::stdio(
        "uvx",
        ["awslabs.aws-documentation-mcp-server@latest"],
    ))
    .await?;

    let remote_client =
        McpClient::connect(McpServerConfig::streamable_http("https://api.example.com/mcp/"))
            .await?;

    // Combine the tools of both servers into one agent
    let mut tools = local_client.list_tools().await?;
    tools.extend(remote_client.list_tools().await?);

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).tools(tools).build()?;
    // --8<-- [end:multiple_servers]
    let _ = agent;
    Ok(())
}

async fn load_servers_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:load_servers]
    use serde_json::json;
    use strands_agents::mcp::{ListOptions, LoadOptions, McpClient};
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    let mut list = ListOptions::default();
    list.prefix_with_server_name = true;

    let clients = McpClient::load_servers(
        json!({
            "mcpServers": {
                "documentation": {
                    "command": "uvx",
                    "args": ["awslabs.aws-documentation-mcp-server@latest"]
                },
                "protected-api": {
                    "url": "https://api.example.com/mcp/",
                    "headers": { "Authorization": "Bearer ${MCP_API_TOKEN}" }
                }
            }
        }),
        LoadOptions::default(),
        list,
    )
    .await?;

    let mut tools = Vec::new();
    for client in &clients {
        tools.extend(client.list_tools().await?);
    }

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).tools(tools).build()?;
    // --8<-- [end:load_servers]
    let _ = agent;
    Ok(())
}

async fn tool_filtering_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:tool_filtering]
    use strands_agents::mcp::{McpClient, McpServerConfig, McpToolFilters};

    // Allow list - loads only the tools whose names match
    let mut filtered = McpServerConfig::stdio(
        "uvx",
        ["awslabs.aws-documentation-mcp-server@latest"],
    );
    filtered.tool_filters = Some(McpToolFilters {
        allowed: vec!["search_documentation".into(), "read_documentation".into()],
        ..Default::default()
    });
    let filtered_client = McpClient::connect(filtered).await?;

    // Patterns are regexes, anchored at the start of the server-side name
    let mut regex = McpServerConfig::stdio(
        "uvx",
        ["awslabs.aws-documentation-mcp-server@latest"],
    );
    regex.tool_filters = Some(McpToolFilters {
        allowed: vec!["search_.*".into()],
        ..Default::default()
    });
    let regex_client = McpClient::connect(regex).await?;

    // Combined filters - applies allowed first, then rejected
    let mut combined = McpServerConfig::stdio(
        "uvx",
        ["awslabs.aws-documentation-mcp-server@latest"],
    );
    combined.tool_filters = Some(McpToolFilters {
        allowed: vec![".*documentation$".into()],
        rejected: vec!["read_documentation".into()],
    });
    let combined_client = McpClient::connect(combined).await?;
    // --8<-- [end:tool_filtering]
    let _ = (filtered_client, regex_client, combined_client);
    Ok(())
}

async fn tool_prefixing_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:tool_prefixing]
    use strands_agents::mcp::{McpClient, McpServerConfig};
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    let mut aws_docs = McpServerConfig::stdio(
        "uvx",
        ["awslabs.aws-documentation-mcp-server@latest"],
    );
    aws_docs.prefix = Some("aws_docs".into());

    let mut other = McpServerConfig::stdio("uvx", ["other-mcp-server@latest"]);
    other.prefix = Some("other".into());

    let aws_docs_client = McpClient::connect(aws_docs).await?;
    let other_client = McpClient::connect(other).await?;

    // Tools will be named: aws_docs_search_documentation, other_search, etc.
    let mut tools = aws_docs_client.list_tools().await?;
    tools.extend(other_client.list_tools().await?);

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).tools(tools).build()?;
    // --8<-- [end:tool_prefixing]
    let _ = agent;
    Ok(())
}

async fn direct_invocation_scope() -> Result<(), Box<dyn std::error::Error>> {
    let mcp_client = strands_agents::mcp::McpClient::connect(
        strands_agents::mcp::McpServerConfig::stdio("uvx", ["calculator-mcp-server"]),
    )
    .await?;
    // --8<-- [start:direct_invocation]
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    // Call by the server-side tool name, not the prefixed agent-facing one
    let result = mcp_client
        .call_tool("calculator", json!({ "x": 10, "y": 20 }), CancellationToken::new())
        .await?;

    println!("Result: {:?}", result.content);
    // --8<-- [end:direct_invocation]
    Ok(())
}
