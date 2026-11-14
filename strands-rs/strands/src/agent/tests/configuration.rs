//! Ported from strands-ts/src/agent/__tests__/agent.test.ts (the `messages`, `printer
//! configuration`, `concurrency guards`, `nested tool arrays`, `systemPrompt configuration`,
//! `model property`, `multimodal input` and `auxModel` cases not in `agent.rs`).
//!
//! TS data-shape conversions (`ContentBlockData[]`, `MessageData[]`) have no Rust equivalent:
//! the typed blocks are the only input form.

use std::sync::Arc;

use futures::StreamExt;

use crate::agent::{Agent, AgentConfig, AgentStreamEvent, InvokeArgs};
use crate::error::Error;
use crate::models::Model;
use crate::testing::MockModel;
use crate::types::{
    CachePointBlock, ContentBlock, Message, ReasoningBlock, Role, StopReason, SystemContentBlock,
    SystemPrompt, TextBlock,
};

fn model_with(text: &str) -> Arc<MockModel> {
    let model = Arc::new(MockModel::new());
    model.add_turn(vec![text.into()]);
    model
}

fn agent(model: &Arc<MockModel>) -> Agent {
    Agent::new(AgentConfig {
        model: Some((Arc::clone(model) as Arc<dyn Model>).into()),
        printer: Some(false),
        ..Default::default()
    })
    .unwrap()
}

fn same(left: &Arc<dyn Model>, right: &Arc<MockModel>) -> bool {
    std::ptr::addr_eq(Arc::as_ptr(left), Arc::as_ptr(right))
}

#[test]
fn returns_array_of_messages() {
    assert!(agent(&model_with("Hello")).messages().is_empty());
}

#[tokio::test]
async fn agent_works_correctly_with_printer_disabled() {
    let mut agent = agent(&model_with("Hello"));
    let mut last = None;
    let mut stream = agent.stream("Test");
    while let Some(event) = stream.next().await {
        last = Some(event.unwrap());
    }
    let Some(AgentStreamEvent::AgentResult(result)) = last else {
        panic!("the last event is the result");
    };
    assert_eq!(result.last_message().content, [ContentBlock::from("Hello")]);
}

#[tokio::test]
async fn releases_lock_after_errors() {
    let model = Arc::new(MockModel::new());
    model
        .add_turn_with(vec!["Partial".into()], Some(StopReason::MaxTokens), None)
        .add_turn(vec!["Success".into()]);
    let mut agent = agent(&model);

    assert!(matches!(agent.invoke("First").await, Err(Error::MaxTokens(_))));
    let result = agent.invoke("Second").await.unwrap();

    assert_eq!(result.last_message().content, [ContentBlock::from("Success")]);
}

#[test]
fn accepts_undefined_tools() {
    assert!(Agent::new(AgentConfig::default()).unwrap().tools().is_empty());
}

#[test]
fn converts_mixed_block_data_types() {
    let blocks = vec![
        SystemContentBlock::from("First block"),
        SystemContentBlock::from(CachePointBlock::new()),
        SystemContentBlock::from("Second block"),
    ];
    let agent = Agent::builder()
        .system_prompt(SystemPrompt::Blocks(blocks.clone()))
        .build()
        .unwrap();
    assert_eq!(agent.system_prompt(), Some(&SystemPrompt::Blocks(blocks)));
}

#[test]
fn accepts_array_of_class_instances() {
    let blocks = vec![
        SystemContentBlock::Text(TextBlock::new("System prompt")),
        SystemContentBlock::CachePoint(CachePointBlock::new()),
    ];
    let agent = Agent::builder()
        .system_prompt(SystemPrompt::Blocks(blocks.clone()))
        .build()
        .unwrap();
    assert_eq!(agent.system_prompt(), Some(&SystemPrompt::Blocks(blocks)));
}

