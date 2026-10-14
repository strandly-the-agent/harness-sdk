//! Minimal stdio MCP server used by the MCP end-to-end tests (`tests/mcp_e2e.rs`).
//! Exposes one tool, `echo`, that returns its `text` argument.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{tool, tool_router, ServiceExt};

struct EchoServer;

#[derive(serde::Deserialize, schemars::JsonSchema, Default)]
struct EchoInput {
    /// Text to echo back.
    text: String,
}

#[tool_router(server_handler)]
impl EchoServer {
    #[tool(name = "echo", description = "Echo the given text back")]
    fn echo(&self, Parameters(EchoInput { text }): Parameters<EchoInput>) -> CallToolResult {
        CallToolResult::success(vec![ContentBlock::text(text)])
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let service = EchoServer.serve(rmcp::transport::io::stdio()).await?;
    service.waiting().await?;
    Ok(())
}
