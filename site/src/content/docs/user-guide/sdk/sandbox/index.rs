//! Rust snippets for `sandbox/index.mdx`.

use strands_agents::Result;

async fn basic_usage() -> Result<()> {
    // --8<-- [start:basic_usage]
    use strands_agents::models::BedrockModel;
    use strands_agents::sandbox::DockerSandbox;
    use strands_agents::Agent;

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .sandbox(DockerSandbox::new("my-container-id"))
        .build()?;

    // The agent's shell tool executes inside the container
    agent.invoke("List all files inside the current directory").await?;
    // --8<-- [end:basic_usage]
    Ok(())
}

async fn no_sandbox() -> Result<()> {
    // --8<-- [start:no_sandbox]
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    // Explicit opt-out: no sandbox, run on host
    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .no_sandbox()
        .build()?;
    // --8<-- [end:no_sandbox]
    let _ = agent;
    Ok(())
}

async fn tool_override() -> Result<()> {
    // --8<-- [start:tool_override]
    use std::sync::Arc;

    use strands_agents::models::BedrockModel;
    use strands_agents::sandbox::DockerSandbox;
    use strands_agents::{Agent, ShellTool};

    let sandbox = Arc::new(DockerSandbox::new("agent-workspace"));

    let locked_shell = ShellTool::with_sandbox(Arc::clone(&sandbox) as _)
        .with_description("Run read-only shell commands. Do not modify files.");

    // The agent keeps locked_shell; the sandbox's own shell tool is skipped
    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .sandbox(Arc::clone(&sandbox))
        .tool(locked_shell)
        .build()?;
    // --8<-- [end:tool_override]
    let _ = agent;
    Ok(())
}

// --8<-- [start:custom_tool]
use strands_agents::sandbox::{ExecuteOptions, Sandbox};
use strands_agents::tools::ToolContext;
use strands_agents::tool;

/// Lint a file and return structured errors
#[tool]
async fn lint(path: String, ctx: ToolContext) -> Result<String> {
    let sandbox = ctx.agent.sandbox()?;
    let result = sandbox
        .execute(&format!("eslint --format json {path}"), ExecuteOptions::default())
        .await?;
    Ok(result.stdout)
}
// --8<-- [end:custom_tool]

async fn custom_tool_agent() -> Result<()> {
    // --8<-- [start:custom_tool_agent]
    use strands_agents::models::BedrockModel;
    use strands_agents::sandbox::DockerSandbox;
    use strands_agents::Agent;

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .sandbox(DockerSandbox::new("my-dev-env"))
        .tool(LintTool::new())
        .build()?;
    // Agent now has: shell (vended by the sandbox) + lint (yours)
    // --8<-- [end:custom_tool_agent]
    let _ = agent;
    Ok(())
}
