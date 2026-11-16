//! Ported from strands-ts/src/conversation-manager/__tests__/sliding-window-conversation-manager.test.ts.
//!
//! TS drives the manager through its hooks with a mock agent; most cases here call the private
//! `reduce_context`/`apply_management` the hooks call, and the `hooks` cases cover the wiring.

use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::conversation_manager::ConversationManagerPlugin;
use crate::conversation_manager::compression::pin_message::pin_message;
use crate::hooks::{AfterInvocationEvent, AfterModelCallEvent, BeforeModelCallEvent};
use crate::models::model::Model;
use crate::plugins::Plugin;
use crate::testing::MockMessageModel;
use crate::types::agent::InvocationState;
use crate::types::media::S3Location;
use crate::types::messages::{JsonBlock, ToolUseBlock};
use crate::types::tools::ToolResultStatus;

fn u(text: &str) -> Message {
    Message::new(Role::User, vec![TextBlock::new(text).into()])
}

fn a(text: &str) -> Message {
    Message::new(Role::Assistant, vec![TextBlock::new(text).into()])
}

fn tool_use(name: &str, id: &str) -> Message {
    Message::new(
        Role::Assistant,
        vec![ToolUseBlock::new(name, id, json!({})).into()],
    )
}

fn result_block(id: &str, items: Vec<ToolResultContent>) -> ToolResultBlock {
    ToolResultBlock::new(id, ToolResultStatus::Success, items)
}

fn result_with(id: &str, items: Vec<ToolResultContent>) -> Message {
    Message::new(Role::User, vec![result_block(id, items).into()])
}

fn tool_result(id: &str, text: &str) -> Message {
    result_with(id, vec![TextBlock::new(text).into()])
}

fn tool_heavy_history() -> Vec<Message> {
    vec![
        u("Review this PR"),
        tool_use("getDiff", "id-1"),
        tool_result("id-1", "Diff"),
        tool_use("getFile", "id-2"),
        tool_result("id-2", "File"),
        tool_use("getTree", "id-3"),
        tool_result("id-3", "Tree"),
        a("Review complete"),
    ]
}

fn manager(window_size: usize, should_truncate_results: bool) -> SlidingWindowConversationManager {
    let config = SlidingWindowConversationManagerConfig::builder()
        .window_size(window_size)
        .should_truncate_results(should_truncate_results)
        .build();
    SlidingWindowConversationManager::new(config).unwrap()
}

fn window(window_size: usize) -> SlidingWindowConversationManager {
    manager(window_size, true)
}

/// TS `triggerContextOverflow`: the reduction the overflow hook runs; `true` sets `retry`.
fn overflow(manager: &SlidingWindowConversationManager, messages: &mut Vec<Message>) -> bool {
    manager.reduce_context(messages, true)
}

fn first_text(message: &Message) -> &str {
    match message.content().first() {
        Some(ContentBlock::Text(text)) => text.text(),
        other => panic!("expected a text block, got {other:?}"),
    }
}

fn first_block(message: &Message) -> &ContentBlock {
    message.content().first().unwrap()
}

fn roles(messages: &[Message]) -> Vec<Role> {
    messages.iter().map(Message::role).collect()
}

fn truncated(prefix: &str, removed: usize, suffix: &str) -> String {
    format!("{prefix}\n<truncated chars=\"{removed}\"/>\n{suffix}")
}

mod constructor {
    use super::*;

    #[test]
    fn sets_default_window_size_to_40() {
        assert_eq!(SlidingWindowConversationManager::default().window_size, 40);
        assert_eq!(SlidingWindowConversationManagerConfig::default().window_size, 40);
    }

    #[test]
    fn sets_default_should_truncate_results_to_true() {
        assert!(SlidingWindowConversationManager::default().should_truncate_results);
        assert!(SlidingWindowConversationManagerConfig::default().should_truncate_results);
    }

    #[test]
    fn accepts_custom_window_size() {
        assert_eq!(window(20).window_size, 20);
    }

    #[test]
    fn accepts_custom_should_truncate_results() {
        assert!(!manager(40, false).should_truncate_results);
    }
}

mod reduce {
    use super::*;

