//! Rust snippets for `interrupts.mdx`. Each function is a scoping wrapper, never called.

async fn hooks_before_tool_call_example() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:hooks_before_tool_call]
    use serde::Deserialize;
    use serde_json::json;
    use strands_agents::hooks::BeforeToolCallEvent;
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::tools::FunctionTool;
    use strands_agents::types::StopReason;
    use strands_agents::{Agent, InterruptParams, InterruptResponseContent};

    #[derive(Deserialize, schemars::JsonSchema)]
    struct FilePaths {
        paths: Vec<String>,
    }

    let delete_files = FunctionTool::builder("delete_files")
        .description("Delete files at the given paths")
        .typed_handler(|_input: FilePaths, _ctx| async move {
            // Implementation here
            Ok(true)
        });

    let inspect_files = FunctionTool::builder("inspect_files")
        .description("Inspect files at the given paths")
        .typed_handler(|_input: FilePaths, _ctx| async move {
            // Implementation here
            Ok(json!({}))
        });

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .system_prompt("You delete files older than 5 days")
        .tool(delete_files)
        .tool(inspect_files)
        .build()?;

    agent.hooks().add_sync_callback(|event: &mut BeforeToolCallEvent| {
        if event.tool_use.name != "delete_files" {
            return Ok(());
        }

        let paths = event.tool_use.input["paths"].clone();
        let approval: String = event.interrupt(
            InterruptParams::new("myapp-approval").with_reason(json!({ "paths": paths })),
        )?;
        if approval.to_lowercase() != "y" {
            event.cancel = "User denied permission to delete files".into();
        }
        Ok(())
    });

    let paths = json!(["a/b/c.txt", "d/e/f.txt"]);
    let mut result = agent.invoke(format!("paths=<{paths}>")).await?;

    while result.stop_reason == StopReason::Interrupt {
        let responses: Vec<InterruptResponseContent> = result
            .interrupts
            .iter()
            // In a real app, collect user input here
            .map(|interrupt| InterruptResponseContent::new(&interrupt.id, json!("y")))
            .collect();

        result = agent.invoke(responses).await?;
    }

    println!("MESSAGE: {}", json!(result.last_message));
    // --8<-- [end:hooks_before_tool_call]
    Ok(())
}

async fn hooks_before_tools_example() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:hooks_before_tools]
    use serde::Deserialize;
    use strands_agents::hooks::BeforeToolsEvent;
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::types::ContentBlock;
    use strands_agents::{Agent, InterruptParams};

    #[derive(Deserialize)]
    struct BatchApproval {
        approved: bool,
    }

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).tools([/* ... */]).build()?;

    agent.hooks().add_sync_callback(|event: &mut BeforeToolsEvent| {
        let dangerous_tools = event
            .message
            .content
            .iter()
            .filter(|block| match block {
                ContentBlock::ToolUse(tool_use) => tool_use.name == "delete_files",
                _ => false,
            })
            .count();

        if dangerous_tools > 0 {
            let response: BatchApproval = event
                .interrupt(InterruptParams::new("batch_approval").with_reason(
                    format!("Approve {dangerous_tools} dangerous calls?"),
                ))?;
            if !response.approved {
                event.cancel = "Batch cancelled by user".into();
            }
        }
        Ok(())
    });
    // --8<-- [end:hooks_before_tools]
    Ok(())
}

async fn tools_example() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:tools_example]
    use serde::Deserialize;
    use serde_json::json;
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::tools::FunctionTool;
    use strands_agents::{Agent, InterruptParams};

    #[derive(Deserialize, schemars::JsonSchema)]
    struct FilePaths {
        paths: Vec<String>,
    }

    let delete_files = FunctionTool::builder("delete_files")
        .description("Delete files at the given paths")
        .typed_handler(|input: FilePaths, ctx| async move {
            let approval: String = ctx.interrupt(
                InterruptParams::new("myapp-approval")
                    .with_reason(json!({ "paths": input.paths })),
            )?;
            if approval.to_lowercase() != "y" {
                return Ok(false);
            }

            // Implementation here

            Ok(true)
        });

    let inspect_files = FunctionTool::builder("inspect_files")
        .description("Inspect files at the given paths")
        .typed_handler(|_input: FilePaths, _ctx| async move {
            // Implementation here
            Ok(json!({}))
        });

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .system_prompt("You delete files older than 5 days")
        .tool(delete_files)
        .tool(inspect_files)
        .build()?;

    // ...
    // --8<-- [end:tools_example]
    let _ = agent;
    Ok(())
}

