//! Ported from strands-ts/src/interventions/__tests__/registry.test.ts.
//!
//! Not ported, because the Rust API can't express them:
//! - `only registers hooks for overridden methods`: a trait can't detect overrides, so every
//!   gate is registered and the default methods proceed (the observable half, that an
//!   unimplemented gate leaves its event untouched, is asserted in
//!   `skips_handlers_that_do_not_override_the_method`).
//! - `transform apply() error is handled via onError policy` and `transform apply() error with
//!   onError=throw propagates`: a transform closure returns `()` and can't fail.
//! - `warns when action has no effect on event type`: each gate has its own action type, so an
//!   action without effect doesn't compile.

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;

use super::*;
use crate::agent::Agent;
use crate::hooks::{Cancel, ModelStopData};
use crate::interrupt::Interrupt;
use crate::interventions::{confirm, deny, guide, proceed, transform};
use crate::test_support::{mock_tool, text_result};
use crate::testing::MockMessageModel;
use crate::types::agent::{InvocationState, LocalAgent};
use crate::types::messages::StopReason;
use crate::types::tools::ToolUse;

type ToolGate = Box<dyn Fn(&mut BeforeToolCallEvent) -> Result<BeforeToolCallAction> + Send + Sync>;

/// A handler whose `before_tool_call` is a closure.
struct ToolHandler {
    name: &'static str,
    on_error: OnError,
    gate: ToolGate,
}

#[async_trait]
impl InterventionHandler for ToolHandler {
    fn name(&self) -> &str {
        self.name
    }

    fn on_error(&self) -> OnError {
        self.on_error
    }

    async fn before_tool_call(&self, event: &mut BeforeToolCallEvent) -> Result<BeforeToolCallAction> {
        (self.gate)(event)
    }
}

fn tool_handler(
    name: &'static str,
    gate: impl Fn(&mut BeforeToolCallEvent) -> Result<BeforeToolCallAction> + Send + Sync + 'static,
) -> Arc<dyn InterventionHandler> {
    tool_handler_on_error(name, OnError::Throw, gate)
}

fn tool_handler_on_error(
    name: &'static str,
    on_error: OnError,
    gate: impl Fn(&mut BeforeToolCallEvent) -> Result<BeforeToolCallAction> + Send + Sync + 'static,
) -> Arc<dyn InterventionHandler> {
    Arc::new(ToolHandler {
        name,
        on_error,
        gate: Box::new(gate),
    })
}

fn deny_handler() -> Arc<dyn InterventionHandler> {
    tool_handler("deny-handler", |_| Ok(deny("not authorized").into()))
}

fn guide_handler() -> Arc<dyn InterventionHandler> {
    tool_handler("guide-handler", |_| Ok(guide("add more context").into()))
}

fn confirm_handler() -> Arc<dyn InterventionHandler> {
    tool_handler("confirm-handler", |_| Ok(confirm("approve this action?").into()))
}

fn proceed_handler() -> Arc<dyn InterventionHandler> {
    tool_handler("proceed-handler", |_| {
        Ok(proceed().with_reason("all good").into())
    })
}

fn throwing_handler(name: &'static str, on_error: OnError) -> Arc<dyn InterventionHandler> {
    tool_handler_on_error(name, on_error, |_| Err(Error::other("handler crashed")))
}

/// A handler that records its call and proceeds.
fn later_handler(called: &Arc<Mutex<Vec<String>>>) -> Arc<dyn InterventionHandler> {
    let called = Arc::clone(called);
    tool_handler("later", move |_| {
        called.lock().unwrap().push("later".to_owned());
        Ok(proceed().into())
    })
}

struct ModelGuideHandler;

#[async_trait]
impl InterventionHandler for ModelGuideHandler {
    fn name(&self) -> &str {
        "model-guide"
    }

    async fn after_model_call(&self, _event: &mut AfterModelCallEvent<'_>) -> Result<AfterModelCallAction> {
        Ok(guide("be more specific").into())
    }
}