    #[test]
    fn returns_true_when_tool_results_are_truncated_even_though_message_count_is_unchanged() {
        let mut messages = vec![tool_result("tool-1", &"x".repeat(500))];
        assert!(overflow(&window(40), &mut messages));
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn returns_true_when_messages_are_trimmed() {
        let mut messages = vec![u("Message 1"), a("Response 1"), u("Message 2"), a("Response 2")];
        assert!(overflow(&manager(2, false), &mut messages));
        assert_eq!(messages.len(), 2);
    }
}

mod apply_management {
    use super::*;

    #[test]
    fn skips_reduction_when_message_count_is_less_than_window_size() {
        let mut messages = vec![u("Message 1"), a("Response 1")];
        window(10).apply_management(&mut messages);
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn skips_reduction_when_message_count_equals_window_size() {
        let mut messages = vec![u("Message 1"), a("Response 1")];
        window(2).apply_management(&mut messages);
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn removes_all_messages_when_window_size_is_0() {
        let mut messages = vec![u("Message 1"), a("Response 1")];
        window(0).apply_management(&mut messages);
        assert!(messages.is_empty());
    }

    #[test]
    fn calls_reduce_context_when_message_count_exceeds_window_size() {
        let mut messages = vec![u("Message 1"), a("Response 1"), u("Message 2"), a("Response 2")];
        window(2).apply_management(&mut messages);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role(), Role::User);
    }
}

mod tool_result_truncation {
    use super::*;

    #[test]
    fn partially_truncates_large_tool_results_preserving_first_and_last_200_chars() {
        let middle = "MIDDLE_CONTENT_TO_REMOVE".repeat(10);
        let original = format!("{}{middle}{}", "A".repeat(200), "B".repeat(200));
        let mut messages = vec![tool_result("tool-1", &original)];
        overflow(&window(40), &mut messages);
        let expected = truncated(&"A".repeat(200), middle.len(), &"B".repeat(200));
        assert_eq!(
            first_block(&messages[0]),
            &result_block("tool-1", vec![TextBlock::new(expected).into()]).into()
        );
    }

    #[test]
    fn preserves_the_durable_message_id_when_truncating_tool_results() {
        let original = format!(
            "{}{}{}",
            "A".repeat(200),
            "MIDDLE_CONTENT_TO_REMOVE".repeat(10),
            "B".repeat(200)
        );
        let mut messages = vec![tool_result("tool-1", &original).with_tracking_id("durable-1")];
        assert!(truncate_tool_results(&mut messages, 0));
        assert_eq!(messages[0].tracking_id(), "durable-1");
    }

    #[test]
    fn leaves_small_tool_results_unchanged() {
        let mut messages = vec![tool_result("tool-1", "Small result")];
        assert!(!truncate_tool_results(&mut messages, 0));
    }

    #[test]
    fn finds_oldest_message_with_tool_results() {
        let second = "S".repeat(500);
        let mut messages = vec![
            u("Message 1"),
            tool_result("tool-1", &"F".repeat(500)),
            a("Response"),
            tool_result("tool-2", &second),
        ];
        overflow(&window(40), &mut messages);
        let expected = truncated(&"F".repeat(200), 100, &"F".repeat(200));
        assert_eq!(
            first_block(&messages[1]),
            &result_block("tool-1", vec![TextBlock::new(expected).into()]).into()
        );
        assert_eq!(
            first_block(&messages[3]),
            &result_block("tool-2", vec![TextBlock::new(second).into()]).into()
        );
    }

    #[test]
    fn returns_after_successful_truncation_without_trimming_messages() {
        let mut messages = vec![
            u("Message 1"),
            a("Response 1"),
            tool_result("tool-1", &"L".repeat(500)),
        ];
        overflow(&manager(2, true), &mut messages);
        assert_eq!(messages.len(), 3);
    }

    #[test]
    fn skips_truncation_when_should_truncate_results_is_false() {
        let long = "L".repeat(500);
        let mut messages = vec![
            u("Message 1"),
            a("Response 1"),
            u("Message 2"),
            tool_use("tool1", "tool-1"),
            tool_result("tool-1", &long),
        ];
        overflow(&manager(3, false), &mut messages);
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role(), Role::User);
        assert_eq!(
            first_block(&messages[2]),
            &result_block("tool-1", vec![TextBlock::new(long).into()]).into()
        );
    }

