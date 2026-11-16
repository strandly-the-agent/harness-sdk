//! Ported from strands-ts/src/context-manager/strategies/__tests__/offload-strategy.test.ts

use std::collections::HashSet;
use std::sync::Arc;

use super::base::{OffloadBase, build_tool_name_map, collect_removable_with_pair, insert_marker};
use super::*;
use crate::experimental::context_manager::test_support::*;
use crate::experimental::context_manager::{ContextStrategy, PreviewMode, TruncateConfig};
use crate::types::messages::{ContentBlock, InjectedBlock, Message, Role, TextBlock, ToolResultContent};
use crate::types::tools::ToolResultStatus;

fn conditions(
    threshold: Option<f64>,
    utilization: Option<f64>,
    preserve_recent: Option<f64>,
) -> OffloadConditions {
    OffloadConditions {
        threshold,
        utilization,
        preserve_recent,
    }
}

fn truncate(target: OffloadTarget) -> TruncateStrategy {
    Offload::truncate(target, TruncateConfig::default()).unwrap()
}

fn preview(tokens: u64) -> TruncateConfig {
    TruncateConfig {
        preview_tokens: Some(tokens),
        preview: None,
    }
}

async fn apply(
    strategy: &dyn ContextStrategy,
    messages: Vec<Message>,
    utilization: f64,
    overflow: bool,
) -> (bool, Vec<Message>) {
    let mut agent = agent_with(messages);
    let acted = strategy
        .apply(&mut state(&mut agent, utilization, overflow))
        .await
        .unwrap();
    (acted, agent.messages().to_vec())
}

fn conversation(count: usize) -> Vec<Message> {
    (1..=count)
        .flat_map(|index| [user(&format!("q{index}")), assistant(&format!("a{index}"))])
        .collect()
}

fn texts(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .flat_map(Message::content)
        .map(|block| text_of(block).to_owned())
        .collect()
}

#[test]
fn creates_a_strategy_with_correct_name() {
    assert_eq!(truncate(OffloadTarget::ToolResults).name(), "offload:truncate");
}

#[test]
fn creates_a_strategy_with_when_conditions() {
    let strategy = Offload::truncate(OffloadTarget::ToolResults, preview(500))
        .unwrap()
        .when(conditions(Some(1000.0), None, None))
        .unwrap();
    assert_eq!(strategy.name(), "offload:truncate");
}

#[test]
fn rejects_a_threshold_at_or_below_the_preview_size() {
    let error = truncate(OffloadTarget::ToolResults)
        .when(conditions(Some(1000.0), None, None))
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "threshold (1000) must be greater than previewTokens (1000) to ensure truncation converges"
    );
}

#[tokio::test]
async fn truncates_large_tool_results() {
    let (acted, messages) = apply(
        &truncate(OffloadTarget::ToolResults),
        vec![tool_result("tool-123", &large_text())],
        0.5,
        false,
    )
    .await;
    assert!(acted);
    assert!(first_text(&messages[0]).contains("[Truncated:"));
}

#[tokio::test]
async fn does_not_truncate_results_below_threshold() {
    let strategy = truncate(OffloadTarget::ToolResults)
        .when(conditions(Some(2500.0), None, None))
        .unwrap();
    let (acted, _) = apply(
        &strategy,
        vec![tool_result("tool-123", "short result")],
        0.5,
        false,
    )
    .await;
    assert!(!acted);
}

#[tokio::test]
async fn skips_error_results_when_target_is_tool_results() {
    let message = tool_result_with("tool-err", ToolResultStatus::Error, &large_text());
    let (acted, _) = apply(&truncate(OffloadTarget::ToolResults), vec![message], 0.5, false).await;
    assert!(!acted);
}

#[tokio::test]
async fn truncates_error_results_when_target_is_tool_result_errors() {
    let message = tool_result_with("tool-err", ToolResultStatus::Error, &large_text());
    let (acted, _) = apply(
        &truncate(OffloadTarget::ToolResultErrors),
        vec![message],
        0.5,
        false,
    )
    .await;
    assert!(acted);
}

