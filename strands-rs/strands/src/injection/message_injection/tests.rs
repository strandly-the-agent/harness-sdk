//! Ported from strands-ts/src/injection/__tests__/message-injection.test.ts.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};

use super::*;
use crate::error::Error;
use crate::injection::{InjectionTrigger, Trigger, render_content};
use crate::test_support::WarnRecorder;
use crate::testing::MockMessageModel;
use crate::types::agent::{InvocationState, LocalAgent, ToolAgent};
use crate::types::messages::{MessageMetadata, ToolResultBlock, ToolResultContent};
use crate::types::tools::ToolResultStatus;

fn user(text: &str) -> Message {
    Message::new(Role::User, vec![TextBlock::new(text).into()])
}

fn assistant(text: &str) -> Message {
    Message::new(Role::Assistant, vec![TextBlock::new(text).into()])
}

fn tool_result() -> Message {
    let block = ToolResultBlock::new(
        "t1",
        ToolResultStatus::Success,
        vec![ToolResultContent::Text(TextBlock::new("done"))],
    );
    Message::new(Role::User, vec![block.into()])
}

/// The messages as wire data, without their tracking ids (TS `anyTrackingId`).
fn wire(messages: &[Message]) -> Value {
    messages
        .iter()
        .map(|message| {
            let mut data = serde_json::to_value(message).unwrap();
            data.as_object_mut().unwrap().remove("trackingId");
            data
        })
        .collect()
}

fn content(message: &Message) -> Value {
    serde_json::to_value(message.content()).unwrap()
}

fn tracking_ids(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .map(|message| message.tracking_id().to_owned())
        .collect()
}

mod fold_into_last_user_message {
    use super::*;

    #[test]
    fn appends_the_text_as_a_trailing_text_block_on_the_last_user_message_behind_its_content() {
        let messages = vec![user("original task"), assistant("prior step"), user("next ask")];
        let ids = tracking_ids(&messages);
        let result = fold_into_last_user_message(messages, "INJECTED");

        // A trailing run is the only placement a provider can keep out of its cached prefix.
        assert_eq!(
            wire(&result.messages),
            json!([
                { "role": "user", "content": [{ "text": "original task" }] },
                { "role": "assistant", "content": [{ "text": "prior step" }] },
                { "role": "user", "content": [{ "text": "next ask" }, { "text": "\n\nINJECTED" }] },
            ])
        );
        assert_eq!(tracking_ids(&result.messages), ids);
        assert_eq!(result.appended, 1);
    }

    #[test]
    fn preserves_the_folded_into_message_tracking_id_same_logical_message() {
        let target = user("ask").with_tracking_id("durable-1");
        let result = fold_into_last_user_message(vec![assistant("prior"), target], "INJECTED");

        assert_eq!(result.messages[1].tracking_id(), "durable-1");
    }

    /// The Rust fold takes the call's messages by value; the caller's copy is its own.
    #[test]
    fn returns_a_new_array_and_does_not_mutate_the_input_or_its_messages() {
        let original = user("ask");
        let messages = vec![assistant("prior"), original.clone()];
        let result = fold_into_last_user_message(messages.clone(), "INJECTED");

        assert_eq!(messages[1], original);
        assert_eq!(original.content().len(), 1);
        assert_ne!(result.messages[1], original);
        assert_eq!(result.messages[0], messages[0]);
    }

    #[test]
    fn appends_after_the_tool_result_block_when_the_target_is_a_tool_result_turn() {
        let tr = tool_result();
        let result =
            fold_into_last_user_message(vec![user("task"), assistant("thinking"), tr.clone()], "INJECTED");

        // A tool result must stay first in the turn answering a tool use; appending satisfies that.
        assert_eq!(
            wire(&result.messages),
            json!([
                { "role": "user", "content": [{ "text": "task" }] },
                { "role": "assistant", "content": [{ "text": "thinking" }] },
                { "role": "user", "content": [content(&tr)[0], { "text": "\n\nINJECTED" }] },
            ])
        );
    }

