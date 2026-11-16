//! OpenAI: an `OpenAIModel` over the Responses API (the default) or the Chat Completions API.
//!
//! The API key comes from `OPENAI_API_KEY`, and `OPENAI_BASE_URL` points the client at a
//! compatible server.
//!
//! ```sh
//! cargo run -p strands --example openai --features openai
//! ```

#![expect(clippy::print_stdout, reason = "examples print their results")]

use strands::Agent;
use strands::models::openai::{OpenAIApi, OpenAIModel, OpenAIModelConfig, OpenAIModelOptions};

#[tokio::main]
async fn main() -> Result<(), strands::Error> {
    let model = OpenAIModel::new(OpenAIModelOptions {
        api: OpenAIApi::Responses, // or `OpenAIApi::Chat`
        config: OpenAIModelConfig {
            model_id: Some("gpt-5.4".to_owned()),
            max_tokens: Some(512),
            ..Default::default()
        },
        ..Default::default()
    })?;

    let mut agent = Agent::builder()
        .model(model)
        .system_prompt("You are a concise assistant.")
        .build()?;

    let result = agent
        .invoke("Explain what a tool call is in two sentences.")
        .await?;
    println!("\n{:?}", result.stop_reason());

    agent.shutdown().await
}