#[tokio::test]
async fn skips_non_user_messages_when_targeting_tool_results() {
    let (acted, _) = apply(
        &truncate(OffloadTarget::ToolResults),
        vec![assistant(&large_text())],
        0.5,
        false,
    )
    .await;
    assert!(!acted);
}

#[tokio::test]
async fn truncates_assistant_text_blocks_with_assistant_text_target() {
    let (acted, messages) = apply(
        &truncate(OffloadTarget::AssistantText),
        vec![assistant(&large_text())],
        0.5,
        false,
    )
    .await;
    assert!(acted);
    assert!(first_text(&messages[0]).contains("[Truncated:"));
}

#[tokio::test]
async fn truncates_user_text_blocks_with_user_text_target() {
    let (acted, messages) = apply(
        &truncate(OffloadTarget::UserText),
        vec![user(&large_text())],
        0.5,
        false,
    )
    .await;
    assert!(acted);
    assert!(first_text(&messages[0]).contains("[Truncated:"));
}

#[tokio::test]
async fn does_not_truncate_user_messages_with_assistant_text_target() {
    let (acted, _) = apply(
        &truncate(OffloadTarget::AssistantText),
        vec![user(&large_text())],
        0.5,
        false,
    )
    .await;
    assert!(!acted);
}

#[tokio::test]
async fn builds_head_tail_preview() {
    let text = format!(
        "{}{}{}",
        "HEAD".repeat(1000),
        "MIDDLE".repeat(5000),
        "TAIL".repeat(1000)
    );
    let strategy = Offload::truncate(OffloadTarget::ToolResults, preview(100)).unwrap();
    let (_, messages) = apply(&strategy, vec![tool_result("tool-123", &text)], 0.5, false).await;
    let preview = first_text(&messages[0]);
    assert!(preview.contains("[..."));
    assert!(preview.contains("chars elided"));
    assert!(preview.contains("HEADHEAD"));
    assert!(preview.ends_with("TAILTAIL"));
}

#[tokio::test]
async fn formats_preview_counts_the_same_regardless_of_the_host_locale() {
    let strategy = Offload::truncate(OffloadTarget::ToolResults, preview(100)).unwrap();
    let (_, messages) = apply(
        &strategy,
        vec![tool_result("tool-123", &"x".repeat(1_000_000))],
        0.5,
        false,
    )
    .await;
    let preview = first_text(&messages[0]);
    assert!(preview.contains("~250,000 tokens]"));
    assert!(preview.contains("[... 999,600 chars elided ...]"));
}

#[tokio::test]
async fn head_and_tail_previews_keep_one_end() {
    let text = format!("{}{}", "A".repeat(5000), "B".repeat(5000));
    for (mode, kept, dropped) in [(PreviewMode::Head, 'A', 'B'), (PreviewMode::Tail, 'B', 'A')] {
        let config = TruncateConfig {
            preview_tokens: Some(100),
            preview: Some(mode),
        };
        let strategy = Offload::truncate(OffloadTarget::ToolResults, config).unwrap();
        let (_, messages) = apply(&strategy, vec![tool_result("t", &text)], 0.5, false).await;
        let body = first_text(&messages[0]).split_once("]\n\n").unwrap().1.to_owned();
        assert!(body.contains(kept) && !body.contains(dropped), "{mode:?}");
    }
}

#[tokio::test]
async fn returns_false_for_empty_messages() {
    let (acted, _) = apply(&truncate(OffloadTarget::ToolResults), Vec::new(), 0.5, false).await;
    assert!(!acted);
}

#[test]
fn offload_drop_creates_a_drop_strategy() {
    assert_eq!(
        Offload::drop(OffloadTarget::ToolResults).unwrap().name(),
        "offload:drop"
    );
}

#[tokio::test]
async fn offload_drop_drops_tool_result_content_from_context_window_entirely() {
    let strategy = Offload::drop(OffloadTarget::ToolResults).unwrap();
    let (acted, messages) = apply(
        &strategy,
        vec![tool_result("tool-123", "some result")],
        0.5,
        false,
    )
    .await;
    assert!(acted);
    assert_eq!(first_text(&messages[0]), DROPPED_MARKER);
}

