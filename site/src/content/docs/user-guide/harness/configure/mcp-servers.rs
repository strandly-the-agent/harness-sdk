//! Snippets for `mcp-servers.mdx`.

async fn harness_mcp_servers() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:harness_mcp_servers]
    use strands_agents::mcp::McpServerConfig;
    use strands_harness::{create_harness, HarnessAgentOptions, McpServersOption};

    // From a JSON file:
    let agent = create_harness(HarnessAgentOptions {
        mcp_servers: Some(McpServersOption::Path("./mcp.json".into())),
        ..Default::default()
    })
    .await?;

    // Or inline:
    let inline = create_harness(HarnessAgentOptions {
        mcp_servers: Some(McpServersOption::Servers(
            [(
                "filesystem".to_owned(),
                McpServerConfig::stdio(
                    "npx",
                    ["-y", "@modelcontextprotocol/server-filesystem", "."],
                ),
            )]
            .into_iter()
            .collect(),
        )),
        ..Default::default()
    })
    .await?;
    // --8<-- [end:harness_mcp_servers]

    let _ = (agent, inline);
    Ok(())
}

async fn sdk_mcp_servers() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:sdk_mcp_servers]
    use strands_agents::mcp::{McpClient, McpServerConfig};
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    // The Strands Harness SDK has no mcpServers-file loader; connect a client per server.
    let client = McpClient::connect(McpServerConfig::stdio(
        "npx",
        ["-y", "@modelcontextprotocol/server-filesystem", "."],
    ))
    .await?;
    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .tools(client.list_tools().await?)
        .build()?;
    agent.invoke("list the files in this project").await?;
    // --8<-- [end:sdk_mcp_servers]

    Ok(())
}