#[test]
fn allows_system_prompt_to_be_set_after_initialization() {
    let mut agent = Agent::builder().system_prompt("Initial prompt").build().unwrap();
    agent.set_system_prompt(Some("Updated prompt".into()));
    assert_eq!(agent.system_prompt(), Some(&SystemPrompt::from("Updated prompt")));
}

#[test]
fn returns_the_configured_model_instance() {
    let model = Arc::new(MockModel::new());
    assert!(same(agent(&model).model(), &model));
}

#[test]
fn updates_the_model_instance() {
    let initial = Arc::new(MockModel::new());
    let replacement = Arc::new(MockModel::new());
    let mut agent = agent(&initial);
    agent.set_model(Arc::clone(&replacement) as Arc<dyn Model>);
    assert!(same(agent.model(), &replacement));
    assert!(!same(agent.model(), &initial));
}

#[tokio::test]
async fn successfully_switches_between_different_model_providers() {
    let first = model_with("Bedrock response");
    let second = model_with("OpenAI response");
    let mut agent = agent(&first);

    assert_eq!(
        agent.invoke("First prompt").await.unwrap().to_string(),
        "Bedrock response"
    );
    agent.set_model(second as Arc<dyn Model>);
    assert_eq!(
        agent.invoke("Second prompt").await.unwrap().to_string(),
        "OpenAI response"
    );
}

#[tokio::test]
async fn creates_user_message_with_single_text_block() {
    let model = model_with("Hi");
    let mut agent = agent(&model);

    agent.invoke("Hello").await.unwrap();

    assert_eq!(agent.messages()[0].role, Role::User);
    assert_eq!(agent.messages()[0].content, [ContentBlock::from("Hello")]);
    assert_eq!(
        model.requests()[0].messages[0].content,
        [ContentBlock::from("Hello")]
    );
}

#[tokio::test]
async fn creates_single_user_message_with_single_text_block() {
    let mut agent = agent(&model_with("Hi"));

    agent.invoke(vec![ContentBlock::from("Hello")]).await.unwrap();

    assert_eq!(agent.messages().len(), 2);
    assert_eq!(agent.messages()[0].content, [ContentBlock::from("Hello")]);
}

#[tokio::test]
async fn supports_all_content_block_types() {
    let model = model_with("Hi");
    let mut agent = agent(&model);
    let blocks = vec![
        ContentBlock::from("text"),
        ContentBlock::Reasoning(ReasoningBlock::new().with_text("thinking")),
        ContentBlock::CachePoint(CachePointBlock::new()),
    ];

    agent.invoke(blocks.clone()).await.unwrap();

    assert_eq!(agent.messages()[0].content, blocks);
}

#[tokio::test]
async fn appends_single_message_to_conversation() {
    let mut agent = agent(&model_with("Hi"));
    let message = Message::new(Role::User, vec!["Hello".into()]);

    agent.invoke(vec![message.clone()]).await.unwrap();

    assert_eq!(agent.messages()[0], message);
    assert_eq!(agent.messages().len(), 2);
}

#[tokio::test]
async fn handles_empty_message_array() {
    let model = model_with("Hi");
    let history = vec![Message::new(Role::User, vec!["Earlier".into()])];
    let mut agent = Agent::new(AgentConfig {
        model: Some((Arc::clone(&model) as Arc<dyn Model>).into()),
        messages: history,
        printer: Some(false),
        ..Default::default()
    })
    .unwrap();

    agent
        .invoke(InvokeArgs::from(Vec::<Message>::new()))
        .await
        .unwrap();

    assert_eq!(model.requests()[0].messages.len(), 1);
    assert_eq!(agent.messages().len(), 2);
}

#[test]
fn aux_model_is_reassignable_at_runtime() {
    let model = Arc::new(MockModel::new());
    let aux = Arc::new(MockModel::new());
    let mut agent = agent(&model);

    agent.set_aux_model(Some((Arc::clone(&aux) as Arc<dyn Model>).into()));
    assert!(same(agent.aux_model(), &aux));
    agent.set_aux_model(None);
    assert!(same(agent.aux_model(), &model));
}