    #[test]
    fn targets_the_most_recent_user_message_when_several_exist() {
        let messages = vec![user("first"), assistant("a"), user("second")];
        let result = fold_into_last_user_message(messages, "INJECTED");

        assert_eq!(
            wire(&result.messages),
            json!([
                { "role": "user", "content": [{ "text": "first" }] },
                { "role": "assistant", "content": [{ "text": "a" }] },
                { "role": "user", "content": [{ "text": "second" }, { "text": "\n\nINJECTED" }] },
            ])
        );
    }

    #[test]
    fn preserves_message_metadata_on_the_folded_message() {
        let custom = json!({ "keep": "me" }).as_object().unwrap().clone();
        let tagged = user("ask").with_metadata(MessageMetadata::new().with_custom(custom.clone()));
        let result = fold_into_last_user_message(vec![tagged], "INJECTED");
        assert_eq!(result.messages[0].metadata().unwrap().custom(), Some(&custom));
    }

    #[test]
    fn returns_the_input_unchanged_when_there_is_no_user_message() {
        let messages = vec![assistant("only assistant")];
        let result = fold_into_last_user_message(messages.clone(), "INJECTED");
        assert_eq!(result.messages, messages);
        assert_eq!(result.appended, 0);
    }

    #[test]
    fn reports_no_per_call_trailing_blocks_when_the_fold_target_is_not_the_last_message() {
        // Counting an earlier message's blocks would make the provider count back through the wrong one.
        let result = fold_into_last_user_message(vec![user("ask"), assistant("reply")], "INJECTED");

        assert_eq!(
            content(&result.messages[0]),
            json!([{ "text": "ask" }, { "text": "\n\nINJECTED" }])
        );
        assert_eq!(result.appended, 0);
    }

    #[test]
    fn omits_the_blank_line_separator_when_the_target_message_has_no_content() {
        let result = fold_into_last_user_message(vec![Message::new(Role::User, Vec::new())], "INJECTED");
        assert_eq!(content(&result.messages[0]), json!([{ "text": "INJECTED" }]));
    }
}

mod is_user_turn {
    use super::*;

    #[test]
    fn is_true_when_the_last_message_is_a_plain_user_ask() {
        assert!(is_user_turn(&[assistant("prior"), user("ask")]));
    }

    #[test]
    fn is_false_when_the_last_message_is_a_user_tool_result_turn() {
        assert!(!is_user_turn(&[user("task"), assistant("a"), tool_result()]));
    }

    #[test]
    fn is_false_when_the_last_message_is_an_assistant_message() {
        assert!(!is_user_turn(&[user("ask"), assistant("reply")]));
    }

    #[test]
    fn is_false_for_an_empty_conversation() {
        assert!(!is_user_turn(&[]));
    }
}

fn mock_agent() -> ToolAgent {
    LocalAgent::new(Arc::new(MockMessageModel::new())).tool_agent()
}

fn injection_ctx(messages: Vec<Message>) -> InjectionContext {
    InjectionContext::new(messages, mock_agent())
}

mod resolve_trigger {
    use super::*;

    #[test]
    fn defaults_undefined_to_the_user_turn_policy() {
        let trigger = Trigger::default();
        assert!(trigger.fires(&injection_ctx(vec![user("ask")])));
        assert!(!trigger.fires(&injection_ctx(vec![tool_result()])));
    }

    #[test]
    fn user_turn_uses_is_user_turn() {
        let trigger = Trigger::from(InjectionTrigger::UserTurn);
        assert!(trigger.fires(&injection_ctx(vec![user("ask")])));
        assert!(!trigger.fires(&injection_ctx(vec![tool_result()])));
    }

    #[test]
    fn every_turn_always_fires() {
        let trigger = Trigger::from(InjectionTrigger::EveryTurn);
        assert!(trigger.fires(&injection_ctx(Vec::new())));
        assert!(trigger.fires(&injection_ctx(vec![tool_result()])));
    }

    #[test]
    fn uses_a_custom_predicate_over_the_context() {
        let trigger = Trigger::predicate(|context| context.messages.len() >= 2);
        assert!(!trigger.fires(&injection_ctx(vec![user("a")])));
        assert!(trigger.fires(&injection_ctx(vec![user("a"), assistant("b")])));
    }
}

mod create_injection_middleware {
    use super::*;

