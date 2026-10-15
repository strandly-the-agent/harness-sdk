//! Rust snippets for `agents/index.mdx`.

async fn agent_anatomy() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:agent_anatomy]
    use strands_agents::models::BedrockModel;
    use strands_agents::{tool, Agent};

    /// Count the words in a piece of text.
    #[tool]
    async fn word_count(
        /// The text to count.
        text: String,
    ) -> String {
        format!("{} words", text.split_whitespace().count())
    }

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .system_prompt("You are a concise writing assistant.")
        .tool(WordCountTool::new())
        .build()?;

    agent.invoke("How many words are in this sentence?").await?;
    // --8<-- [end:agent_anatomy]

    Ok(())
}