    #[test]
    fn does_not_re_truncate_already_truncated_results() {
        let already = truncated(&"A".repeat(200), 1000, &"B".repeat(200));
        let mut messages = vec![tool_result("tool-1", &already)];
        assert!(!truncate_tool_results(&mut messages, 0));
        let mut messages = vec![tool_result("tool-1", &already), a("Response"), u("Message")];
        overflow(&window(40), &mut messages);
        assert!(messages.len() < 3);
    }

    #[test]
    fn replaces_image_blocks_nested_in_tool_results_with_descriptive_placeholders() {
        let image = ImageBlock::new("png", vec![0_u8; 1234]);
        let mut messages = vec![result_with(
            "tool-1",
            vec![image.into(), TextBlock::new("tail").into()],
        )];
        assert!(truncate_tool_results(&mut messages, 0));
        let expected = result_block(
            "tool-1",
            vec![
                TextBlock::new("[image: png, source: bytes, 1234 bytes]").into(),
                TextBlock::new("tail").into(),
            ],
        );
        assert_eq!(first_block(&messages[0]), &expected.into());
    }

    #[test]
    fn preserves_the_error_field_on_truncated_tool_results() {
        let block = ToolResultBlock::new(
            "tool-1",
            ToolResultStatus::Error,
            vec![TextBlock::new("x".repeat(500)).into()],
        )
        .with_error(std::io::Error::other("tool blew up"));
        let mut messages = vec![Message::new(Role::User, vec![block.into()])];
        assert!(truncate_tool_results(&mut messages, 0));
        let ContentBlock::ToolResult(result) = first_block(&messages[0]) else {
            panic!("expected a tool result");
        };
        let expected = truncated(&"x".repeat(200), 100, &"x".repeat(200));
        assert_eq!(result.status(), &ToolResultStatus::Error);
        assert_eq!(
            result.content(),
            [ToolResultContent::Text(TextBlock::new(expected))]
        );
        assert_eq!(
            result.error().map(ToString::to_string).as_deref(),
            Some("tool blew up")
        );
    }

    #[test]
    fn image_placeholder_reflects_non_bytes_source_kinds_honestly() {
        let url = ImageBlock::new("jpeg", ImageSource::Url("https://example.com/x.jpg".to_owned()));
        let s3 = ImageBlock::new("png", S3Location::new("s3://bucket/key"));
        let mut messages = vec![result_with("tool-1", vec![url.into(), s3.into()])];
        truncate_tool_results(&mut messages, 0);
        let expected = result_block(
            "tool-1",
            vec![
                TextBlock::new("[image: jpeg, source: url]").into(),
                TextBlock::new("[image: png, source: s3]").into(),
            ],
        );
        assert_eq!(first_block(&messages[0]), &expected.into());
    }

    fn placeholder_case(item: ToolResultContent, placeholder: &str) {
        let mut messages = vec![result_with("tool-1", vec![item])];
        assert!(truncate_tool_results(&mut messages, 0));
        let expected = result_block("tool-1", vec![TextBlock::new(placeholder).into()]);
        assert_eq!(first_block(&messages[0]), &expected.into());
    }

    #[test]
    fn replaces_video_bytes_blocks_with_a_descriptive_placeholder() {
        placeholder_case(
            VideoBlock::new("mp4", vec![0_u8; 4096]).into(),
            "[video: mp4, source: bytes, 4096 bytes]",
        );
    }

    #[test]
    fn replaces_video_s3_blocks_with_a_descriptive_placeholder() {
        placeholder_case(
            VideoBlock::new("mp4", S3Location::new("s3://b/k")).into(),
            "[video: mp4, source: s3]",
        );
    }

    #[test]
    fn replaces_document_bytes_blocks_with_a_descriptive_placeholder() {
        let document = DocumentBlock::new("report", "pdf", vec![0_u8; 8192]);
        placeholder_case(
            document.into(),
            "[document: report, pdf, source: bytes, 8192 bytes]",
        );
    }

    #[test]
    fn replaces_document_s3_blocks_with_a_descriptive_placeholder() {
        let document = DocumentBlock::new("spec", "pdf", S3Location::new("s3://b/k"));
        placeholder_case(document.into(), "[document: spec, pdf, source: s3]");
    }