    fn ctx_with(agent: ToolAgent, messages: Vec<Message>) -> InvokeModelContext {
        let model = Arc::clone(agent.model());
        InvokeModelContext::new(
            agent,
            model,
            messages,
            None,
            Vec::new(),
            None,
            InvocationState::new(),
            None,
            None,
        )
    }

    fn ctx(messages: Vec<Message>) -> InvokeModelContext {
        ctx_with(mock_agent(), messages)
    }

    fn options(trigger: Trigger, text: Option<&'static str>) -> InjectionMiddlewareOptions {
        InjectionMiddlewareOptions {
            trigger,
            render_content: render_content(move |_context| async move { Ok(text.map(str::to_owned)) }),
        }
    }

    fn injecting(text: &'static str) -> InjectionMiddlewareOptions {
        options(Trigger::default(), Some(text))
    }

    fn assert_unchanged(result: &InvokeModelContext, input: &[Message]) {
        assert_eq!(result.messages, input);
        assert_eq!(result.dynamic_trailing_blocks, None);
    }

    #[tokio::test]
    async fn folds_render_content_text_into_the_latest_user_message_leaving_other_context_fields_intact() {
        let input = ctx(vec![assistant("prior"), user("ask")]);
        let ids = tracking_ids(&input.messages);
        let result = apply_injection(&injecting("INJECTED"), input).await;

        assert_eq!(
            wire(&result.messages),
            json!([
                { "role": "assistant", "content": [{ "text": "prior" }] },
                { "role": "user", "content": [{ "text": "ask" }, { "text": "\n\nINJECTED" }] },
            ])
        );
        assert_eq!(tracking_ids(&result.messages), ids);
        assert!(result.system_prompt.is_none());
        assert!(result.tool_specs.is_empty());
        assert_eq!(result.projected_input_tokens, None);
    }

