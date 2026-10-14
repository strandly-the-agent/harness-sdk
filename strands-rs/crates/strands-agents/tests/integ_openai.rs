//! Live OpenAI integration test. Run with `OPENAI_API_KEY` set and `--ignored`.

#![cfg(feature = "openai")]

use futures::StreamExt;
use strands_agents::models::{Model, OpenAIApi, OpenAIModel, OpenAIModelConfig, StreamOptions};
use strands_agents::types::{ContentBlock, Message, Role};

async fn run(api: OpenAIApi) {
    if std::env::var("OPENAI_API_KEY").is_err() {
        eprintln!("skipping: OPENAI_API_KEY is not set");
        return;
    }
    let model = OpenAIModel::new(OpenAIModelConfig {
        api,
        max_tokens: Some(64),
        ..Default::default()
    })
    .expect("model builds");
    let messages = vec![Message::new(
        Role::User,
        vec![ContentBlock::Text("Reply with the single word: ok".into())],
    )];
    let options = StreamOptions::default();
    let result = model
        .stream_aggregated(&messages, &options)
        .collect::<Vec<_>>()
        .await;
    let last = result
        .into_iter()
        .last()
        .expect("at least one item")
        .expect("no error");
    assert!(format!("{last:?}").to_lowercase().contains("ok"), "{last:?}");
}

#[tokio::test]
#[ignore = "live OpenAI (OPENAI_API_KEY)"]
async fn responses_api_round_trip() {
    run(OpenAIApi::Responses).await;
}

#[tokio::test]
#[ignore = "live OpenAI (OPENAI_API_KEY)"]
async fn chat_completions_round_trip() {
    run(OpenAIApi::Chat).await;
}