#[tokio::test]
async fn offload_drop_drops_assistant_text_blocks() {
    let strategy = Offload::drop(OffloadTarget::AssistantText).unwrap();
    let (acted, messages) = apply(&strategy, vec![assistant("some text")], 0.5, false).await;
    assert!(acted);
    assert_eq!(first_text(&messages[0]), "[Dropped]");
}

#[tokio::test]
async fn offload_drop_drops_user_text_blocks() {
    let strategy = Offload::drop(OffloadTarget::UserText).unwrap();
    let (acted, messages) = apply(&strategy, vec![user("some text")], 0.5, false).await;
    assert!(acted);
    assert_eq!(first_text(&messages[0]), "[Dropped]");
}

#[tokio::test]
async fn offload_truncate_with_string_target_matches_tool_by_name_from_assistant_message() {
    let strategy = truncate(OffloadTarget::Tools(vec!["tool::bash".into()]));
    let messages = vec![
        tool_use("tool-bash", "bash"),
        tool_result("tool-bash", &large_text()),
    ];
    let (acted, messages) = apply(&strategy, messages, 0.5, false).await;
    assert!(acted);
    assert!(first_text(&messages[1]).contains("[Truncated:"));
}

#[tokio::test]
async fn offload_truncate_with_string_target_skips_non_matching_tools() {
    let strategy = truncate(OffloadTarget::Tools(vec!["tool::bash".into()]));
    let messages = vec![
        tool_use("tool-read", "read_file"),
        tool_result("tool-read", &large_text()),
    ];
    let (acted, _) = apply(&strategy, messages, 0.5, false).await;
    assert!(!acted);
}

#[tokio::test]
async fn an_exclude_target_matches_every_other_tool() {
    let strategy = truncate(OffloadTarget::Tools(vec!["!bash".into()]));
    let messages = vec![
        tool_use("tool-bash", "bash"),
        tool_result("tool-bash", &large_text()),
        tool_use("tool-read", "read_file"),
        tool_result("tool-read", &large_text()),
    ];
    let (_, messages) = apply(&strategy, messages, 0.5, false).await;
    assert!(!first_text(&messages[1]).contains("[Truncated:"));
    assert!(first_text(&messages[3]).contains("[Truncated:"));
}

#[test]
fn offload_summarize_creates_a_summarize_strategy() {
    let strategy = Offload::summarize(OffloadTarget::ToolResults, SummarizeConfig::default()).unwrap();
    assert_eq!(strategy.name(), "offload:summarize");
}

#[test]
fn offload_summarize_when_creates_a_strategy_with_conditions() {
    let strategy = Offload::summarize(OffloadTarget::All, SummarizeConfig::default())
        .unwrap()
        .when(conditions(None, Some(0.8), Some(4.0)))
        .unwrap();
    assert_eq!(strategy.name(), "offload:summarize");
}

#[test]
fn throws_on_empty_array_target() {
    let error = Offload::drop(OffloadTarget::Tools(Vec::new())).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Empty array target matches nothing — provide at least one target"
    );
    assert!(Offload::truncate(OffloadTarget::Tools(Vec::new()), TruncateConfig::default()).is_err());
    assert!(Offload::summarize(OffloadTarget::Tools(Vec::new()), SummarizeConfig::default()).is_err());
}

#[tokio::test]
async fn drop_assistant_text_repairs_alternation_after_removing_assistant_messages() {
    let strategy = Offload::drop(OffloadTarget::AssistantText)
        .unwrap()
        .when(conditions(None, Some(0.5), None))
        .unwrap();
    let (acted, messages) = apply(&strategy, conversation(3), 0.6, false).await;
    assert!(acted);
    assert!(alternates(&messages));
}

#[tokio::test]
async fn drop_user_text_repairs_alternation_after_removing_user_messages() {
    let strategy = Offload::drop(OffloadTarget::UserText)
        .unwrap()
        .when(conditions(None, Some(0.5), None))
        .unwrap();
    let (acted, messages) = apply(&strategy, conversation(3), 0.6, false).await;
    assert!(acted);
    assert!(alternates(&messages));
}

