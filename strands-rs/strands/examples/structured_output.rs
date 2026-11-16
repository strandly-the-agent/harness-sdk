//! Structured output: the agent answers with a value of a Rust type. The type derives
//! `serde::Deserialize` and `schemars::JsonSchema`; its doc comments describe the fields to the
//! model, and the answer is validated against the schema.
//!
//! The default model is Amazon Bedrock's; AWS credentials and the region come from the default
//! chain.
//!
//! ```sh
//! cargo run -p strands --example structured_output
//! ```

#![expect(clippy::print_stdout, reason = "examples print their results")]

use strands::{Agent, InvokeOptions};

/// A summary of a discussion.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Summary {
    /// A title of at most eight words.
    title: String,
    /// The key points, one sentence each.
    points: Vec<String>,
}

const THREAD: &str = "\
Ana: The nightly build has failed three times this week, always in the integration tests.
Ben: It is the database container; it sometimes takes over a minute to start.
Ana: Then let's wait for its health check instead of sleeping for ten seconds.
Ben: Agreed. I'll open the change today and we watch the next three nights.";

#[tokio::main]
async fn main() -> Result<(), strands::Error> {
    let mut agent = Agent::builder()
        .system_prompt("You summarize engineering discussions.")
        .printer(false)
        .build()?;

    // The schema applies to this invocation only; `Agent::builder().structured_output_schema(..)`
    // sets one for every invocation.
    let summary: Summary = agent
        .invoke_with(
            format!("Summarize this thread:\n{THREAD}"),
            InvokeOptions::default().structured_output_schema::<Summary>(),
        )
        .await?
        .structured_output()?;

    println!("{}", summary.title);
    for point in &summary.points {
        println!("- {point}");
    }

    agent.shutdown().await
}
