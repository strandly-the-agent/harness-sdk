//! Ports the `injected blocks` case of `agent/__tests__/agent.test.ts`.

use super::*;
use crate::types::messages::InjectedBlock;

#[tokio::test]
async fn sends_an_injected_block_to_the_model_as_text_and_keeps_it_in_agent_messages() {
    let model = mock(vec![vec![text("ok")]]);
    let mut agent = test_agent(model.clone());
    let input: Vec<ContentBlock> = vec![
        text("Refactor the tokenizer."),
        InjectedBlock::new("reminder", "test").into(),
    ];

    agent.invoke(input.clone()).await.unwrap();

    assert_eq!(
        serde_json::to_value(model.requests()[0].messages[0].content()).unwrap(),
        json!([{ "text": "Refactor the tokenizer." }, { "text": "\n\nreminder" }])
    );
    assert_eq!(agent.messages()[0].content(), input);
}
