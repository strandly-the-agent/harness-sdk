//! `ContextInjector` through the public API of a running agent. The unit tests beside the plugin
//! port strands-ts/src/vended-plugins/context-injector/__tests__/plugin.test.ts.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test code"
)]

use std::sync::Arc;

use serde_json::json;
use strands::Agent;
use strands::testing::MockMessageModel;
use strands::types::messages::{ContentBlock, InjectedBlock, TextBlock};
use strands::vended_plugins::context_injector::{ContextInjector, ContextInjectorConfig, InjectionTrigger};

fn model(replies: &[&str]) -> Arc<MockMessageModel> {
    let model = Arc::new(MockMessageModel::new());
    for reply in replies {
        model.add_turn(vec![TextBlock::new(*reply).into()]);
    }
    model
}

// No TS case: public-API check of the plugin on a running agent.
#[tokio::test]
async fn renders_from_app_state_and_keeps_the_block_in_the_conversation() {
    let model = model(&["ok"]);
    let config = ContextInjectorConfig::new(|context| async move {
        let theme: Option<String> = context.app_state.get("theme")?;
        Ok(theme.map(|theme| format!("<theme>{theme}</theme>")))
    })
    .with_name("theme")
    .with_trigger(InjectionTrigger::EveryTurn);
    let mut agent = Agent::builder()
        .model(Arc::clone(&model))
        .printer(false)
        .plugin(ContextInjector::new(config))
        .build()
        .unwrap();
    agent.app_state().set("theme", "dark").unwrap();

    agent.invoke("what theme?").await.unwrap();

    let expected: [ContentBlock; 2] = [
        TextBlock::new("what theme?").into(),
        InjectedBlock::new("<theme>dark</theme>", "theme").into(),
    ];
    assert_eq!(agent.messages()[0].content(), expected);
    let request = &model.requests()[0];
    assert_eq!(
        serde_json::to_value(request.messages[0].content()).unwrap(),
        json!([{ "text": "what theme?" }, { "text": "\n\n<theme>dark</theme>" }])
    );
    assert_eq!(request.dynamic_trailing_blocks, Some(1));
}

// No TS case: public-API check of the predicate trigger on a running agent.
#[tokio::test]
async fn a_predicate_trigger_gates_the_injection() {
    let model = model(&["one", "two"]);
    let config = ContextInjectorConfig::new(|_context| async { Ok(Some("reminder".to_owned())) })
        .with_trigger_fn(|context| context.messages.len() > 1);
    let mut agent = Agent::builder()
        .model(Arc::clone(&model))
        .printer(false)
        .plugin(ContextInjector::new(config))
        .build()
        .unwrap();

    agent.invoke("first").await.unwrap();
    agent.invoke("second").await.unwrap();

    assert_eq!(agent.messages()[0].content().len(), 1);
    let expected: [ContentBlock; 2] = [
        TextBlock::new("second").into(),
        InjectedBlock::new("reminder", "strands:context-injector").into(),
    ];
    assert_eq!(agent.messages()[2].content(), expected);
}
