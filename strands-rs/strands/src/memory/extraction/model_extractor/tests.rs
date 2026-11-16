//! Ports `memory/extraction/__tests__/model-extractor.test.ts`.

use std::num::NonZeroU32;

use super::*;
use crate::agent::InvocationContext;
use crate::models::streaming::Usage;
use crate::testing::{MockMessageModel, MockTurnOptions};
use crate::types::agent::InvokeLimits;
use crate::types::messages::SystemPrompt;

fn user_turn(text: &str) -> Message {
    Message::new(Role::User, vec![TextBlock::new(text).into()])
}

fn replying(text: &str) -> Arc<MockMessageModel> {
    let model = MockMessageModel::new();
    model.add_turn(vec![TextBlock::new(text).into()]);
    Arc::new(model)
}

fn extractor_for(model: &Arc<MockMessageModel>) -> ModelExtractor {
    ModelExtractor::new(ModelExtractorOptions {
        model: Some(model.clone()),
        system_prompt: None,
    })
}

async fn extract(model: &Arc<MockMessageModel>, text: &str) -> Vec<ExtractionResult> {
    extractor_for(model)
        .extract(&[user_turn(text)], &ExtractorContext::default())
        .await
        .unwrap()
}

fn system_prompt(model: &MockMessageModel) -> String {
    match model.requests()[0].system_prompt.clone().unwrap() {
        SystemPrompt::Text(text) => text,
        other => panic!("expected a text system prompt, got {other:?}"),
    }
}

#[tokio::test]
async fn parses_a_json_array_of_entries_from_the_model_response() {
    let model = replying(r#"[{"content": "User prefers dark mode"}, {"content": "Lives in Berlin"}]"#);
    assert_eq!(
        extract(&model, "I like dark mode and live in Berlin").await,
        [
            ExtractionResult::new("User prefers dark mode"),
            ExtractionResult::new("Lives in Berlin")
        ]
    );
    assert_eq!(
        serde_json::to_value(&model.requests()[0].messages[0].content()).unwrap(),
        serde_json::json!([{
            "text": "Extract facts from the following transcript:\n\nuser: I like dark mode and live in Berlin",
        }])
    );
}

#[tokio::test]
async fn extracts_a_json_array_even_when_wrapped_in_prose_or_a_code_fence() {
    let model = replying("Here are the facts:\n```json\n[{\"content\": \"fact\"}]\n```\nHope that helps.");
    assert_eq!(
        extract(&model, "something").await,
        [ExtractionResult::new("fact")]
    );
}

#[tokio::test]
async fn preserves_entry_metadata() {
    let model = replying(r#"[{"content": "fact", "metadata": {"topic": "pref"}}]"#);
    let metadata = serde_json::from_value(serde_json::json!({ "topic": "pref" })).unwrap();
    assert_eq!(
        extract(&model, "x").await,
        [ExtractionResult::new("fact").with_metadata(metadata)]
    );
}

#[tokio::test]
async fn returns_no_entries_on_malformed_json_without_throwing() {
    let model = replying("not json at all");
    assert!(extract(&model, "x").await.is_empty());
}

#[tokio::test]
async fn drops_entries_without_a_string_content_and_empty_strings() {
    let model = replying(r#"[{"content": "keep"}, {"content": ""}, {"foo": "bar"}, "loose"]"#);
    assert_eq!(extract(&model, "x").await, [ExtractionResult::new("keep")]);
}

#[tokio::test]
async fn returns_empty_for_an_empty_message_batch_without_calling_the_model() {
    let model = Arc::new(MockMessageModel::new());
    let entries = extractor_for(&model)
        .extract(&[], &ExtractorContext::default())
        .await
        .unwrap();
    assert!(entries.is_empty());
    assert!(model.requests().is_empty());
}

#[tokio::test]
async fn appends_the_output_format_to_the_default_system_prompt() {
    let model = replying("[]");
    extract(&model, "x").await;
    assert_eq!(
        system_prompt(&model),
        "You extract durable facts worth remembering across future conversations from a transcript.\n\
         Each entry is one discrete, self-contained fact (a preference, decision, or stable detail about the user or task). Do not include transient chit-chat, questions, or anything already obvious.\n\n\
         Return ONLY a JSON array of objects, each: {\"content\": string}. If there is nothing worth remembering, return []."
    );
}

#[tokio::test]
async fn keeps_the_output_format_when_a_custom_system_prompt_replaces_the_default() {
    let model = replying(r#"[{"content": "Prefers aisle seats"}]"#);
    let extractor = ModelExtractor::new(ModelExtractorOptions {
        model: Some(model.clone()),
        system_prompt: Some("Extract user preferences.".to_owned()),
    });
    let entries = extractor
        .extract(&[user_turn("I like aisle seats")], &ExtractorContext::default())
        .await
        .unwrap();
    let system_prompt = system_prompt(&model);
    assert!(system_prompt.starts_with("Extract user preferences.\n\n"));
    assert!(!system_prompt.contains("You extract durable facts"));
    assert!(system_prompt.contains("Return ONLY a JSON array of objects, each: {\"content\": string}."));
    assert_eq!(entries, [ExtractionResult::new("Prefers aisle seats")]);
}

#[tokio::test]
async fn falls_back_to_the_default_model_from_context_when_none_configured() {
    let model = replying(r#"[{"content": "fact"}]"#);
    let entries = ModelExtractor::default()
        .extract(&[user_turn("x")], &ExtractorContext::new(Some(model)))
        .await
        .unwrap();
    assert_eq!(entries, [ExtractionResult::new("fact")]);
}

#[tokio::test]
async fn throws_when_no_model_is_configured_and_no_default_is_provided() {
    let error = ModelExtractor::default()
        .extract(&[user_turn("x")], &ExtractorContext::default())
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "ModelExtractor: no model configured and no default model available"
    );
}

#[tokio::test]
async fn extracts_without_a_tracer_in_context_standalone_use() {
    let model = replying(r#"[{"content": "fact"}]"#);
    assert_eq!(extract(&model, "x").await, [ExtractionResult::new("fact")]);
}

/// The span half of TS "ends the model-invoke span with an error when the model throws" waits
/// for telemetry; the error still propagates.
#[tokio::test]
async fn propagates_the_error_when_the_model_throws() {
    let model = Arc::new(MockMessageModel::new());
    model.add_error(|| Error::other("model down"));
    let error = extractor_for(&model)
        .extract(&[user_turn("x")], &ExtractorContext::default())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "model down");
}

// request limits

#[tokio::test]
async fn adds_the_extraction_usage_to_the_request_total_even_when_the_request_limit_is_used_up() {
    let model = MockMessageModel::new();
    model.add_turn_with(
        vec![TextBlock::new(r#"[{"content": "fact"}]"#).into()],
        MockTurnOptions {
            usage: Some(Usage::new(4, 5, 9)),
            ..Default::default()
        },
    );
    let extractor = extractor_for(&Arc::new(model));
    let invocation_context = InvocationContext::new(
        Some(InvokeLimits {
            turns: NonZeroU32::new(1),
            ..Default::default()
        }),
        None,
        false,
    );
    invocation_context.set_turns_taken(5);
    let context = ExtractorContext {
        default_model: None,
        invocation_context: Some(invocation_context.clone()),
    };

    let entries = extractor.extract(&[user_turn("x")], &context).await.unwrap();

    assert_eq!(entries, [ExtractionResult::new("fact")]);
    let usage = invocation_context.usage();
    assert_eq!(usage.turns_taken(), 5);
    assert_eq!(usage.tokens(), &Usage::new(4, 5, 9));
}
