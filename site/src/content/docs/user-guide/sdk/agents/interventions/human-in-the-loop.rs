//! Rust snippets for `human-in-the-loop.mdx`.

use strands_agents::types::JsonValue;
use strands_agents::{tool, Result};

/// Delete files at the given paths.
#[tool]
async fn delete_files(paths: Vec<String>) -> String {
    format!("Deleted {} files", paths.len())
}

/// Read a file.
#[tool]
async fn read_file(path: String) -> String {
    format!("Contents of {path}")
}

/// Stands in for the reader's own UI.
async fn ask_user_via_slack(prompt: String) -> Result<JsonValue> {
    Ok(JsonValue::from(prompt))
}

// =====================
// Basic (Interrupt/Resume Mode)
// =====================

async fn basic_interrupt_example() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:basic_interrupt]
    use strands_agents::interventions::vended::HumanInTheLoop;
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::types::StopReason;
    use strands_agents::{Agent, InterruptResponseContent, InvokeInput};

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .tool(DeleteFilesTool::new())
        .intervention(HumanInTheLoop::new())
        .build()?;

    // Agent pauses with stop_reason Interrupt when a tool needs approval
    let mut result = agent.invoke("Delete the temp files").await?;

    if result.stop_reason == StopReason::Interrupt {
        // Present the interrupt to the user (web UI, Slack, etc.)
        println!("{:?}", result.interrupts[0].reason);

        // Resume with the human's response
        result = agent
            .invoke(InvokeInput::InterruptResponses(vec![
                // "y", "yes", or true → approved
                InterruptResponseContent::new(&result.interrupts[0].id, "yes"),
            ]))
            .await?;
    }
    println!("{result}");
    // --8<-- [end:basic_interrupt]
    Ok(())
}

// =====================
// Stdio Mode
// =====================

async fn stdio_mode_example() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:stdio_mode]
    use std::sync::Arc;

    use strands_agents::interventions::vended::HumanInTheLoop;
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::types::JsonValue;
    use strands_agents::Agent;
    use tokio::io::{AsyncBufReadExt, BufReader};

    let hitl = HumanInTheLoop::new().ask(Arc::new(|prompt: String| {
        Box::pin(async move {
            println!("{prompt} (y/n)");
            let mut answer = String::new();
            BufReader::new(tokio::io::stdin())
                .read_line(&mut answer)
                .await?;
            Ok(JsonValue::from(answer.trim()))
        })
    }));

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .tool(DeleteFilesTool::new())
        .intervention(hitl)
        .build()?;

    agent.invoke("Delete the temp files").await?;
    // Terminal prompt:
    // Approve "delete_files"?
    //   Input: {"paths":["/tmp/a"]} (y/n)
    // --8<-- [end:stdio_mode]
    Ok(())
}

// =====================
// Custom Ask Callback
// =====================

async fn custom_ask_example() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:custom_ask]
    use std::sync::Arc;

    use strands_agents::interventions::vended::HumanInTheLoop;
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    let hitl = HumanInTheLoop::new().ask(Arc::new(|prompt: String| {
        // Your UI: Slack DM, web modal, push notification, etc.
        Box::pin(async move { ask_user_via_slack(prompt).await })
    }));

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .tool(DeleteFilesTool::new())
        .intervention(hitl)
        .build()?;

    agent.invoke("Delete the temp files").await?;
    // --8<-- [end:custom_ask]
    Ok(())
}

// =====================
// Allowed Tools
// =====================

async fn allowed_tools_example() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:allowed_tools]
    use strands_agents::interventions::vended::HumanInTheLoop;
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    let hitl = HumanInTheLoop::new()
        // Pattern syntax:
        //   "read_file"             → runs without approval
        //   "*"                     → all tools run freely (disables handler)
        //   ["*", "!delete_files"]  → all except delete_files
        .allowed_tools(["read_file"]);

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .tool(ReadFileTool::new())
        .tool(DeleteFilesTool::new())
        .intervention(hitl)
        .build()?;

    agent.invoke("Read config.json then delete /tmp/old-logs").await?;
    // Only delete_files prompts; read_file executes immediately
    // --8<-- [end:allowed_tools]
    Ok(())
}

// =====================
// Trust Mode
// =====================

async fn trust_mode_example() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:trust_mode]
    use strands_agents::interventions::vended::HumanInTheLoop;
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    let hitl = HumanInTheLoop::new().enable_trust(true).ask(stdin_ask());

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .tool(DeleteFilesTool::new())
        .intervention(hitl)
        .build()?;

    agent.invoke("Delete all log files in /tmp").await?;
    // First call: user responds "t" → approved AND remembered
    // Subsequent calls: no prompt needed for the session
    // --8<-- [end:trust_mode]
    Ok(())
}

// =====================
// Custom Evaluate
// =====================

async fn custom_evaluate_example() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:custom_evaluate]
    use std::sync::Arc;

    use strands_agents::interventions::vended::HumanInTheLoop;
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::types::JsonValue;
    use strands_agents::Agent;

    let hitl = HumanInTheLoop::new()
        .ask(stdin_ask())
        // Only approve if the user types "confirm"
        .evaluate(Arc::new(|response: &JsonValue| {
            response.as_str().is_some_and(|s| s.to_lowercase() == "confirm")
        }));

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .tool(DeleteFilesTool::new())
        .intervention(hitl)
        .build()?;

    agent.invoke("Delete the temp files").await?;
    // User must type "confirm" to approve (not just "y" or "yes")
    // --8<-- [end:custom_evaluate]
    Ok(())
}

/// The stdin `ask` callback from the "Stdio Mode" section.
fn stdin_ask() -> strands_agents::interventions::vended::hitl::AskFn {
    use std::sync::Arc;

    use tokio::io::{AsyncBufReadExt, BufReader};

    Arc::new(|prompt: String| {
        Box::pin(async move {
            println!("{prompt} (y/n)");
            let mut answer = String::new();
            BufReader::new(tokio::io::stdin())
                .read_line(&mut answer)
                .await?;
            Ok(JsonValue::from(answer.trim()))
        })
    })
}
