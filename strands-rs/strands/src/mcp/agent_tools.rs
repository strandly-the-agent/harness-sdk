//! The agent side of MCP tool sources (TS `Agent.initialize` MCP block).

use std::sync::{Arc, Mutex, PoisonError};

use crate::error::Result;
use crate::hooks::{BeforeModelCallEvent, HookRegistry};
use crate::tools::Tool;
use crate::types::agent::LocalAgent;

use super::{McpClient, McpTool};

type Changes = Arc<Mutex<Vec<(Vec<String>, Vec<McpTool>)>>>;

/// Lists every client's tools concurrently and registers them in client order, then keeps them
/// current: a tools-list change removes the old names and adds or replaces the new tools.
///
/// TS applies a change to the registry as it arrives; here it is applied before the next model
/// call, since the agent owns its registry.
pub(crate) async fn initialize(
    clients: &[McpClient],
    local: &mut LocalAgent,
    hooks: &HookRegistry,
) -> Result<()> {
    if clients.is_empty() {
        return Ok(());
    }
    let listed = futures::future::try_join_all(clients.iter().map(McpClient::list_tools)).await?;
    for tools in listed {
        local
            .tool_registry_mut()
            .add_all(tools.into_iter().map(as_tool))?;
    }
    let changes = Changes::default();
    for client in clients {
        let changes = Arc::clone(&changes);
        client.set_on_tools_changed(Some(Arc::new(move |old, new| {
            changes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((old, new));
        })));
    }
    hooks.add_hook(move |event: &mut BeforeModelCallEvent<'_>| {
        let pending = std::mem::take(&mut *changes.lock().unwrap_or_else(PoisonError::into_inner));
        let registry = event.agent_mut().tool_registry_mut();
        for (old, new) in pending {
            for name in &old {
                registry.remove(name);
            }
            if let Err(error) = registry.add_or_replace(new.into_iter().map(as_tool)) {
                tracing::warn!("error=<{error}> | failed to register refreshed mcp tools");
            }
        }
    });
    Ok(())
}

fn as_tool(tool: McpTool) -> Arc<dyn Tool> {
    Arc::new(tool)
}