#[tokio::test]
async fn drop_message_level_repairs_alternation() {
    let strategy = Offload::drop(OffloadTarget::All)
        .unwrap()
        .when(conditions(None, Some(0.5), None))
        .unwrap();
    let (acted, messages) = apply(&strategy, conversation(3), 0.6, false).await;
    assert!(acted);
    assert!(alternates(&messages));
}

#[tokio::test]
async fn drop_assistant_text_message_level_with_tool_pairs_preserves_tool_pair_integrity() {
    let strategy = Offload::drop(OffloadTarget::AssistantText)
        .unwrap()
        .when(conditions(None, Some(0.5), None))
        .unwrap();
    let messages = vec![
        user("q1"),
        assistant("a1"),
        user("q2"),
        tool_use("tu-1", "bash"),
        tool_result("tu-1", "result"),
        assistant("a2"),
        user("q3"),
        assistant("a3"),
    ];
    let (_, messages) = apply(&strategy, messages, 0.6, false).await;
    assert!(alternates(&messages));
    let mut uses = HashSet::new();
    let mut results = HashSet::new();
    for block in messages.iter().flat_map(Message::content) {
        match block {
            ContentBlock::ToolUse(tool_use) => uses.insert(tool_use.tool_use_id().to_owned()),
            ContentBlock::ToolResult(result) => results.insert(result.tool_use_id().to_owned()),
            _ => false,
        };
    }
    assert!(results.is_subset(&uses));
}

#[tokio::test]
async fn drop_user_text_is_not_a_no_op_when_messages_0_is_the_only_front_candidate() {
    let strategy = Offload::drop(OffloadTarget::UserText)
        .unwrap()
        .when(conditions(None, Some(0.5), None))
        .unwrap();
    let messages = vec![user("q1"), assistant("a1"), user("q2"), assistant("a2")];
    let (acted, messages) = apply(&strategy, messages, 0.6, false).await;
    assert!(acted);
    assert!(messages.len() <= 4);
}

#[tokio::test]
async fn message_level_starts_with_user_message_after_operation() {
    let strategy = Offload::drop(OffloadTarget::AssistantText)
        .unwrap()
        .when(conditions(None, Some(0.5), None))
        .unwrap();
    let (_, messages) = apply(&strategy, conversation(3), 0.6, false).await;
    assert_eq!(messages[0].role(), Role::User);
}

#[tokio::test]
async fn offload_drop_drops_all_content() {
    let strategy = Offload::drop(OffloadTarget::All).unwrap();
    let messages = vec![
        assistant("assistant text"),
        user("user text"),
        tool_result("tool-1", "tool text"),
    ];
    let (acted, messages) = apply(&strategy, messages, 0.5, false).await;
    assert!(acted);
    assert_eq!(texts(&messages), ["[Dropped]", "[Dropped]", "[Dropped]"]);
}

#[tokio::test]
async fn offload_truncate_truncates_all_large_content() {
    let messages = vec![
        assistant(&large_text()),
        user(&large_text()),
        tool_result("tool-1", &large_text()),
    ];
    let (acted, messages) = apply(&truncate(OffloadTarget::All), messages, 0.5, false).await;
    assert!(acted);
    assert!(texts(&messages).iter().all(|text| text.contains("[Truncated:")));
}

#[tokio::test]
async fn offload_truncate_with_threshold_skips_small_content() {
    let strategy = truncate(OffloadTarget::All)
        .when(conditions(Some(2500.0), None, None))
        .unwrap();
    let (acted, _) = apply(&strategy, vec![assistant("small")], 0.5, false).await;
    assert!(!acted);
}

#[tokio::test]
async fn leaves_injected_text_in_place() {
    let injected: ContentBlock = InjectedBlock::new("x".repeat(2000), "memory").into();
    let message = Message::new(Role::User, vec![TextBlock::new("ask").into(), injected.clone()]);
    let strategy = Offload::truncate(OffloadTarget::All, preview(50))
        .unwrap()
        .when(conditions(Some(100.0), None, None))
        .unwrap();
    let (acted, messages) = apply(&strategy, vec![message], 0.5, false).await;
    assert!(!acted);
    assert_eq!(messages[0].content(), [TextBlock::new("ask").into(), injected]);
}

