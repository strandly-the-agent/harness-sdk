//! Hooks: observe and steer the agent loop. A sync hook cancels one tool, an ordered hook
//! rewrites a tool call, an async hook audits results, and a cancel handle stops a long run.
//!
//! This example scripts the model with `strands::testing::MockMessageModel`, so it runs offline:
//!
//! ```sh
//! cargo run -p strands --example hooks
//! ```

#![expect(clippy::print_stdout, reason = "examples print their results")]

use std::time::Duration;

use serde_json::json;
use strands::hooks::{AfterToolCallEvent, BeforeToolCallEvent, HookCallbackOptions, HookOrder};
use strands::testing::MockMessageModel;
use strands::types::messages::{TextBlock, ToolUseBlock};
use strands::types::tools::ToolUse;
use strands::{Agent, CancelOptions, ToolContext, ToolError, tool};

/// Run a shell command.
#[tool]
async fn shell(
    /// The command line.
    command: String,
) -> Result<String, ToolError> {
    Ok(format!("ran `{command}`"))
}

/// Wait for a deployment to finish.
#[tool]
async fn wait_for_deploy(#[tool(context)] ctx: &ToolContext) -> Result<String, ToolError> {
    // A running tool finishes unless it watches the cancel signal.
    tokio::select! {
        () = ctx.cancel_signal().cancelled() => Ok("stopped waiting".to_owned()),
        () = tokio::time::sleep(Duration::from_secs(3600)) => Ok("deployed".to_owned()),
    }
}

async fn audit_log(tool_use: &ToolUse, status: String) {
    println!("audit: {} {} -> {status}", tool_use.name(), tool_use.input());
}

#[tokio::main]
async fn main() -> Result<(), strands::Error> {
    // The scripted model: two shell calls, an answer, then a call to the slow tool.
    let model = MockMessageModel::new();
    model.add_turn(vec![
        ToolUseBlock::new("shell", "tool-1", json!({ "command": "rm -rf /" })).into(),
        ToolUseBlock::new("shell", "tool-2", json!({ "command": "ls" })).into(),
    ]);
    model.add_turn(vec![
        TextBlock::new("One command ran and one was refused.").into(),
    ]);
    model.add_turn(vec![
        ToolUseBlock::new("wait_for_deploy", "tool-3", json!({})).into(),
    ]);

    let mut agent = Agent::builder()
        .model(model)
        .tools(strands::tool_list![Shell, WaitForDeploy])
        .printer(false)
        .build()?;

    // A sync hook takes the event by `&mut`; cancelling skips the tool with an error result.
    agent.add_hook(|event: &mut BeforeToolCallEvent| {
        let command = event.tool_use().input()["command"].as_str().unwrap_or_default();
        if command.starts_with("rm ") {
            event.set_cancel("destructive commands are disabled");
        }
    });

    // Hooks run in registration order unless given an order; this one runs before the hook
    // above and rewrites the call it sees.
    let rewrite = agent.add_hook_with(
        HookCallbackOptions {
            order: HookOrder::DEFAULT - 1,
        },
        |event: &mut BeforeToolCallEvent| {
            if event.tool_use().input()["command"] == "ls" {
                let call = event.tool_use();
                let rewritten = ToolUse::new(call.name(), call.tool_use_id(), json!({ "command": "ls -la" }));
                event.set_tool_use(rewritten);
            }
        },
    );

    // An async hook names its event with a turbofish and returns a boxed future.
    agent.add_async_hook::<AfterToolCallEvent>(|event| {
        Box::pin(async move {
            let status = format!("{:?}", event.result().status());
            audit_log(event.tool_use(), status).await;
        })
    });

    let result = agent.invoke("Clean up, then list the files.").await?;
    println!("{result}");

    // Registering a hook returns a cleanup that unregisters it.
    rewrite.remove();

    // A cancel handle stops a running invocation from another task.
    let cancel = agent.cancel_handle();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel.cancel(CancelOptions::default());
    });
    let result = agent.invoke("Deploy and wait.").await?;
    println!("stop reason: {:?}", result.stop_reason());

    agent.shutdown().await
}
