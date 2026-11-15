//! Hooks and structured output (PROMPT.md section 13), against a scripted mock model. The
//! cancel-handle part of that example arrives with cancellation (Phase 4).

#![expect(clippy::print_stdout, reason = "examples print their results")]

use strands::hooks::BeforeToolCallEvent;
use strands::testing::MockMessageModel;
use strands::types::messages::ToolUseBlock;
use strands::types::tools::ToolUse;
use strands::{Agent, InvokeOptions, JsonValue};

async fn audit_log(tool_use: &ToolUse) {
    println!("audit: {}", tool_use.name());
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Summary {
    title: String,
    points: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output: JsonValue = r#"{"title": "Thread", "points": ["one", "two"]}"#.parse()?;
    let model = MockMessageModel::new();
    model.add_turn(vec![
        ToolUseBlock::new("strands_structured_output", "tool-1", output).into(),
    ]);
    let mut agent = Agent::builder().model(model).printer(false).build()?;

    agent.add_hook(|event: &mut BeforeToolCallEvent| {
        if event.tool_use().name() == "shell" {
            event.set_cancel("shell is disabled");
        }
    });

    agent.add_async_hook::<BeforeToolCallEvent>(|event| {
        Box::pin(async move {
            audit_log(event.tool_use()).await;
        })
    });

    let summary: Summary = agent
        .invoke_with(
            "Summarize this thread",
            InvokeOptions::default().structured_output_schema::<Summary>(),
        )
        .await?
        .structured_output()?;
    println!("{}: {}", summary.title, summary.points.join(", "));
    agent.shutdown().await?;
    Ok(())
}