#[tokio::test]
async fn drop_leaves_dropped_n_messages_marker() {
    let strategy = Offload::drop(OffloadTarget::All)
        .unwrap()
        .when(conditions(None, Some(0.5), None))
        .unwrap();
    let (_, messages) = apply(&strategy, conversation(4), 0.6, false).await;
    let marker = texts(&messages)
        .into_iter()
        .find(|text| text.starts_with("[Dropped: "))
        .unwrap();
    assert!(
        marker.ends_with("messages]") || marker.ends_with("message]"),
        "{marker}"
    );
}

#[tokio::test]
async fn truncate_leaves_n_messages_elided_marker() {
    let strategy = truncate(OffloadTarget::All)
        .when(conditions(None, Some(0.5), None))
        .unwrap();
    let (_, messages) = apply(&strategy, conversation(4), 0.6, false).await;
    let marker = texts(&messages)
        .into_iter()
        .find(|text| text.starts_with("[... "))
        .unwrap();
    assert!(marker.ends_with(" elided ...]"), "{marker}");
}

#[tokio::test]
async fn message_level_drop_skips_pinned_messages() {
    let strategy = Offload::drop(OffloadTarget::All)
        .unwrap()
        .when(conditions(None, Some(0.5), None))
        .unwrap();
    let messages = vec![
        user("q1"),
        assistant("a1"),
        pinned(user("pinned-msg")),
        assistant("a2"),
        user("q3"),
        assistant("a3"),
    ];
    let (_, messages) = apply(&strategy, messages, 0.6, false).await;
    assert!(texts(&messages).contains(&"pinned-msg".to_owned()));
}

#[tokio::test]
async fn message_level_drop_skips_tool_pair_partner_of_pinned_message() {
    let strategy = Offload::drop(OffloadTarget::All)
        .unwrap()
        .when(conditions(None, Some(0.5), None))
        .unwrap();
    let messages = vec![
        user("q1"),
        pinned(tool_use("tu-1", "bash")),
        tool_result("tu-1", "result"),
        assistant("a2"),
        user("q3"),
        assistant("a3"),
    ];
    let (_, messages) = apply(&strategy, messages, 0.6, false).await;
    let has_result = messages
        .iter()
        .flat_map(Message::content)
        .any(|block| matches!(block, ContentBlock::ToolResult(_)));
    assert!(has_result);
}

#[tokio::test]
async fn pinned_message_survives_repair_alternation_merge_across_two_passes() {
    let strategy = Offload::drop(OffloadTarget::All)
        .unwrap()
        .when(conditions(None, Some(0.5), Some(4.0)))
        .unwrap();
    let messages = vec![
        user("q1"),
        assistant("a1"),
        user("q2"),
        pinned(assistant("pinned-a3")),
        user("q4"),
        assistant("a5"),
        user("q6"),
        assistant("a7"),
        user("q8"),
        assistant("a9"),
        user("q10"),
    ];
    let (_, messages) = apply(&strategy, messages, 0.6, false).await;
    let (_, messages) = apply(&strategy, messages, 0.6, false).await;
    let merged = messages
        .iter()
        .find(|message| {
            message
                .content()
                .iter()
                .any(|block| text_of(block) == "pinned-a3")
        })
        .unwrap();
    let flag = merged
        .metadata()
        .and_then(|metadata| metadata.custom())
        .and_then(|custom| custom.get("pinned"));
    assert_eq!(flag, Some(&serde_json::json!(true)));
}

#[tokio::test]
async fn message_level_strategy_fires_on_low_utilization_when_overflow_is_set() {
    let strategy = Offload::drop(OffloadTarget::All)
        .unwrap()
        .when(conditions(None, Some(0.85), None))
        .unwrap();
    let (acted, messages) = apply(&strategy, conversation(2), 0.1, true).await;
    assert!(acted);
    assert!(messages.len() < 4);
}

