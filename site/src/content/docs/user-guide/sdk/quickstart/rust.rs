//! Snippets for `rust.mdx`. Each provider program is its own module so the file compiles.

mod run_bedrock {
    // --8<-- [start:run_bedrock]
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    #[tokio::main]
    async fn main() -> Result<(), Box<dyn std::error::Error>> {
        // Bedrock is the default provider, so the default model is all you need.
        let agent = Agent::new(BedrockModel::default_model().await?)?;

        let result = agent.invoke("What is an agent harness, in one sentence?").await?;
        println!("{result}");
        Ok(())
    }
    // --8<-- [end:run_bedrock]
}

mod run_anthropic {
    // --8<-- [start:run_anthropic]
    use strands_agents::models::anthropic::{AnthropicModel, AnthropicModelConfig};
    use strands_agents::Agent;

    #[tokio::main]
    async fn main() -> Result<(), Box<dyn std::error::Error>> {
        // Reads ANTHROPIC_API_KEY from the environment.
        let model = AnthropicModel::new(AnthropicModelConfig {
            model_id: Some("claude-sonnet-5".into()),
            ..Default::default()
        })?;
        let agent = Agent::new(model)?;

        let result = agent.invoke("What is an agent harness, in one sentence?").await?;
        println!("{result}");
        Ok(())
    }
    // --8<-- [end:run_anthropic]
}

mod run_openai {
    // --8<-- [start:run_openai]
    use strands_agents::models::openai::{OpenAIModel, OpenAIModelConfig};
    use strands_agents::Agent;

    #[tokio::main]
    async fn main() -> Result<(), Box<dyn std::error::Error>> {
        // Reads OPENAI_API_KEY from the environment.
        let model = OpenAIModel::new(OpenAIModelConfig {
            model_id: Some("gpt-5.4".into()),
            ..Default::default()
        })?;
        let agent = Agent::new(model)?;

        let result = agent.invoke("What is an agent harness, in one sentence?").await?;
        println!("{result}");
        Ok(())
    }
    // --8<-- [end:run_openai]
}

// --8<-- [start:custom_tool]
use strands_agents::{tool, Agent, FileEditorTool};

/// Count occurrences of a specific letter in a word. Matching is case-insensitive.
#[tool]
async fn letter_counter(
    /// The input word to search in.
    word: String,
    /// The specific letter to count.
    letter: String,
) -> String {
    let needle = letter.to_lowercase();
    let count = word.to_lowercase().matches(&needle).count();
    format!("The letter '{letter}' appears {count} time(s) in '{word}'")
}
// --8<-- [end:custom_tool]

async fn tool_bedrock() -> Result<(), Box<dyn std::error::Error>> {
    use strands_agents::models::BedrockModel;

    // --8<-- [start:tool_bedrock]
    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .tool(LetterCounterTool::new())
        .tool(FileEditorTool::new())
        .build()?;

    let prompt = "How many letter R's are in 'strawberry'? Write it to answer.txt.";
    let result = agent.invoke(prompt).await?;
    println!("{result}");
    // --8<-- [end:tool_bedrock]

    Ok(())
}

async fn tool_anthropic() -> Result<(), Box<dyn std::error::Error>> {
    use strands_agents::models::anthropic::{AnthropicModel, AnthropicModelConfig};

    // --8<-- [start:tool_anthropic]
    let model = AnthropicModel::new(AnthropicModelConfig {
        model_id: Some("claude-sonnet-5".into()),
        ..Default::default()
    })?;
    let agent = Agent::builder()
        .model(model)
        .tool(LetterCounterTool::new())
        .tool(FileEditorTool::new())
        .build()?;

    let prompt = "How many letter R's are in 'strawberry'? Write it to answer.txt.";
    let result = agent.invoke(prompt).await?;
    println!("{result}");
    // --8<-- [end:tool_anthropic]

    Ok(())
}

async fn tool_openai() -> Result<(), Box<dyn std::error::Error>> {
    use strands_agents::models::openai::{OpenAIModel, OpenAIModelConfig};

    // --8<-- [start:tool_openai]
    let model = OpenAIModel::new(OpenAIModelConfig {
        model_id: Some("gpt-5.4".into()),
        ..Default::default()
    })?;
    let agent = Agent::builder()
        .model(model)
        .tool(LetterCounterTool::new())
        .tool(FileEditorTool::new())
        .build()?;

    let prompt = "How many letter R's are in 'strawberry'? Write it to answer.txt.";
    let result = agent.invoke(prompt).await?;
    println!("{result}");
    // --8<-- [end:tool_openai]

    Ok(())
}