    #[tokio::test]
    async fn passes_an_injection_context_carrying_the_conversation_as_data_to_render_content() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);
        let opts = InjectionMiddlewareOptions {
            trigger: Trigger::default(),
            render_content: render_content(move |context| {
                let recorded = Arc::clone(&recorded);
                async move {
                    let roles = context.messages.iter().map(Message::role);
                    recorded.lock().unwrap().extend(roles);
                    Ok(Some("x".to_owned()))
                }
            }),
        };
        apply_injection(&opts, ctx(vec![assistant("prior"), user("ask")])).await;

        assert_eq!(*seen.lock().unwrap(), [Role::Assistant, Role::User]);
    }

    #[tokio::test]
    async fn exposes_app_state_and_the_agent_on_the_injection_context() {
        let agent = mock_agent();
        agent.app_state().set("key", json!("stashed")).unwrap();
        let received = Arc::new(std::sync::Mutex::new(None));
        let recorded = Arc::clone(&received);
        let opts = InjectionMiddlewareOptions {
            trigger: Trigger::default(),
            render_content: render_content(move |context| {
                let recorded = Arc::clone(&recorded);
                async move {
                    *recorded.lock().unwrap() = Some((
                        context.app_state.get_value("key"),
                        context.agent.app_state().get_value("key"),
                        context.agent.id().to_owned(),
                    ));
                    Ok(None)
                }
            }),
        };
        let id = agent.id().to_owned();
        apply_injection(&opts, ctx_with(agent, vec![user("ask")])).await;

        assert_eq!(
            received.lock().unwrap().take(),
            Some((Some(json!("stashed")), Some(json!("stashed")), id))
        );
    }

    #[tokio::test]
    async fn returns_the_context_unchanged_when_the_trigger_does_not_fire() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let opts = InjectionMiddlewareOptions {
            trigger: Trigger::default(),
            render_content: render_content(move |_context| {
                counted.fetch_add(1, Ordering::SeqCst);
                async { Ok(Some("x".to_owned())) }
            }),
        };
        let input = vec![user("task"), assistant("a"), tool_result()];
        let result = apply_injection(&opts, ctx(input.clone())).await;

        assert_unchanged(&result, &input);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn every_turn_injects_on_an_autonomous_tool_result_turn_keeping_the_tool_result_first() {
        let tr = tool_result();
        let opts = options(InjectionTrigger::EveryTurn.into(), Some("INJECTED"));
        let result = apply_injection(&opts, ctx(vec![user("task"), assistant("a"), tr.clone()])).await;

        // The most recent user message on a tool-result turn carries the tool result, which must
        // stay the first block, so the injected text is appended after it.
        assert_eq!(
            wire(&result.messages),
            json!([
                { "role": "user", "content": [{ "text": "task" }] },
                { "role": "assistant", "content": [{ "text": "a" }] },
                { "role": "user", "content": [content(&tr)[0], { "text": "\n\nINJECTED" }] },
            ])
        );
    }

    #[tokio::test]
    async fn returns_the_context_unchanged_when_render_content_yields_empty_text() {
        let input = vec![assistant("prior"), user("ask")];
        let result = apply_injection(&injecting("   "), ctx(input.clone())).await;

        assert_unchanged(&result, &input);
    }

    #[test]
    fn fails_open_returns_the_context_unchanged_logs_when_render_content_throws() {
        let opts = InjectionMiddlewareOptions {
            trigger: Trigger::default(),
            render_content: render_content(|_context| async { Err(Error::other("boom")) }),
        };
        let input = vec![assistant("prior"), user("ask")];
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let (result, warnings) =
            WarnRecorder::capture(|| runtime.block_on(apply_injection(&opts, ctx(input.clone()))));

        assert_unchanged(&result, &input);
        assert_eq!(
            warnings,
            ["reason=<boom> | injection renderContent threw; skipping injection"]
        );
    }

    #[tokio::test]
    async fn does_not_mutate_the_original_context_messages() {
        let input = ctx(vec![assistant("prior"), user("ask")]);
        let before = input.messages[1].clone();
        let history = Arc::clone(input.agent().messages());
        apply_injection(&injecting("INJECTED"), input).await;

        assert_eq!(before.content().len(), 1);
        assert!(history.is_empty());
    }

    // No TS case: TS has no middleware phases to assert; this pins the phase of the Rust handler.
    /// The handler `create_injection_middleware` builds is an Input-phase handler.
    #[test]
    fn builds_an_input_phase_handler() {
        let middleware = create_injection_middleware(injecting("INJECTED"));
        assert_eq!(middleware.phase(), crate::middleware::MiddlewarePhaseKind::Input);
    }

    mod per_call_trailing_blocks_reporting {
        use super::*;

        // The boundary a provider needs is reported on the context, not written into the messages.
        #[tokio::test]
        async fn reports_one_trailing_block_as_per_call() {
            let result = apply_injection(&injecting("INJECTED"), ctx(vec![user("ask")])).await;

            assert_eq!(result.dynamic_trailing_blocks, Some(1));
        }

        #[tokio::test]
        async fn accumulates_across_producers_on_one_call() {
            let first = apply_injection(&injecting("ONE"), ctx(vec![user("ask")])).await;
            let result = apply_injection(&injecting("TWO"), first).await;

            assert_eq!(result.dynamic_trailing_blocks, Some(2));
        }

        #[tokio::test]
        async fn reports_nothing_when_injection_is_skipped() {
            let result = apply_injection(&injecting(""), ctx(vec![user("ask")])).await;

            assert_eq!(result.dynamic_trailing_blocks.unwrap_or(0), 0);
        }

        #[tokio::test]
        async fn reports_nothing_when_the_fold_target_is_not_the_last_message() {
            let opts = options(InjectionTrigger::EveryTurn.into(), Some("INJECTED"));
            let result = apply_injection(&opts, ctx(vec![user("ask"), assistant("reply")])).await;

            assert_eq!(result.dynamic_trailing_blocks, Some(0));
        }

        #[tokio::test]
        async fn emits_no_cache_point_into_the_messages() {
            // Most providers reject a cachePoint inside a message, so the framework never writes one.
            let result = apply_injection(&injecting("INJECTED"), ctx(vec![user("ask")])).await;

            let mut blocks = result.messages.iter().flat_map(Message::content);
            assert!(!blocks.any(|block| matches!(block, ContentBlock::CachePoint(_))));
        }
    }
}