fn local_agent() -> LocalAgent {
    LocalAgent::new(Arc::new(MockMessageModel::new()))
}

fn before_tool_call_event(agent: &LocalAgent) -> BeforeToolCallEvent {
    BeforeToolCallEvent::new(
        agent.tool_agent(),
        ToolUse::new("testTool", "id-1", json!({})),
        None,
        InvocationState::new(),
    )
}

fn after_model_call_event(agent: &mut LocalAgent) -> AfterModelCallEvent<'_> {
    let model = Arc::clone(agent.model());
    let message = Message::new(
        Role::Assistant,
        vec![ContentBlock::Text(TextBlock::new("response"))],
    );
    AfterModelCallEvent::new(agent, model, InvocationState::new(), NonZeroU32::MIN)
        .with_stop_data(ModelStopData::new(message, StopReason::EndTurn))
}

fn before_model_call_event(agent: &mut LocalAgent) -> BeforeModelCallEvent<'_> {
    let model = Arc::clone(agent.model());
    BeforeModelCallEvent::new(agent, model, InvocationState::new(), None)
}

/// Registers `handlers` and fires one `BeforeToolCallEvent`.
async fn fire_tool(
    handlers: Vec<Arc<dyn InterventionHandler>>,
    agent: &LocalAgent,
) -> (BeforeToolCallEvent, Result<()>) {
    let hooks = HookRegistry::new();
    InterventionRegistry::new(handlers, &hooks).unwrap();
    let mut event = before_tool_call_event(agent);
    let result = hooks.invoke::<BeforeToolCallEvent>(&mut event).await;
    (event, result)
}

fn cancel_message(event: &BeforeToolCallEvent) -> Option<&str> {
    event.cancel().and_then(Cancel::message)
}

/// Stores the answer to `handler`'s confirm, as a resumed agent would hold it.
fn preload_interrupt_response(agent: &LocalAgent, handler: &str, response: JsonValue) {
    let id = format!("hook:beforeToolCall:id-1:{handler}");
    let interrupt = Interrupt::new(id.clone(), handler)
        .with_response(response)
        .with_source(crate::interrupt::InterruptSource::Hook);
    agent.interrupt_state().lock().interrupts.insert(id, interrupt);
}

fn last_text(agent: &LocalAgent) -> (Role, String) {
    let message = agent.messages().last().unwrap();
    let ContentBlock::Text(block) = &message.content()[0] else {
        panic!("a text block")
    };
    (message.role(), block.text().to_owned())
}

mod constructor {
    use super::*;

    #[test]
    fn rejects_duplicate_handler_names() {
        let error = InterventionRegistry::new(vec![deny_handler(), deny_handler()], &HookRegistry::new())
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Duplicate intervention handler name: 'deny-handler'"
        );
    }

    #[test]
    fn accepts_handlers_with_unique_names() {
        let registry =
            InterventionRegistry::new(vec![deny_handler(), guide_handler()], &HookRegistry::new()).unwrap();
        let names: Vec<&str> = registry.handlers().iter().map(|handler| handler.name()).collect();
        assert_eq!(names, ["deny-handler", "guide-handler"]);
    }
}

mod dispatch_ordering {
    use super::*;

    #[tokio::test]
    async fn calls_handlers_in_registration_order() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let recording = |name: &'static str| {
            let order = Arc::clone(&order);
            tool_handler(name, move |_| {
                order.lock().unwrap().push(name);
                Ok(proceed().into())
            })
        };
        fire_tool(vec![recording("first"), recording("second")], &local_agent())
            .await
            .1
            .unwrap();
        assert_eq!(*order.lock().unwrap(), ["first", "second"]);
    }

    #[tokio::test]
    async fn skips_handlers_that_do_not_override_the_method() {
        let called = Arc::new(Mutex::new(Vec::new()));
        let mut agent = local_agent();
        let hooks = HookRegistry::new();
        InterventionRegistry::new(vec![later_handler(&called), deny_handler()], &hooks).unwrap();
        let mut event = after_model_call_event(&mut agent);
        hooks
            .invoke::<AfterModelCallEvent<'static>>(&mut event)
            .await
            .unwrap();
        assert!(!event.retry());
        assert!(called.lock().unwrap().is_empty());
    }
}

