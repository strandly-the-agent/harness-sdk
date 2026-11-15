//! The section 13 hooks, structured output and cancellation target.
//!
//! The hook API takes `Option<Cancel>` and exposes the tool name as a field, so the deny hook reads
//! `event.set_cancel(Some(..))` and `event.tool_use().name`.

#![expect(clippy::print_stdout, reason = "an example prints its output")]

use std::time::Duration;

use strands::hooks::BeforeToolCallEvent;
use strands::types::tools::ToolUse;
use strands::{Agent, CancelOptions, InvokeOptions};

#[expect(clippy::unused_async, reason = "a real audit log would await its sink")]
async fn audit_log(tool_use: &ToolUse) {
    println!("tool call: {}", tool_use.name);
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Summary {
    title: String,
    points: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<(), strands::Error> {
    let mut agent = Agent::builder().build()?;

    agent.add_hook(|event: &mut BeforeToolCallEvent| {
        if event.tool_use().name == "shell" {
            event.set_cancel(Some("shell is disabled".into()));
        }
    });

    agent.add_async_hook::<BeforeToolCallEvent>(|event| {
        Box::pin(async move {
            audit_log(event.tool_use()).await;
        })
    });

    let cancel = agent.cancel_handle();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        cancel.cancel(CancelOptions::default());
    });

    let summary: Summary = agent
        .invoke_with(
            "Summarize this thread",
            InvokeOptions::default().structured_output_schema::<Summary>(),
        )
        .await?
        .structured_output()?;
    println!("{}: {:?}", summary.title, summary.points);
    Ok(())
}