    #[test]
    fn partially_truncates_large_text_inside_a_document_text_source() {
        let middle = "M".repeat(240);
        let original = format!("{}{middle}{}", "A".repeat(200), "B".repeat(200));
        let document = DocumentBlock::new("report", "txt", DocumentSource::Text(original));
        let mut messages = vec![result_with("tool-1", vec![document.into()])];
        assert!(truncate_tool_results(&mut messages, 0));
        let expected_text = truncated(&"A".repeat(200), middle.len(), &"B".repeat(200));
        let expected = DocumentBlock::new("report", "txt", DocumentSource::Text(expected_text));
        assert_eq!(
            first_block(&messages[0]),
            &result_block("tool-1", vec![expected.into()]).into()
        );
    }

    #[test]
    fn leaves_small_text_inside_a_document_text_source_unchanged() {
        let document = DocumentBlock::new("short", "txt", DocumentSource::Text("hello".to_owned()));
        let mut messages = vec![result_with("tool-1", vec![document.into()])];
        assert!(!truncate_tool_results(&mut messages, 0));
    }

    #[test]
    fn truncates_long_nested_text_blocks_inside_a_document_content_source() {
        let long = format!("{}{}{}", "A".repeat(200), "M".repeat(240), "B".repeat(200));
        let source = DocumentSource::Content(vec![TextBlock::new(long), TextBlock::new("short")]);
        let mut messages = vec![result_with(
            "tool-1",
            vec![DocumentBlock::new("pages", "txt", source).into()],
        )];
        assert!(truncate_tool_results(&mut messages, 0));
        let expected_text = truncated(&"A".repeat(200), 240, &"B".repeat(200));
        let source = DocumentSource::Content(vec![TextBlock::new(expected_text), TextBlock::new("short")]);
        let expected = DocumentBlock::new("pages", "txt", source);
        assert_eq!(
            first_block(&messages[0]),
            &result_block("tool-1", vec![expected.into()]).into()
        );
    }

    #[test]
    fn replaces_large_json_blocks_with_a_size_placeholder() {
        let big = json!({"payload": "x".repeat(1000)});
        let size = serde_json::to_string(&big).unwrap().len();
        placeholder_case(JsonBlock::new(big).into(), &format!("[json: {size} chars]"));
    }

    #[test]
    fn leaves_small_json_blocks_unchanged() {
        let mut messages = vec![result_with(
            "tool-1",
            vec![JsonBlock::new(json!({"ok": true})).into()],
        )];
        assert!(!truncate_tool_results(&mut messages, 0));
    }

    #[test]
    fn does_not_call_truncate_tool_results_unless_an_error_is_passed_in() {
        let mut messages = vec![
            u("Message 1"),
            tool_use("tool1", "id-1"),
            tool_result("id-1", "Tool result content"),
            a("Response 1"),
            u("Message 2"),
        ];
        window(2).apply_management(&mut messages);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role(), Role::User);
        assert_eq!(first_text(&messages[0]), "Message 2");
    }
}

mod message_trimming {
    use super::*;

    #[test]
    fn trims_oldest_messages_when_tool_results_cannot_be_truncated() {
        let mut messages = vec![
            u("Message 1"),
            a("Response 1"),
            u("Message 2"),
            a("Response 2"),
            u("Message 3"),
        ];
        overflow(&manager(3, false), &mut messages);
        assert_eq!(messages.len(), 3);
        assert_eq!(first_text(&messages[0]), "Message 2");
    }

