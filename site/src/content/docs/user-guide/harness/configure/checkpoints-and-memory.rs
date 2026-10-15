//! Rust snippets for `harness/configure/checkpoints-and-memory.mdx`.

async fn checkpoints() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:checkpoints]
    use strands_harness::{create_harness, HarnessAgentOptions, SessionConfig, SessionOption};

    let agent = create_harness(HarnessAgentOptions {
        session: SessionOption::Config(SessionConfig {
            id: Some("refactor-parser".into()),
            ..Default::default()
        }),
        ..Default::default()
    })
    .await?;
    agent
        .invoke("Let's refactor the parser. Where should we start?")
        .await?;
    // --8<-- [end:checkpoints]

    Ok(())
}