fn tools_rethrow_example() {
    // --8<-- [start:tools_rethrow]
    use serde::Deserialize;
    use serde_json::json;
    use strands_agents::tools::FunctionTool;
    use strands_agents::InterruptParams;

    #[derive(Deserialize, schemars::JsonSchema)]
    struct FilePaths {
        paths: Vec<String>,
    }

    let delete_files = FunctionTool::builder("delete_files")
        .description("Delete files at the given paths")
        .typed_handler(|input: FilePaths, ctx| async move {
            let approval: String = match ctx.interrupt(
                InterruptParams::new("myapp-approval")
                    .with_reason(json!({ "paths": input.paths })),
            ) {
                Ok(approval) => approval,
                // Let the agent pause for the interrupt; handle every other error here.
                Err(error) if error.is_control_flow() => return Err(error),
                Err(_) => return Ok(false),
            };
            if approval.to_lowercase() != "y" {
                return Ok(false);
            }

            // Implementation here

            Ok(true)
        });
    // --8<-- [end:tools_rethrow]
    let _ = delete_files;
}

async fn session_management_example() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:session_management]
    use std::sync::Arc;

    use serde::Deserialize;
    use serde_json::json;
    use strands_agents::hooks::BeforeToolCallEvent;
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::session::{FileStorage, SessionManager};
    use strands_agents::tools::FunctionTool;
    use strands_agents::types::StopReason;
    use strands_agents::{Agent, AgentResult, InterruptParams, InterruptResponseContent};
    use strands_agents::{InvokeInput, Result};

    #[derive(Deserialize, schemars::JsonSchema)]
    struct FilePaths {
        paths: Vec<String>,
    }

    // Server function: a fresh agent restored from the session on every call
    async fn server(input: impl Into<InvokeInput>) -> Result<AgentResult> {
        let delete_files = FunctionTool::builder("delete_files")
            .description("Delete files at the given paths")
            .typed_handler(|_input: FilePaths, _ctx| async move {
                // Implementation here
                Ok(true)
            });

        let model = BedrockModel::new(BedrockModelConfig::default()).await?;
        let storage = Arc::new(FileStorage::new("/path/to/storage"));
        let session = SessionManager::new("myapp", storage)?;
        let agent = Agent::builder()
            .model(model)
            .system_prompt("You delete files older than 5 days")
            .tool(delete_files)
            .session_manager(session)
            .build()?;

        agent.hooks().add_sync_callback(|event: &mut BeforeToolCallEvent| {
            if event.tool_use.name != "delete_files" {
                return Ok(());
            }

            // Check if user already trusted this approval
            if event.agent.state().get("myapp-approval") == Some(json!("t")) {
                return Ok(());
            }

            let paths = event.tool_use.input["paths"].clone();
            let params = InterruptParams::new("myapp-approval")
                .with_reason(json!({ "paths": paths }));
            let approval: String = event.interrupt(params)?;
            if !["y", "t"].contains(&approval.to_lowercase().as_str()) {
                event.cancel = "User denied permission to delete files".into();
            }

            event.agent.state().set("myapp-approval", approval.to_lowercase());
            Ok(())
        });

        agent.invoke(input).await
    }

    // Client function
    async fn client(paths: &[&str]) -> Result<AgentResult> {
        let mut result = server(format!("paths=<{}>", json!(paths))).await?;

        while result.stop_reason == StopReason::Interrupt {
            let responses: Vec<InterruptResponseContent> = result
                .interrupts
                .iter()
                // In a real app, collect user input here
                .map(|interrupt| InterruptResponseContent::new(&interrupt.id, json!("y")))
                .collect();

            result = server(responses).await?;
        }

        Ok(result)
    }

    let result = client(&["a/b/c.txt", "d/e/f.txt"]).await?;
    println!("MESSAGE: {}", json!(result.last_message));
    // --8<-- [end:session_management]
    Ok(())
}