#[tokio::test]
async fn message_level_strategy_skips_on_low_utilization_without_overflow() {
    let strategy = Offload::drop(OffloadTarget::All)
        .unwrap()
        .when(conditions(None, Some(0.85), None))
        .unwrap();
    let (acted, messages) = apply(&strategy, conversation(2), 0.1, false).await;
    assert!(!acted);
    assert_eq!(messages.len(), 4);
}

fn alternating(count: usize) -> Vec<Message> {
    (0..count)
        .map(|index| {
            if index % 2 == 0 {
                user(&format!("msg-{index}"))
            } else {
                assistant(&format!("msg-{index}"))
            }
        })
        .collect()
}

fn oldest(messages: &[Message], preserve_recent: f64) -> usize {
    let base = OffloadBase::new(OffloadTarget::All, conditions(None, None, Some(preserve_recent))).unwrap();
    base.oldest_matches(messages, &build_tool_name_map(messages))
        .len()
}

#[test]
fn integer_preserve_recent_keeps_absolute_count() {
    assert_eq!(oldest(&alternating(10), 3.0), 7);
}

#[test]
fn decimal_preserve_recent_keeps_ratio_of_matches() {
    assert_eq!(oldest(&alternating(10), 0.7), 3);
}

#[test]
fn decimal_preserve_recent_rounds_up() {
    assert_eq!(oldest(&alternating(3), 0.5), 1);
}

#[test]
fn decimal_preserve_recent_of_0_99_preserves_almost_all() {
    assert_eq!(oldest(&alternating(10), 0.99), 0);
}

fn tool_pair(id: &str) -> [Message; 2] {
    [tool_use(id, "read"), tool_result(id, "x")]
}

#[test]
fn collects_a_tool_use_with_its_following_tool_result() {
    let [call, result] = tool_pair("tu-1");
    let messages = vec![user("task"), call, result, assistant("latest")];
    assert_eq!(collect_removable_with_pair(&messages, 1), [1, 2]);
}

#[test]
fn never_collects_the_last_message() {
    let messages = vec![user("task"), assistant("a1"), user("latest")];
    assert!(collect_removable_with_pair(&messages, 2).is_empty());
}

#[test]
fn skips_a_tool_use_whose_result_is_the_last_message() {
    let [call, result] = tool_pair("tu-1");
    let messages = vec![user("task"), call, result];
    assert!(collect_removable_with_pair(&messages, 1).is_empty());
}

#[test]
fn inserts_an_assistant_message_between_user_messages() {
    let mut messages = vec![user("task"), user("u1")];
    insert_marker(&mut messages, 1, "marker");
    let roles: Vec<Role> = messages.iter().map(Message::role).collect();
    assert_eq!(roles, [Role::User, Role::Assistant, Role::User]);
    assert_eq!(messages[1].content(), [TextBlock::new("marker").into()]);
}

#[test]
fn appends_to_the_assistant_message_before_the_gap() {
    let custom = crate::types::json::JsonObject::from_iter([("k".to_owned(), serde_json::json!("v"))]);
    let original = assistant("a1")
        .with_tracking_id("msg-1")
        .with_metadata(crate::types::messages::MessageMetadata::new().with_custom(custom.clone()));
    let mut messages = vec![user("task"), original, user("u1")];
    insert_marker(&mut messages, 2, "marker");
    assert_eq!(messages.len(), 3);
    assert_eq!(texts(&messages[1..2]), ["a1", "marker"]);
    assert_eq!(messages[1].tracking_id(), "msg-1");
    assert_eq!(
        messages[1].metadata().and_then(|metadata| metadata.custom()),
        Some(&custom)
    );
}

#[test]
fn appends_to_the_assistant_message_after_the_gap() {
    let mut messages = vec![user("task"), assistant("a1"), user("u1")];
    insert_marker(&mut messages, 1, "marker");
    assert_eq!(messages.len(), 3);
    assert_eq!(texts(&messages[1..2]), ["a1", "marker"]);
}