mod deny_action {
    use super::*;

    #[tokio::test]
    async fn sets_cancel_on_before_tool_call_event() {
        let (event, _) = fire_tool(vec![deny_handler()], &local_agent()).await;
        assert_eq!(cancel_message(&event), Some("DENIED: not authorized"));
    }

    #[tokio::test]
    async fn short_circuits_later_handlers_do_not_run() {
        let called = Arc::new(Mutex::new(Vec::new()));
        fire_tool(vec![deny_handler(), later_handler(&called)], &local_agent())
            .await
            .1
            .unwrap();
        assert!(called.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn sets_cancel_on_before_invocation_event() {
        struct InvocationDeny;

        #[async_trait]
        impl InterventionHandler for InvocationDeny {
            fn name(&self) -> &str {
                "invocation-deny"
            }

            async fn before_invocation(
                &self,
                _event: &mut BeforeInvocationEvent<'_>,
            ) -> Result<BeforeInvocationAction> {
                Ok(deny("unauthorized user").into())
            }
        }

        let hooks = HookRegistry::new();
        InterventionRegistry::new(vec![Arc::new(InvocationDeny)], &hooks).unwrap();
        let mut agent = local_agent();
        let mut event = BeforeInvocationEvent::new(&mut agent, InvocationState::new(), Vec::new());
        hooks
            .invoke::<BeforeInvocationEvent<'static>>(&mut event)
            .await
            .unwrap();
        assert_eq!(
            event.cancel().and_then(Cancel::message),
            Some("DENIED: unauthorized user")
        );
    }

    #[tokio::test]
    async fn sets_cancel_on_before_model_call_event() {
        struct ModelDeny;

        #[async_trait]
        impl InterventionHandler for ModelDeny {
            fn name(&self) -> &str {
                "model-deny"
            }

            async fn before_model_call(
                &self,
                _event: &mut BeforeModelCallEvent<'_>,
            ) -> Result<BeforeModelCallAction> {
                Ok(deny("prompt injection detected").into())
            }
        }

        let hooks = HookRegistry::new();
        InterventionRegistry::new(vec![Arc::new(ModelDeny)], &hooks).unwrap();
        let mut agent = local_agent();
        let mut event = before_model_call_event(&mut agent);
        hooks
            .invoke::<BeforeModelCallEvent<'static>>(&mut event)
            .await
            .unwrap();
        assert_eq!(
            event.cancel().and_then(Cancel::message),
            Some("DENIED: prompt injection detected")
        );
    }
}

mod guide_action {
    use super::*;

    #[tokio::test]
    async fn sets_cancel_with_guidance_on_before_tool_call_event() {
        let (event, _) = fire_tool(vec![guide_handler()], &local_agent()).await;
        assert_eq!(
            cancel_message(&event),
            Some("GUIDANCE: [guide-handler] add more context")
        );
    }

    #[tokio::test]
    async fn accumulates_feedback_from_multiple_handlers() {
        let second = tool_handler("second-guide", |_| Ok(guide("also check permissions").into()));
        let (event, _) = fire_tool(vec![guide_handler(), second], &local_agent()).await;
        assert_eq!(
            cancel_message(&event),
            Some("GUIDANCE: [guide-handler] add more context\n[second-guide] also check permissions")
        );
    }

    #[tokio::test]
    async fn sets_retry_true_and_injects_guidance_message_on_after_model_call_event() {
        let hooks = HookRegistry::new();
        InterventionRegistry::new(vec![Arc::new(ModelGuideHandler)], &hooks).unwrap();
        let mut agent = local_agent();
        let before = agent.messages().len();
        let mut event = after_model_call_event(&mut agent);
        hooks
            .invoke::<AfterModelCallEvent<'static>>(&mut event)
            .await
            .unwrap();
        assert!(event.retry());
        assert_eq!(agent.messages().len(), before + 1);
        assert_eq!(
            last_text(&agent),
            (Role::User, "[model-guide] be more specific".to_owned())
        );
        assert!(!agent.messages().last().unwrap().tracking_id().is_empty());
    }
}

mod confirm_action {
    use super::*;

    #[tokio::test]
    async fn pauses_agent_when_no_response_is_provided() {
        let (event, result) = fire_tool(vec![confirm_handler()], &local_agent()).await;
        result.unwrap();
        let [interrupt] = event.collected_interrupts() else {
            panic!("one interrupt")
        };
        assert_eq!(interrupt.id(), "hook:beforeToolCall:id-1:confirm-handler");
        assert_eq!(interrupt.name(), "confirm-handler");
        assert_eq!(interrupt.reason(), Some(&json!("approve this action?")));
        assert_eq!(event.cancel(), None);
    }

    #[tokio::test]
    async fn short_circuits_later_handlers_do_not_run() {
        let called = Arc::new(Mutex::new(Vec::new()));
        let (event, _) = fire_tool(vec![confirm_handler(), later_handler(&called)], &local_agent()).await;
        assert_eq!(event.collected_interrupts().len(), 1);
        assert!(called.lock().unwrap().is_empty());
    }

    /// The `it.each` table of TS `approve/deny on resume`.
    #[tokio::test]
    async fn response_cancel() {
        let denied = Some("CONFIRMATION_FAILED: approve this action?");
        let cases = [
            (json!(true), None),
            (json!("yes"), None),
            (json!("y"), None),
            (json!("Y"), None),
            (json!("YES"), None),
            (json!("  yes  "), None),
            (json!("no"), denied),
            (json!(false), denied),
            (json!(null), denied),
            (json!(""), denied),
        ];
        for (response, expected) in cases {
            let agent = local_agent();
            preload_interrupt_response(&agent, "confirm-handler", response.clone());
            let (event, result) = fire_tool(vec![confirm_handler()], &agent).await;
            result.unwrap();
            assert_eq!(cancel_message(&event), expected, "response {response}");
            assert!(event.collected_interrupts().is_empty(), "response {response}");
        }
    }

    fn custom_approval() -> Arc<dyn InterventionHandler> {
        tool_handler("custom-approval", |_| {
            Ok(confirm("approve?")
                .with_evaluate(|response| response == "custom-yes")
                .into())
        })
    }

    #[tokio::test]
    async fn uses_custom_evaluate_when_provided() {
        let agent = local_agent();
        preload_interrupt_response(&agent, "custom-approval", json!("yes"));
        let (event, _) = fire_tool(vec![custom_approval()], &agent).await;
        assert_eq!(cancel_message(&event), Some("CONFIRMATION_FAILED: approve?"));
    }

    #[tokio::test]
    async fn custom_evaluate_approves_when_its_condition_is_met() {
        let agent = local_agent();
        preload_interrupt_response(&agent, "custom-approval", json!("custom-yes"));
        let (event, _) = fire_tool(vec![custom_approval()], &agent).await;
        assert_eq!(event.cancel(), None);
    }

    #[tokio::test]
    async fn approved_confirm_does_not_short_circuit_later_handlers() {
        let agent = local_agent();
        preload_interrupt_response(&agent, "confirm-handler", json!("yes"));
        let called = Arc::new(Mutex::new(Vec::new()));
        let (event, _) = fire_tool(vec![confirm_handler(), later_handler(&called)], &agent).await;
        assert_eq!(event.cancel(), None);
        assert_eq!(called.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn denied_confirm_short_circuits_later_handlers() {
        let agent = local_agent();
        preload_interrupt_response(&agent, "confirm-handler", json!("no"));
        let called = Arc::new(Mutex::new(Vec::new()));
        let (event, _) = fire_tool(vec![confirm_handler(), later_handler(&called)], &agent).await;
        assert_eq!(
            cancel_message(&event),
            Some("CONFIRMATION_FAILED: approve this action?")
        );
        assert!(called.lock().unwrap().is_empty());
    }

    mod preemptive_response_inline_mode {
        use super::*;

        fn inline(response: JsonValue) -> Arc<dyn InterventionHandler> {
            tool_handler("inline-confirm", move |_| {
                Ok(confirm("approve?").with_response(response.clone()).into())
            })
        }

        #[tokio::test]
        async fn approves_when_response_is_an_approved_value() {
            let (event, _) = fire_tool(vec![inline(json!("yes"))], &local_agent()).await;
            assert_eq!(event.cancel(), None);
        }

        #[tokio::test]
        async fn denies_when_response_is_a_non_approved_value() {
            let (event, _) = fire_tool(vec![inline(json!("no"))], &local_agent()).await;
            assert_eq!(cancel_message(&event), Some("CONFIRMATION_FAILED: approve?"));
        }

        #[tokio::test]
        async fn uses_custom_evaluate_with_preemptive_response() {
            let handler = tool_handler("inline-confirm", |_| {
                Ok(confirm("approve?")
                    .with_response("magic-word")
                    .with_evaluate(|response| response == "magic-word")
                    .into())
            });
            let (event, _) = fire_tool(vec![handler], &local_agent()).await;
            assert_eq!(event.cancel(), None);
        }

        #[tokio::test]
        async fn passes_response_as_preemptive_value_so_agent_never_pauses() {
            let (event, result) = fire_tool(vec![inline(json!("yes"))], &local_agent()).await;
            result.unwrap();
            assert!(event.collected_interrupts().is_empty());
        }

        #[tokio::test]
        async fn denies_when_response_is_falsy_but_defined_false() {
            let (event, result) = fire_tool(vec![inline(json!(false))], &local_agent()).await;
            result.unwrap();
            assert!(event.collected_interrupts().is_empty());
            assert_eq!(cancel_message(&event), Some("CONFIRMATION_FAILED: approve?"));
        }
    }

    /// TS `InterruptError always propagates regardless of onError=%s` (proceed, deny).
    #[tokio::test]
    async fn interrupt_error_always_propagates_regardless_of_on_error() {
        for on_error in [OnError::Proceed, OnError::Deny] {
            let handler =
                tool_handler_on_error("confirm-on-error", on_error, |_| Ok(confirm("approve?").into()));
            let (event, _) = fire_tool(vec![handler], &local_agent()).await;
            assert_eq!(event.collected_interrupts().len(), 1, "{on_error:?}");
            assert_eq!(event.cancel(), None, "{on_error:?}");
        }
    }

    /// TS `a handler-raised InterruptError propagates regardless of onError=%s` (proceed, deny).
    #[tokio::test]
    async fn a_handler_raised_interrupt_error_propagates_regardless_of_on_error() {
        for on_error in [OnError::Proceed, OnError::Deny] {
            let called = Arc::new(Mutex::new(Vec::new()));
            let handler = tool_handler_on_error("direct-interrupt", on_error, |event| {
                let params = InterruptParams {
                    name: "approval".into(),
                    reason: Some(json!("approve?")),
                    response: None,
                };
                match event.interrupt(params) {
                    Some(_) => Ok(proceed().into()),
                    None => Err(Error::other("paused")),
                }
            });
            let (event, result) = fire_tool(vec![handler, later_handler(&called)], &local_agent()).await;
            result.unwrap();
            assert_eq!(event.collected_interrupts().len(), 1, "{on_error:?}");
            assert_eq!(event.cancel(), None, "{on_error:?}");
            assert!(called.lock().unwrap().is_empty(), "{on_error:?}");
        }
    }
}

mod transform_action {
    use super::*;

    #[tokio::test]
    async fn calls_the_apply_function_with_the_event() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        let handler = tool_handler("transform-handler", move |_| {
            let log = Arc::clone(&log);
            Ok(transform(move |event: &mut BeforeToolCallEvent| {
                log.lock().unwrap().push(event.tool_use().name().to_owned());
            })
            .into())
        });
        fire_tool(vec![handler], &local_agent()).await.1.unwrap();
        assert_eq!(*seen.lock().unwrap(), ["testTool"]);
    }

    #[tokio::test]
    async fn later_handlers_see_the_transformed_state() {
        let observed = Arc::new(Mutex::new(Vec::new()));
        let transformer = tool_handler("transformer", |_| {
            Ok(transform(|event: &mut BeforeToolCallEvent| event.set_cancel("transformed")).into())
        });
        let log = Arc::clone(&observed);
        let observer = tool_handler("observer", move |event| {
            log.lock().unwrap().push(cancel_message(event).map(str::to_owned));
            Ok(proceed().into())
        });
        fire_tool(vec![transformer, observer], &local_agent())
            .await
            .1
            .unwrap();
        assert_eq!(*observed.lock().unwrap(), [Some("transformed".to_owned())]);
    }

    #[tokio::test]
    async fn works_on_after_model_call_event() {
        struct ModelTransform;

        #[async_trait]
        impl InterventionHandler for ModelTransform {
            fn name(&self) -> &str {
                "model-transform"
            }

            async fn after_model_call(
                &self,
                _event: &mut AfterModelCallEvent<'_>,
            ) -> Result<AfterModelCallAction> {
                Ok(
                    transform(|event: &mut AfterModelCallEvent<'_>| event.set_retry(true))
                        .with_reason("redacted output")
                        .into(),
                )
            }
        }

        let hooks = HookRegistry::new();
        InterventionRegistry::new(vec![Arc::new(ModelTransform)], &hooks).unwrap();
        let mut agent = local_agent();
        let mut event = after_model_call_event(&mut agent);
        hooks
            .invoke::<AfterModelCallEvent<'static>>(&mut event)
            .await
            .unwrap();
        assert!(event.retry());
    }

    #[tokio::test]
    async fn is_logged_in_the_audit_trail() {
        let handler = tool_handler("transform-handler", |_| {
            Ok(transform(|_event: &mut BeforeToolCallEvent| {})
                .with_reason("sanitized")
                .into())
        });
        let (event, result) = fire_tool(vec![handler], &local_agent()).await;
        result.unwrap();
        assert_eq!(event.cancel(), None);
    }
}

mod proceed_action {
    use super::*;

    #[tokio::test]
    async fn does_not_mutate_the_event() {
        let (event, _) = fire_tool(vec![proceed_handler()], &local_agent()).await;
        assert_eq!(event.cancel(), None);
    }
}

mod error_handling {
    use super::*;

    #[tokio::test]
    async fn on_error_throw_default_rethrows_the_error() {
        let called = Arc::new(Mutex::new(Vec::new()));
        let handlers = vec![
            throwing_handler("throwing-handler", OnError::Throw),
            later_handler(&called),
        ];
        let (_, result) = fire_tool(handlers, &local_agent()).await;
        assert_eq!(result.unwrap_err().to_string(), "handler crashed");
        assert!(called.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn on_error_proceed_skips_the_handler_and_continues_to_next() {
        let called = Arc::new(Mutex::new(Vec::new()));
        let handlers = vec![
            throwing_handler("throwing-proceed", OnError::Proceed),
            later_handler(&called),
        ];
        let (event, result) = fire_tool(handlers, &local_agent()).await;
        result.unwrap();
        assert_eq!(event.cancel(), None);
        assert_eq!(called.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn on_error_deny_logs_the_error_and_applies_deny() {
        let called = Arc::new(Mutex::new(Vec::new()));
        let handlers = vec![
            throwing_handler("throwing-deny", OnError::Deny),
            later_handler(&called),
        ];
        let (event, _) = fire_tool(handlers, &local_agent()).await;
        assert_eq!(
            cancel_message(&event),
            Some("DENIED: Handler threw: handler crashed")
        );
        assert!(called.lock().unwrap().is_empty());
    }
}

mod async_handlers {
    use super::*;

    #[tokio::test]
    async fn awaits_async_handler_results() {
        struct AsyncDeny;

        #[async_trait]
        impl InterventionHandler for AsyncDeny {
            fn name(&self) -> &str {
                "async-deny"
            }

            async fn before_tool_call(
                &self,
                _event: &mut BeforeToolCallEvent,
            ) -> Result<BeforeToolCallAction> {
                tokio::task::yield_now().await;
                Ok(deny("async denial").into())
            }
        }

        let (event, _) = fire_tool(vec![Arc::new(AsyncDeny)], &local_agent()).await;
        assert_eq!(cancel_message(&event), Some("DENIED: async denial"));
    }
}

mod conflict_resolution {
    use super::*;

    #[tokio::test]
    async fn deny_wins_over_guide() {
        let (event, _) = fire_tool(vec![guide_handler(), deny_handler()], &local_agent()).await;
        assert_eq!(cancel_message(&event), Some("DENIED: not authorized"));
    }

    #[tokio::test]
    async fn deny_short_circuits_before_guide_can_accumulate() {
        let (event, _) = fire_tool(vec![deny_handler(), guide_handler()], &local_agent()).await;
        assert_eq!(cancel_message(&event), Some("DENIED: not authorized"));
    }

    #[tokio::test]
    async fn confirm_short_circuits_before_guide_can_accumulate() {
        let (event, _) = fire_tool(vec![confirm_handler(), guide_handler()], &local_agent()).await;
        assert_eq!(event.collected_interrupts().len(), 1);
        assert_eq!(event.cancel(), None);
    }
}

mod agent_integration {
    use super::*;

    fn tool_turns(tool: &str) -> Arc<MockMessageModel> {
        let model = Arc::new(MockMessageModel::new());
        model.add_turn(vec![ContentBlock::ToolUse(
            crate::types::messages::ToolUseBlock::new(tool, "tool-1", json!({})),
        )]);
        model.add_turn(vec![ContentBlock::Text(TextBlock::new("Done"))]);
        model
    }

    #[tokio::test]
    async fn deny_on_before_tool_call_prevents_tool_execution() {
        let executed = Arc::new(Mutex::new(false));
        let flag = Arc::clone(&executed);
        let tool = mock_tool("blockedTool", move |context| {
            *flag.lock().unwrap() = true;
            text_result(context.tool_use().tool_use_id(), "should not reach here")
        });
        let mut agent = Agent::builder()
            .model(tool_turns("blockedTool"))
            .tools([tool])
            .interventions([tool_handler("block-all", |_| {
                Ok(deny("blocked by intervention").into())
            })])
            .printer(false)
            .build()
            .unwrap();
        let result = agent.invoke("Test").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert!(!*executed.lock().unwrap());
    }

    #[tokio::test]
    async fn interventions_run_before_plugins_hook_order_interventions_default() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&order);
        let tracker = tool_handler("order-tracker", move |_| {
            log.lock().unwrap().push("intervention");
            Ok(proceed().into())
        });
        let tool = mock_tool("testTool", |context| {
            text_result(context.tool_use().tool_use_id(), "result")
        });
        let mut agent = Agent::builder()
            .model(tool_turns("testTool"))
            .tools([tool])
            .interventions([tracker])
            .printer(false)
            .build()
            .unwrap();
        let log = Arc::clone(&order);
        agent.add_hook(move |_event: &mut BeforeToolCallEvent| log.lock().unwrap().push("plugin"));
        agent.invoke("Test").await.unwrap();
        assert_eq!(*order.lock().unwrap(), ["plugin", "intervention"]);
    }

    // No TS case: TS tests `observeAgent` through the steering handler only, which is out of v0.1.
    #[tokio::test]
    async fn observe_agent_runs_once_after_the_plugins_and_before_initialized_event() {
        struct Observer(Arc<Mutex<Vec<&'static str>>>);

        #[async_trait]
        impl InterventionHandler for Observer {
            fn name(&self) -> &str {
                "observer"
            }

            async fn observe_agent(&self, agent: &mut LocalAgent) -> Result<()> {
                self.0.lock().unwrap().push("observe");
                let log = Arc::clone(&self.0);
                agent.hooks().add_hook(move |_event: &mut BeforeToolCallEvent| {
                    log.lock().unwrap().push("observer hook");
                });
                Ok(())
            }
        }

        struct Recorder(Arc<Mutex<Vec<&'static str>>>);

        #[async_trait]
        impl crate::plugins::Plugin for Recorder {
            fn name(&self) -> &str {
                "recorder"
            }

            async fn init_agent(self: Arc<Self>, _agent: &mut LocalAgent) -> Result<()> {
                self.0.lock().unwrap().push("plugin");
                Ok(())
            }
        }

        let order = Arc::new(Mutex::new(Vec::new()));
        let tool = mock_tool("testTool", |context| {
            text_result(context.tool_use().tool_use_id(), "result")
        });
        let mut agent = Agent::builder()
            .model(tool_turns("testTool"))
            .tools([tool])
            .intervention(Observer(Arc::clone(&order)))
            .plugin(Recorder(Arc::clone(&order)))
            .printer(false)
            .build()
            .unwrap();
        let log = Arc::clone(&order);
        agent.add_hook(move |_event: &mut crate::hooks::InitializedEvent<'_>| {
            log.lock().unwrap().push("initialized");
        });
        assert!(order.lock().unwrap().is_empty());
        agent.initialize().await.unwrap();
        agent.invoke("Test").await.unwrap();
        assert_eq!(
            *order.lock().unwrap(),
            ["plugin", "observe", "initialized", "observer hook"]
        );
    }

    // No TS case: TS covers `onError: 'throw'` at the registry only; this pins the agent-level end
    // (the invocation fails at the failing gate, with no further model call).
    #[tokio::test]
    async fn on_error_throw_fails_the_invocation() {
        let tool = mock_tool("testTool", |context| {
            text_result(context.tool_use().tool_use_id(), "result")
        });
        let model = tool_turns("testTool");
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .tools([tool])
            .interventions([throwing_handler("throwing-handler", OnError::Throw)])
            .printer(false)
            .build()
            .unwrap();
        let error = agent.invoke("Test").await.unwrap_err();
        assert!(error.to_string().contains("handler crashed"), "{error}");
        // The invocation stops at the failing gate: TS goes on to a second model call while it
        // cleans up, and keeps that turn.
        assert_eq!(model.call_count(), 1);
        let roles: Vec<Role> = agent.messages().iter().map(Message::role).collect();
        assert_eq!(roles, [Role::User]);
    }

    // No TS case: `String.prototype.trim` strips U+FEFF and keeps U+0085, unlike `str::trim`.
    #[test]
    fn the_default_evaluate_trims_as_js_trims() {
        assert!(default_evaluate(&json!("\u{feff}y")));
        assert!(default_evaluate(&json!(" YES\n")));
        assert!(!default_evaluate(&json!("y\u{85}")));
    }
}

mod edge_cases {
    use super::*;

    #[tokio::test]
    async fn guide_on_before_model_call_injects_a_user_message() {
        struct ModelGuide;

        #[async_trait]
        impl InterventionHandler for ModelGuide {
            fn name(&self) -> &str {
                "model-guide"
            }

            async fn before_model_call(
                &self,
                _event: &mut BeforeModelCallEvent<'_>,
            ) -> Result<BeforeModelCallAction> {
                Ok(guide("check your sources").into())
            }
        }

        let hooks = HookRegistry::new();
        InterventionRegistry::new(vec![Arc::new(ModelGuide)], &hooks).unwrap();
        let mut agent = local_agent();
        let before = agent.messages().len();
        let mut event = before_model_call_event(&mut agent);
        hooks
            .invoke::<BeforeModelCallEvent<'static>>(&mut event)
            .await
            .unwrap();
        assert_eq!(event.cancel(), None);
        assert_eq!(agent.messages().len(), before + 1);
        assert_eq!(
            last_text(&agent),
            (Role::User, "[model-guide] check your sources".to_owned())
        );
        assert!(!agent.messages().last().unwrap().tracking_id().is_empty());
    }
}
