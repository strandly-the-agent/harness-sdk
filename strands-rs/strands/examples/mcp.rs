//! Model Context Protocol: an agent uses the tools of an MCP server. The client starts the
//! server as a child process and talks to it over stdio; an agent takes the client as a tool
//! source.
//!
//! The server command comes from the arguments, and defaults to the AWS documentation server
//! (which needs `uvx`). The default model is Amazon Bedrock's; AWS credentials and the region
//! come from the default chain.
//!
//! ```sh
//! cargo run -p strands --example mcp --features mcp
//! cargo run -p strands --example mcp --features mcp -- npx -y @modelcontextprotocol/server-everything
//! ```

#![expect(clippy::print_stdout, reason = "examples print their results")]

use strands::mcp::{McpClient, McpClientConfig, McpTransport, StdioServerParameters};
use strands::{Agent, Tool};

#[tokio::main]
async fn main() -> Result<(), strands::Error> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        args = vec![
            "uvx".to_owned(),
            "awslabs.aws-documentation-mcp-server@latest".to_owned(),
        ];
    }
    let command = args.remove(0);

    let client = McpClient::new(McpClientConfig {
        transport: Some(McpTransport::stdio(StdioServerParameters {
            command,
            args,
            ..Default::default()
        })),
        ..Default::default()
    })?;

    // Listing connects on first use. An agent does this itself when it initializes.
    for tool in client.list_tools().await? {
        println!(
            "{}: {}",
            tool.name(),
            tool.description().lines().next().unwrap_or_default()
        );
    }

    let mut agent = Agent::builder()
        .system_prompt("You are a concise assistant. Use the tools to answer.")
        .tools([client.clone()])
        .build()?;
    agent
        .invoke("In one sentence: which tools do you have, and what are they for?")
        .await?;
    println!();

    agent.shutdown().await?;
    client.disconnect().await
}
