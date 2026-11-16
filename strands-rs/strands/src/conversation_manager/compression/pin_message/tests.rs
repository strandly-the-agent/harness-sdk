//! Ported from strands-ts/src/conversation-manager/__tests__/pin.test.ts.

use serde_json::json;

use super::{is_pinned, pin_message, unpin_message};
use crate::models::streaming::Usage;
use crate::test_support::text_result;
use crate::types::json::{JsonObject, JsonValue};
use crate::types::messages::{Message, MessageMetadata, Role, TextBlock, ToolUseBlock};

fn custom(value: JsonValue) -> JsonObject {
    serde_json::from_value(value).unwrap()
}

fn make_message(text: &str, metadata: Option<MessageMetadata>) -> Message {
    let message = Message::new(Role::User, vec![TextBlock::new(text).into()]);
    match metadata {
        Some(metadata) => message.with_metadata(metadata),
        None => message,
    }
}

fn with_custom(text: &str, value: JsonValue) -> Message {
    make_message(text, Some(MessageMetadata::new().with_custom(custom(value))))
}

fn custom_field<'a>(message: &'a Message, key: &str) -> Option<&'a JsonValue> {
    message.metadata()?.custom()?.get(key)
}

fn tool_pair() -> Vec<Message> {
    vec![
        Message::new(
            Role::Assistant,
            vec![ToolUseBlock::new("test", "id-1", json!({})).into()],
        ),
        Message::new(Role::User, vec![text_result("id-1", "result").into()]),
        make_message("other", None),
    ]
}

mod is_pinned {
    use super::*;

    #[test]
    fn returns_false_for_message_without_metadata() {
        assert!(!is_pinned(&[make_message("hello", None)], 0));
    }

    #[test]
    fn returns_false_for_message_with_empty_custom() {
        assert!(!is_pinned(&[with_custom("hello", json!({}))], 0));
    }

    #[test]
    fn returns_true_for_message_with_custom_pinned_true() {
        assert!(is_pinned(&[with_custom("hello", json!({"pinned": true}))], 0));
    }

    #[test]
    fn returns_false_for_message_with_custom_pinned_false() {
        assert!(!is_pinned(&[with_custom("hello", json!({"pinned": false}))], 0));
    }
}

mod pin_message {
    use super::*;

    #[test]
    fn sets_pinned_true_in_custom_metadata() {
        let mut messages = [make_message("important", None)];
        pin_message(&mut messages, 0);
        assert!(is_pinned(&messages, 0));
        assert_eq!(messages[0].role(), Role::User);
    }

    #[test]
    fn preserves_existing_metadata() {
        let usage = Usage::new(10, 5, 15);
        let mut messages = [make_message(
            "important",
            Some(MessageMetadata::new().with_usage(usage.clone())),
        )];
        pin_message(&mut messages, 0);
        assert_eq!(
            messages[0].metadata().and_then(MessageMetadata::usage),
            Some(&usage)
        );
        assert!(is_pinned(&messages, 0));
    }

    #[test]
    fn preserves_existing_custom_fields() {
        let mut messages = [with_custom("important", json!({"myField": "value"}))];
        pin_message(&mut messages, 0);
        assert_eq!(custom_field(&messages[0], "myField"), Some(&json!("value")));
        assert!(is_pinned(&messages, 0));
    }
}

mod unpin_message {
    use super::*;

    #[test]
    fn removes_pinned_from_custom_metadata() {
        let mut messages = [make_message("important", None)];
        pin_message(&mut messages, 0);
        unpin_message(&mut messages, 0);
        assert!(!is_pinned(&messages, 0));
    }

    #[test]
    fn preserves_other_custom_fields() {
        let mut messages = [with_custom("important", json!({"pinned": true, "other": "keep"}))];
        unpin_message(&mut messages, 0);
        assert!(!is_pinned(&messages, 0));
        assert_eq!(custom_field(&messages[0], "other"), Some(&json!("keep")));
    }

    #[test]
    fn removes_metadata_entirely_when_nothing_remains() {
        let mut messages = [make_message("hello", None)];
        pin_message(&mut messages, 0);
        unpin_message(&mut messages, 0);
        assert!(messages[0].metadata().is_none());
    }

    #[test]
    fn preserves_non_custom_metadata_fields() {
        let usage = Usage::new(10, 5, 15);
        let metadata = MessageMetadata::new()
            .with_usage(usage.clone())
            .with_custom(custom(json!({"pinned": true})));
        let mut messages = [make_message("important", Some(metadata))];
        unpin_message(&mut messages, 0);
        assert_eq!(
            messages[0].metadata().and_then(MessageMetadata::usage),
            Some(&usage)
        );
        assert!(!is_pinned(&messages, 0));
    }
}

mod is_pinned_with_messages_array {
    use super::*;

    #[test]
    fn returns_false_for_unpinned_message() {
        let messages = [make_message("a", None), make_message("b", None)];
        assert!(!is_pinned(&messages, 0));
    }

    #[test]
    fn returns_true_for_pinned_message() {
        let mut messages = [make_message("a", None), make_message("b", None)];
        pin_message(&mut messages, 0);
        assert!(is_pinned(&messages, 0));
    }

    #[test]
    fn returns_true_for_tool_result_whose_tool_use_partner_is_pinned() {
        let mut messages = tool_pair();
        pin_message(&mut messages, 0);
        assert!(is_pinned(&messages, 1));
    }

    #[test]
    fn returns_true_for_tool_use_whose_tool_result_partner_is_pinned() {
        let mut messages = tool_pair();
        pin_message(&mut messages, 1);
        assert!(is_pinned(&messages, 0));
    }

    #[test]
    fn returns_false_for_unrelated_message_next_to_pinned() {
        let mut messages = [make_message("a", None), make_message("b", None)];
        pin_message(&mut messages, 0);
        assert!(!is_pinned(&messages, 1));
    }
}