#[test]
fn clamps_index_into_the_history_after_message_0() {
    for (index, expected) in [(0, ["task", "marker", "u1"]), (5, ["task", "u1", "marker"])] {
        let mut messages = vec![user("task"), user("u1")];
        insert_marker(&mut messages, index, "marker");
        assert_eq!(texts(&messages), expected);
    }
}

#[tokio::test]
async fn leaves_a_marker_on_the_assistant_message_after_the_cut() {
    let messages = vec![
        user("pin"),
        assistant("a1"),
        user("u2"),
        assistant("a2"),
        user("u3"),
    ];
    let (acted, messages) = apply(&EmergencyTruncateStrategy::new(), messages, 1.1, true).await;
    assert!(acted);
    let shape: Vec<(Role, Vec<String>)> = messages
        .iter()
        .map(|message| {
            (
                message.role(),
                message
                    .content()
                    .iter()
                    .map(|block| text_of(block).to_owned())
                    .collect(),
            )
        })
        .collect();
    assert_eq!(
        shape,
        [
            (Role::User, vec!["pin".to_owned()]),
            (
                Role::Assistant,
                vec!["a2".to_owned(), "[... 2 messages elided ...]".to_owned()]
            ),
            (Role::User, vec!["u3".to_owned()]),
        ]
    );
}

fn repeated_strategies() -> Vec<(&'static str, Arc<dyn ContextStrategy>)> {
    let message_level = |preserve_recent| conditions(None, Some(0.7), preserve_recent);
    vec![
        (
            "drop",
            Arc::new(
                Offload::drop(OffloadTarget::All)
                    .unwrap()
                    .when(message_level(Some(3.0)))
                    .unwrap(),
            ),
        ),
        (
            "drop-all",
            Arc::new(
                Offload::drop(OffloadTarget::All)
                    .unwrap()
                    .when(message_level(None))
                    .unwrap(),
            ),
        ),
        (
            "truncate",
            Arc::new(
                truncate(OffloadTarget::All)
                    .when(message_level(Some(3.0)))
                    .unwrap(),
            ),
        ),
        (
            "truncate-all",
            Arc::new(truncate(OffloadTarget::All).when(message_level(None)).unwrap()),
        ),
        ("emergency", Arc::new(EmergencyTruncateStrategy::new())),
    ]
}

fn turn(index: usize, text_only: bool) -> Vec<Message> {
    if text_only {
        return vec![assistant(&format!("a{index}")), user(&format!("u{index}"))];
    }
    tool_pair(&format!("tu-{index}")).to_vec()
}

#[tokio::test]
async fn keeps_message_0_verbatim_when_text_only() {
    for text_only in [false, true] {
        for (name, strategy) in repeated_strategies() {
            let task = pinned(user("task"));
            let mut agent = agent_with(vec![task.clone()]);
            for index in 0..30 {
                let added = turn(index, text_only);
                let last = added[added.len() - 1].clone();
                agent.messages_mut().extend(added);
                strategy.apply(&mut state(&mut agent, 1.1, true)).await.unwrap();
                assert_eq!(
                    agent.messages().last(),
                    Some(&last),
                    "{name} textOnly={text_only}"
                );
            }
            assert_eq!(agent.messages()[0].content(), task.content(), "{name}");
            assert_eq!(agent.messages()[0].metadata(), task.metadata(), "{name}");
            assert!(
                agent.messages().len() < 10,
                "{name} textOnly={text_only}: {}",
                agent.messages().len()
            );
        }
    }
}

#[test]
fn tool_result_json_items_truncate_as_their_json() {
    let block = crate::types::messages::ToolResultBlock::new(
        "t",
        ToolResultStatus::Success,
        vec![ToolResultContent::Json(crate::types::messages::JsonBlock::new(
            serde_json::json!({"k": "v".repeat(5000)}),
        ))],
    );
    let truncated = crate::experimental::context_manager::methods::truncate::truncate_tool_result_block(
        &block,
        &preview(100),
    )
    .unwrap();
    let ToolResultContent::Text(text) = &truncated.content()[0] else {
        panic!("expected text");
    };
    assert!(text.text().contains("{\"json\":{\"k\":\"vvv"));
}
