//! `countTokens` cases of strands-ts/src/models/__tests__/model.test.ts.

use serde_json::json;

use super::provider;
use crate::models::model::{CountTokensOptions, Model};
use crate::testing::TestModelProvider;
use crate::types::citations::{CitationGeneratedContent, CitationsBlock};
use crate::types::messages::{
    ContentBlock, GuardContentBlock, GuardContentText, GuardQualifier, InjectedBlock, Message,
    ReasoningBlock, Role, SummaryBlock, SystemContentBlock, SystemPrompt, TextBlock, ToolResultBlock,
    ToolUseBlock,
};
use crate::types::tools::{ToolResultStatus, ToolSpec};

fn test_provider() -> TestModelProvider {
    provider(json!([]))
}

fn message(role: Role, block: impl Into<ContentBlock>) -> Message {
    Message::new(role, vec![block.into()])
}

fn ceil4(text: &str) -> u64 {
    u64::try_from(text.encode_utf16().count()).unwrap().div_ceil(4)
}

fn guard(text: &str) -> GuardContentBlock {
    GuardContentText::new(text, vec![GuardQualifier::Query]).into()
}

async fn count(messages: &[Message]) -> u64 {
    test_provider()
        .count_tokens(messages, CountTokensOptions::default())
        .await
        .unwrap()
}

async fn count_with(messages: &[Message], options: CountTokensOptions) -> u64 {
    test_provider().count_tokens(messages, options).await.unwrap()
}

#[tokio::test]
async fn counts_summary_text_like_a_text_block() {
    let text = "x".repeat(400);
    assert_eq!(
        count(&[message(Role::Assistant, SummaryBlock::new(&text))]).await,
        count(&[message(Role::Assistant, TextBlock::new(&text))]).await
    );
}

#[tokio::test]
async fn estimates_text_block_tokens_using_chars_4_heuristic() {
    assert_eq!(
        count(&[message(Role::User, TextBlock::new("Hello world"))]).await,
        3
    );
}

#[tokio::test]
async fn estimates_tool_use_block_tokens_name_json_input() {
    let tool_use = ToolUseBlock::new("get_weather", "id1", json!({ "city": "Seattle" }));
    assert_eq!(count(&[message(Role::Assistant, tool_use)]).await, 3 + 9);
}

#[tokio::test]
async fn estimates_tool_result_block_tokens_text_items_only() {
    let result = ToolResultBlock::new("id1", ToolResultStatus::Success, vec!["72°F and sunny".into()]);
    assert_eq!(
        count(&[message(Role::User, result)]).await,
        ceil4("72°F and sunny")
    );
}

#[tokio::test]
async fn counts_json_tool_result_items_as_json() {
    let item = crate::types::messages::JsonBlock::new(json!({ "a": 1 }));
    let result = ToolResultBlock::new("id1", ToolResultStatus::Success, vec![item.into()]);
    assert_eq!(count(&[message(Role::User, result)]).await, 4);
}

#[tokio::test]
async fn estimates_reasoning_block_tokens() {
    let text = "Let me think about this step by step";
    assert_eq!(
        count(&[message(Role::Assistant, ReasoningBlock::new().with_text(text))]).await,
        ceil4(text)
    );
}

#[tokio::test]
async fn estimates_guard_content_block_tokens() {
    assert_eq!(
        count(&[message(Role::User, guard("Is this safe?"))]).await,
        ceil4("Is this safe?")
    );
}

#[tokio::test]
async fn estimates_citations_block_tokens() {
    let block = CitationsBlock::new(Vec::new(), vec![CitationGeneratedContent::new("cited text here")]);
    assert_eq!(
        count(&[message(Role::Assistant, block)]).await,
        ceil4("cited text here")
    );
}

#[tokio::test]
async fn estimates_injected_block_tokens() {
    let text = "cwd: /repo, date: today";
    assert_eq!(
        count(&[message(Role::User, InjectedBlock::new(text, "test"))]).await,
        ceil4(text)
    );
}

