//! Snippets for `index.mdx`.

async fn get_one_running() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:get_one_running]
    // Cargo.toml:
    // strands-harness = { git = "https://github.com/strands-agents/harness-sdk" }
    use strands_harness::{create_harness, HarnessAgentOptions};

    let agent = create_harness(HarnessAgentOptions::default()).await?;
    agent
        .invoke(
            "Research the top three vector databases, compare pricing and limits, \
             and write it up in comparison.md",
        )
        .await?;
    // --8<-- [end:get_one_running]

    Ok(())
}
