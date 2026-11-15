//! The agent side of MCP tool sources (TS `Agent.initialize` MCP block).

use std::sync::Arc;

use crate::error::Result;
use crate::hooks::HookRegistry;
use crate::tools::Tool;
use crate::types::agent::LocalAgent;

use super::McpClient;

/// Lists every client's tools concurrently and registers them, in client order.
pub(crate) async fn initialize(
    clients: &[McpClient],
    local: &mut LocalAgent,
    hooks: &HookRegistry,
) -> Result<()> {
    let listed = futures::future::try_join_all(clients.iter().map(McpClient::list_tools)).await?;
    for tools in listed {
        let tools = tools.into_iter().map(|tool| Arc::new(tool) as Arc<dyn Tool>);
        local.tool_registry_mut().add_all(tools)?;
    }
    let _ = hooks;
    Ok(())
}