#[tokio::test]
async fn counts_only_the_injected_blocks_of_the_latest_user_message() {
    let messages = [
        message(Role::User, InjectedBlock::new("x".repeat(400), "test")),
        message(Role::Assistant, TextBlock::new("ok")),
        message(Role::User, InjectedBlock::new("cwd: /repo", "test")),
    ];
    assert_eq!(count(&messages).await, ceil4("ok") + ceil4("cwd: /repo"));
}

#[tokio::test]
async fn estimates_string_system_prompt_tokens() {
    let options = CountTokensOptions::builder()
        .system_prompt("You are a helpful assistant")
        .build();
    assert_eq!(
        count_with(&[message(Role::User, TextBlock::new("Hi"))], options).await,
        ceil4("You are a helpful assistant") + ceil4("Hi")
    );
}

#[tokio::test]
async fn estimates_array_system_prompt_tokens() {
    let options = CountTokensOptions::builder()
        .system_prompt(SystemPrompt::Blocks(vec![SystemContentBlock::text(
            "System instructions",
        )]))
        .build();
    assert_eq!(
        count_with(&[message(Role::User, TextBlock::new("Hi"))], options).await,
        ceil4("System instructions") + ceil4("Hi")
    );
}

#[tokio::test]
async fn estimates_tool_spec_tokens() {
    let spec = ToolSpec::new("get_weather", "Get weather for a city");
    let spec_json = r#"{"name":"get_weather","description":"Get weather for a city"}"#;
    assert_eq!(crate::types::json::to_string(&spec).unwrap(), spec_json);
    let options = CountTokensOptions::builder().tool_specs(vec![spec]).build();
    assert_eq!(
        count_with(&[message(Role::User, TextBlock::new("Hi"))], options).await,
        ceil4("Hi") + u64::try_from(spec_json.len()).unwrap().div_ceil(2)
    );
}

#[tokio::test]
async fn returns_0_for_empty_messages() {
    assert_eq!(count(&[]).await, 0);
}

#[tokio::test]
async fn skips_reasoning_blocks_without_text() {
    assert_eq!(
        count(&[message(
            Role::Assistant,
            ReasoningBlock::new().with_signature("sig123")
        )])
        .await,
        0
    );
}

#[tokio::test]
async fn estimates_guard_content_in_array_system_prompt() {
    let options = CountTokensOptions::builder()
        .system_prompt(SystemPrompt::Blocks(vec![guard("Guard text here").into()]))
        .build();
    assert_eq!(
        count_with(&[message(Role::User, TextBlock::new("Hi"))], options).await,
        ceil4("Guard text here") + ceil4("Hi")
    );
}

#[tokio::test]
async fn accumulates_tokens_across_multiple_messages_with_mixed_content() {
    let messages = [
        message(Role::User, TextBlock::new("What is the weather?")),
        message(
            Role::Assistant,
            ToolUseBlock::new("get_weather", "id1", json!({ "city": "Seattle" })),
        ),
        message(
            Role::User,
            ToolResultBlock::new("id1", ToolResultStatus::Success, vec!["72F".into()]),
        ),
    ];
    let options = CountTokensOptions::builder()
        .system_prompt("You are helpful")
        .build();
    let expected = ceil4("You are helpful")
        + ceil4("What is the weather?")
        + ceil4("get_weather")
        + u64::try_from(r#"{"city":"Seattle"}"#.len()).unwrap().div_ceil(2)
        + ceil4("72F");
    assert_eq!(count_with(&messages, options).await, expected);
}

#[tokio::test]
async fn counts_utf16_code_units_like_typescript() {
    assert_eq!(count(&[message(Role::User, TextBlock::new("😀😀"))]).await, 1);
    assert_eq!(count(&[message(Role::User, TextBlock::new("😀😀😀"))]).await, 2);
}