    #[test]
    fn calculates_correct_trim_index_messages_length_minus_window_size() {
        let mut messages = vec![u("Message 1"), a("Response 1"), u("Message 2"), a("Response 2")];
        overflow(&window(2), &mut messages);
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn removes_all_messages_when_window_size_is_0_on_context_overflow() {
        let mut messages = vec![u("Message 1"), a("Response 1")];
        overflow(&manager(0, false), &mut messages);
        assert!(messages.is_empty());
    }

    #[test]
    fn uses_default_trim_index_of_2_when_messages_le_window_size() {
        let mut messages = vec![u("Message 1"), a("Response 1"), u("Message 2")];
        overflow(&window(5), &mut messages);
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn removes_messages_from_start_of_array_using_splice() {
        let mut messages = vec![u("Message 1"), a("Response 1"), u("Message 2"), a("Response 2")];
        overflow(&window(2), &mut messages);
        assert_eq!(messages.len(), 2);
        assert_eq!(first_text(&messages[0]), "Message 2");
        assert_eq!(first_text(&messages[1]), "Response 2");
    }
}

mod tool_pair_validation {
    use super::*;

    #[test]
    fn does_not_trim_at_index_where_oldest_message_is_tool_result() {
        let mut messages = vec![
            tool_use("tool1", "id-1"),
            tool_result("id-1", "Result"),
            a("Response"),
            u("Message"),
        ];
        overflow(&manager(2, false), &mut messages);
        assert_eq!(messages.len(), 1);
        assert_eq!(first_text(&messages[0]), "Message");
    }

    #[test]
    fn does_not_trim_at_index_where_oldest_message_is_tool_use_without_following_tool_result() {
        let mut messages = vec![
            u("Message 1"),
            tool_use("tool1", "id-1"),
            a("Response"),
            u("Message 2"),
        ];
        overflow(&manager(2, false), &mut messages);
        assert_eq!(messages.len(), 1);
        assert_eq!(first_text(&messages[0]), "Message 2");
    }

    #[test]
    fn allows_trim_when_oldest_message_is_tool_use_with_following_tool_result() {
        let mut messages = vec![
            u("Message 1"),
            tool_use("tool1", "id-1"),
            tool_result("id-1", "Result"),
            a("Response"),
            u("Message 2"),
        ];
        overflow(&manager(2, false), &mut messages);
        assert_eq!(messages.len(), 1);
        assert_eq!(first_text(&messages[0]), "Message 2");
    }

    #[test]
    fn allows_trim_at_tool_use_when_tool_result_immediately_follows() {
        let mut messages = vec![
            u("Message 1"),
            a("Response 1"),
            u("Message 2"),
            tool_use("tool1", "id-1"),
            tool_result("id-1", "Result"),
            a("Response 2"),
        ];
        overflow(&manager(4, false), &mut messages);
        assert_eq!(messages.len(), 4);
        assert_eq!(first_text(&messages[0]), "Message 2");
    }

    #[test]
    fn falls_back_to_a_complete_tool_pair_when_no_plain_user_trim_point_exists() {
        let mut messages = tool_heavy_history();
        let expected: Vec<Message> = [&messages[..1], &messages[5..]].concat();
        overflow(&manager(4, false), &mut messages);
        assert_eq!(messages, expected);
        assert_eq!(
            roles(&messages),
            [Role::User, Role::Assistant, Role::User, Role::Assistant]
        );
    }

    #[test]
    fn keeps_routine_window_management_user_first_when_it_uses_the_tool_pair_fallback() {
        let mut messages = tool_heavy_history();
        let expected: Vec<Message> = [&messages[..1], &messages[5..]].concat();
        manager(4, false).apply_management(&mut messages);
        assert_eq!(messages, expected);
        assert_eq!(
            roles(&messages),
            [Role::User, Role::Assistant, Role::User, Role::Assistant]
        );
    }

    #[test]
    fn prefers_a_plain_user_trim_point_over_a_reachable_complete_tool_pair_later_in_range() {
        let mut messages = vec![
            u("First"),
            a("Response A"),
            u("Plain user message"),
            tool_use("lookup", "id-1"),
            tool_result("id-1", "Result"),
            a("Final"),
        ];
        let expected = messages[2..].to_vec();
        overflow(&manager(4, false), &mut messages);
        assert_eq!(messages, expected);
        assert_eq!(first_text(&messages[0]), "Plain user message");
    }
}

mod tool_pair_fallback {
    use super::*;

    fn pinned_first(window_size: usize, pin_first: usize) -> SlidingWindowConversationManager {
        let config = SlidingWindowConversationManagerConfig::builder()
            .window_size(window_size)
            .should_truncate_results(false)
            .pin_first(pin_first)
            .build();
        SlidingWindowConversationManager::new(config).unwrap()
    }

    /// The history is unchanged apart from the pins `pin_first` adds (TS compares the same objects).
    fn assert_declines(manager: &SlidingWindowConversationManager, mut messages: Vec<Message>) {
        let content = |messages: &[Message]| {
            messages
                .iter()
                .map(|m| (m.role(), m.content().to_vec()))
                .collect::<Vec<_>>()
        };
        let before = content(&messages);
        assert!(!overflow(manager, &mut messages));
        assert_eq!(content(&messages), before);
    }

    fn old_and_recent_pairs() -> [Message; 4] {
        [
            tool_use("lookup", "id-1"),
            tool_result("id-1", "Old result"),
            tool_use("lookup", "id-2"),
            tool_result("id-2", "Recent result"),
        ]
    }

    #[test]
    fn does_not_treat_a_user_message_containing_tool_use_as_an_assistant_tool_pair_boundary() {
        let mixed = Message::new(
            Role::User,
            vec![
                result_block("orphan", vec![TextBlock::new("Orphan result").into()]).into(),
                ToolUseBlock::new("lookup", "id-1", json!({})).into(),
            ],
        );
        let messages = vec![
            u("Anchor"),
            a("Old response"),
            mixed,
            tool_result("id-1", "Result"),
            a("Response"),
            a("Final"),
        ];
        assert_declines(&manager(4, false), messages);
    }

    #[test]
    fn returns_false_when_an_assistant_tool_use_is_not_followed_by_a_tool_result() {
        let user_tool_use = Message::new(
            Role::User,
            vec![ToolUseBlock::new("anotherLookup", "id-2", json!({})).into()],
        );
        let messages = vec![
            u("Anchor"),
            a("Old response"),
            tool_use("lookup", "id-1"),
            user_tool_use,
            a("Response"),
            a("Final"),
        ];
        assert_declines(&manager(4, false), messages);
    }

    #[test]
    fn uses_the_pinned_first_user_as_a_safe_fallback_anchor() {
        let mut messages = vec![u("Pinned user"), a("Old response"), u("Old request")];
        messages.extend(old_and_recent_pairs());
        assert!(overflow(&pinned_first(2, 1), &mut messages));
        assert_eq!(roles(&messages), [Role::User, Role::Assistant, Role::User]);
        assert_eq!(first_text(&messages[0]), "Pinned user");
        assert!(
            matches!(first_block(&messages[1]), ContentBlock::ToolUse(tool) if tool.tool_use_id() == "id-2")
        );
        assert!(
            matches!(first_block(&messages[2]), ContentBlock::ToolResult(result) if result.tool_use_id() == "id-2")
        );
    }

    #[test]
    fn declines_fallback_when_a_pinned_prefix_ends_with_an_assistant_message() {
        let mut messages = vec![u("Pinned user"), a("Pinned response"), u("Old request")];
        messages.extend(old_and_recent_pairs());
        let manager = pinned_first(2, 2);
        let mut copy = messages.clone();
        let (reduced, warnings) =
            crate::test_support::WarnRecorder::capture(|| overflow(&manager, &mut copy));
        assert!(!reduced);
        assert_eq!(copy.len(), messages.len());
        assert_eq!(roles(&copy), roles(&messages));
        assert!(
            !warnings
                .iter()
                .any(|warning| warning.contains("no valid trim point found")),
            "{warnings:?}"
        );
    }

    #[test]
    fn declines_fallback_when_non_contiguous_pinned_messages_do_not_alternate() {
        let mut messages = vec![
            u("Pinned user one"),
            a("Unpinned response"),
            u("Pinned user two"),
            a("Old response"),
            u("Old request"),
        ];
        messages.extend(old_and_recent_pairs());
        pin_message(&mut messages, 0);
        pin_message(&mut messages, 2);
        assert_declines(&manager(2, false), messages);
    }

    #[test]
    fn declines_fallback_when_the_first_pinned_message_is_an_orphaned_tool_result() {
        let mut messages = vec![
            tool_result("orphan", "Orphan result"),
            a("Old response"),
            u("Old request"),
        ];
        messages.extend(old_and_recent_pairs());
        pin_message(&mut messages, 0);
        assert_declines(&manager(2, false), messages);
    }

    #[test]
    fn declines_fallback_when_the_last_pinned_message_carries_a_bare_tool_use_block() {
        let weird = Message::new(
            Role::User,
            vec![ToolUseBlock::new("weird", "weird-1", json!({})).into()],
        );
        let mut messages = vec![weird, a("Old response"), u("Old request")];
        messages.extend(old_and_recent_pairs());
        assert_declines(&pinned_first(2, 1), messages);
    }

    #[test]
    fn declines_fallback_when_no_plain_user_message_exists_before_the_tool_pair() {
        let messages = vec![
            tool_result("orphan-0", "Orphan result"),
            tool_use("lookup", "id-1"),
            tool_result("id-1", "Result 1"),
            tool_use("lookup", "id-2"),
            tool_result("id-2", "Result 2"),
        ];
        assert_declines(&manager(2, false), messages);
    }

    #[test]
    fn allows_trim_when_oldest_message_is_text_or_other_non_tool_content() {
        let mut messages = vec![u("Message 1"), a("Response 1"), u("Message 2"), a("Response 2")];
        overflow(&window(2), &mut messages);
        assert_eq!(messages.len(), 2);
        assert_eq!(first_text(&messages[0]), "Message 2");
    }

    #[test]
    fn skips_assistant_message_to_ensure_trimmed_conversation_starts_with_user() {
        let mut messages: Vec<Message> = (0..9)
            .map(|index| {
                let text = format!("message {index}");
                if index % 2 == 0 { u(&text) } else { a(&text) }
            })
            .collect();
        window(8).apply_management(&mut messages);
        assert_eq!(messages[0].role(), Role::User);
        assert_eq!(first_text(&messages[0]), "message 2");
    }

    #[test]
    fn returns_false_when_no_valid_trim_point_exists() {
        let mut messages = vec![tool_result("id-1", "Result")];
        assert!(!overflow(&manager(1, false), &mut messages));
    }
}

mod helper_methods {
    use super::*;

    #[test]
    fn find_oldest_message_with_tool_results_returns_correct_index_when_tool_results_exist() {
        let messages = [u("Message 1"), tool_result("id-1", "Result 1"), a("Response")];
        assert_eq!(find_oldest_message_with_tool_results(&messages), Some(1));
    }

    #[test]
    fn find_oldest_message_with_tool_results_returns_undefined_when_no_tool_results_exist() {
        assert_eq!(
            find_oldest_message_with_tool_results(&[u("Message 1"), a("Response 1")]),
            None
        );
    }

    #[test]
    fn find_oldest_message_with_tool_results_iterates_forward_from_start() {
        let messages = [
            tool_result("id-1", "Result 1"),
            a("Response"),
            tool_result("id-2", "Result 2"),
        ];
        assert_eq!(find_oldest_message_with_tool_results(&messages), Some(0));
    }

    #[test]
    fn truncate_tool_results_returns_true_when_changes_are_made() {
        assert!(truncate_tool_results(
            &mut [tool_result("id-1", &"x".repeat(500))],
            0
        ));
    }

    #[test]
    fn truncate_tool_results_returns_false_when_already_truncated() {
        let already = truncated(&"A".repeat(200), 1000, &"B".repeat(200));
        assert!(!truncate_tool_results(&mut [tool_result("id-1", &already)], 0));
    }

    #[test]
    fn truncate_tool_results_returns_false_when_no_tool_results_found() {
        assert!(!truncate_tool_results(&mut [u("Message")], 0));
    }

    #[test]
    fn truncation_counts_utf16_units_like_ts() {
        // 226 surrogate pairs are 452 UTF-16 units (over the 450 threshold) but 226 chars.
        let text = "😀".repeat(226);
        assert!(truncate_tool_results(&mut [tool_result("id-1", &text)], 0));
    }
}

mod reduce_on_threshold {
    use super::*;

    async fn proactive(messages: Vec<Message>, projected: u64) -> usize {
        let config = SlidingWindowConversationManagerConfig::builder()
            .window_size(4)
            .proactive_compression(ProactiveCompression::Threshold(0.7))
            .build();
        let model = crate::testing::TestModelProvider::new();
        model.update_config(crate::models::model::BaseModelConfig {
            context_window_limit: Some(1000),
            ..Default::default()
        });
        let model: Arc<dyn Model> = Arc::new(model);
        let mut agent = LocalAgent::new(Arc::clone(&model));
        agent.set_messages(messages);
        let plugin = Arc::new(ConversationManagerPlugin(Arc::new(
            SlidingWindowConversationManager::new(config).unwrap(),
        )));
        plugin.init_agent(&mut agent).await.unwrap();
        let hooks = agent.hooks().clone();
        let mut event = BeforeModelCallEvent::new(&mut agent, model, InvocationState::new(), Some(projected));
        hooks
            .invoke::<BeforeModelCallEvent<'static>>(&mut event)
            .await
            .unwrap();
        agent.messages().len()
    }

    #[tokio::test]
    async fn trims_oldest_messages_when_compression_threshold_is_exceeded() {
        let messages = vec![
            u("Message 1"),
            a("Response 1"),
            u("Message 2"),
            a("Response 2"),
            u("Message 3"),
            a("Response 3"),
        ];
        assert_eq!(proactive(messages, 800).await, 4);
    }

    #[tokio::test]
    async fn does_not_trim_when_below_compression_threshold() {
        assert_eq!(proactive(vec![u("Message 1"), a("Response 1")], 500).await, 2);
    }
}

mod pin_first {
    use super::*;

    fn texts(messages: &[Message]) -> Vec<&str> {
        messages.iter().map(first_text).collect()
    }

    fn six() -> Vec<Message> {
        vec![
            u("first"),
            a("second"),
            u("third"),
            a("fourth"),
            u("fifth"),
            a("sixth"),
        ]
    }

    #[test]
    fn protects_first_n_messages_from_trimming() {
        let config = SlidingWindowConversationManagerConfig::builder()
            .window_size(2)
            .pin_first(2)
            .build();
        let mut messages = six();
        SlidingWindowConversationManager::new(config)
            .unwrap()
            .apply_management(&mut messages);
        assert_eq!(texts(&messages), ["first", "second", "fifth", "sixth"]);
    }

    #[test]
    fn returns_false_when_all_messages_in_trim_range_are_protected() {
        let config = SlidingWindowConversationManagerConfig::builder()
            .window_size(2)
            .pin_first(4)
            .build();
        let mut messages = vec![u("a"), a("b"), u("c"), a("d")];
        assert!(
            !SlidingWindowConversationManager::new(config)
                .unwrap()
                .reduce_context(&mut messages, false)
        );
    }

    #[test]
    fn pinned_message_in_middle_of_window_survives_trimming() {
        let mut messages = six();
        messages[2] = u("pinned-middle");
        pin_message(&mut messages, 2);
        window(4).apply_management(&mut messages);
        assert_eq!(texts(&messages), ["pinned-middle", "fourth", "fifth", "sixth"]);
    }
}

mod hooks {
    use super::*;

    async fn agent_with(manager: SlidingWindowConversationManager, messages: Vec<Message>) -> LocalAgent {
        let model: Arc<dyn Model> = Arc::new(MockMessageModel::new());
        let mut agent = LocalAgent::new(model);
        agent.set_messages(messages);
        let plugin = Arc::new(ConversationManagerPlugin(Arc::new(manager)));
        plugin.init_agent(&mut agent).await.unwrap();
        agent
    }

    #[tokio::test]
    async fn after_invocation_event_applies_the_window() {
        let mut agent = agent_with(
            window(2),
            vec![u("Message 1"), a("Response 1"), u("Message 2"), a("Response 2")],
        )
        .await;
        let hooks = agent.hooks().clone();
        let mut event = AfterInvocationEvent::new(&mut agent, InvocationState::new());
        hooks
            .invoke::<AfterInvocationEvent<'static>>(&mut event)
            .await
            .unwrap();
        assert_eq!(agent.messages().len(), 2);
    }

    #[tokio::test]
    async fn propagates_the_original_context_window_overflow_error_when_reduce_cannot_reduce_further() {
        let mut agent = agent_with(manager(1, false), vec![tool_result("id-1", "Result")]).await;
        let hooks = agent.hooks().clone();
        let error = crate::error::ContextWindowOverflowError::new("Context overflow").into();
        let model: Arc<dyn Model> = Arc::new(MockMessageModel::new());
        let mut event = AfterModelCallEvent::new(
            &mut agent,
            model,
            InvocationState::new(),
            std::num::NonZeroU32::MIN,
        )
        .with_error(&error);
        hooks
            .invoke::<AfterModelCallEvent<'static>>(&mut event)
            .await
            .unwrap();
        assert!(!event.retry());
    }

    #[tokio::test]
    async fn an_overflow_trims_and_sets_retry() {
        let mut agent = agent_with(
            manager(2, false),
            vec![u("Message 1"), a("Response 1"), u("Message 2"), a("Response 2")],
        )
        .await;
        let hooks = agent.hooks().clone();
        let error = crate::error::ContextWindowOverflowError::new("Context overflow").into();
        let model: Arc<dyn Model> = Arc::new(MockMessageModel::new());
        let mut event = AfterModelCallEvent::new(
            &mut agent,
            model,
            InvocationState::new(),
            std::num::NonZeroU32::MIN,
        )
        .with_error(&error);
        hooks
            .invoke::<AfterModelCallEvent<'static>>(&mut event)
            .await
            .unwrap();
        assert!(event.retry());
        assert_eq!(agent.messages().len(), 2);
    }
}
