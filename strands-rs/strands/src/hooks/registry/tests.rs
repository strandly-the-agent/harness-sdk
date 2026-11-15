//! Ported from strands-ts/src/hooks/__tests__/registry.test.ts.
//!
//! TS cases about callbacks that throw (`propagates callback errors`, `stops execution on first
//! non-interrupt error`, `stops on non-interrupt error even when interrupts were collected`) have
//! no Rust form: hook callbacks return `()` and can't fail.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::*;
use crate::hooks::events::{AfterInvocationEvent, BeforeInvocationEvent, BeforeToolCallEvent};
use crate::interrupt::InterruptParams;
use crate::testing::MockMessageModel;
use crate::types::agent::{InvocationState, LocalAgent};
use crate::types::tools::ToolUse;

type Log = Arc<Mutex<Vec<String>>>;

fn local_agent() -> LocalAgent {
    LocalAgent::new(Arc::new(MockMessageModel::new()))
}

fn log() -> Log {
    Arc::new(Mutex::new(Vec::new()))
}

fn push(log: &Log, entry: &str) {
    log.lock().unwrap().push(entry.to_owned());
}

fn entries(log: &Log) -> Vec<String> {
    log.lock().unwrap().clone()
}

fn add_logging<M: HookEvent, F: IntoSyncHook<M>>(
    registry: &HookRegistry,
    order: i32,
    make: impl FnOnce() -> F,
) {
    registry.add_hook_with(
        HookCallbackOptions {
            order: HookOrder(order),
        },
        make(),
    );
}

async fn fire_before_invocation(registry: &HookRegistry, agent: &mut LocalAgent) {
    let mut event = BeforeInvocationEvent::new(agent, InvocationState::new(), Vec::new());
    registry
        .invoke::<BeforeInvocationEvent<'static>>(&mut event)
        .await
        .unwrap();
}

async fn fire_after_invocation(registry: &HookRegistry, agent: &mut LocalAgent) {
    let mut event = AfterInvocationEvent::new(agent, InvocationState::new());
    registry
        .invoke::<AfterInvocationEvent<'static>>(&mut event)
        .await
        .unwrap();
}

fn tool_call_event(agent: &LocalAgent) -> BeforeToolCallEvent {
    BeforeToolCallEvent::new(
        agent.tool_agent(),
        ToolUse::new("testTool", "test-id", json!({})),
        None,
        InvocationState::new(),
    )
}

mod add_callback {
    use super::*;

    #[tokio::test]
    async fn registers_callback_for_event_type() {
        let registry = HookRegistry::new();
        let calls = log();
        let c = Arc::clone(&calls);
        registry.add_hook(move |_: &mut BeforeInvocationEvent<'_>| push(&c, "called"));
        fire_before_invocation(&registry, &mut local_agent()).await;
        assert_eq!(entries(&calls), ["called"]);
    }

    #[tokio::test]
    async fn registers_multiple_callbacks_for_same_event_type() {
        let registry = HookRegistry::new();
        let calls = log();
        for name in ["one", "two"] {
            let c = Arc::clone(&calls);
            registry.add_hook(move |_: &mut BeforeInvocationEvent<'_>| push(&c, name));
        }
        fire_before_invocation(&registry, &mut local_agent()).await;
        assert_eq!(entries(&calls), ["one", "two"]);
    }

    #[tokio::test]
    async fn registers_callbacks_for_different_event_types_separately() {
        let registry = HookRegistry::new();
        let calls = log();
        let c = Arc::clone(&calls);
        registry.add_hook(move |_: &mut BeforeInvocationEvent<'_>| push(&c, "before"));
        let c = Arc::clone(&calls);
        registry.add_hook(move |_: &mut AfterInvocationEvent<'_>| push(&c, "after"));
        let mut agent = local_agent();
        fire_before_invocation(&registry, &mut agent).await;
        assert_eq!(entries(&calls), ["before"]);
        fire_after_invocation(&registry, &mut agent).await;
        assert_eq!(entries(&calls), ["before", "after"]);
    }
}

mod invoke_callbacks {
    use super::*;

    #[tokio::test]
    async fn calls_registered_callbacks_in_order() {
        let registry = HookRegistry::new();
        let calls = log();
        for name in ["first", "second", "third"] {
            let c = Arc::clone(&calls);
            registry.add_hook(move |_: &mut BeforeInvocationEvent<'_>| push(&c, name));
        }
        fire_before_invocation(&registry, &mut local_agent()).await;
        assert_eq!(entries(&calls), ["first", "second", "third"]);
    }

    #[tokio::test]
    async fn reverses_callback_order_for_after_events() {
        let registry = HookRegistry::new();
        let calls = log();
        for name in ["first", "second", "third"] {
            let c = Arc::clone(&calls);
            registry.add_hook(move |_: &mut AfterInvocationEvent<'_>| push(&c, name));
        }
        fire_after_invocation(&registry, &mut local_agent()).await;
        assert_eq!(entries(&calls), ["third", "second", "first"]);
    }

    #[tokio::test]
    async fn awaits_async_callbacks() {
        let registry = HookRegistry::new();
        let calls = log();
        let c = Arc::clone(&calls);
        registry.add_async_hook::<BeforeInvocationEvent<'_>>(move |_| {
            let c = Arc::clone(&c);
            Box::pin(async move {
                tokio::task::yield_now().await;
                push(&c, "async");
            })
        });
        fire_before_invocation(&registry, &mut local_agent()).await;
        assert_eq!(entries(&calls), ["async"]);
    }

    #[tokio::test]
    async fn handles_mixed_sync_and_async_callbacks() {
        let registry = HookRegistry::new();
        let calls = log();
        let c = Arc::clone(&calls);
        registry.add_hook(move |_: &mut BeforeInvocationEvent<'_>| push(&c, "sync"));
        let c = Arc::clone(&calls);
        registry.add_async_hook::<BeforeInvocationEvent<'_>>(move |_| {
            let c = Arc::clone(&c);
            Box::pin(async move {
                tokio::task::yield_now().await;
                push(&c, "async");
            })
        });
        let c = Arc::clone(&calls);
        registry.add_hook(move |_: &mut BeforeInvocationEvent<'_>| push(&c, "sync2"));
        fire_before_invocation(&registry, &mut local_agent()).await;
        assert_eq!(entries(&calls), ["sync", "async", "sync2"]);
    }

    #[tokio::test]
    async fn returns_the_event_after_invocation() {
        let registry = HookRegistry::new();
        registry.add_hook(|event: &mut BeforeInvocationEvent<'_>| event.set_cancel("stop"));
        let mut agent = local_agent();
        let mut event = BeforeInvocationEvent::new(&mut agent, InvocationState::new(), Vec::new());
        registry
            .invoke::<BeforeInvocationEvent<'static>>(&mut event)
            .await
            .unwrap();
        assert_eq!(event.cancel().and_then(|c| c.message()), Some("stop"));
    }

    #[tokio::test]
    async fn hook_impls_receive_the_borrowed_agent() {
        struct Rename;

        #[async_trait::async_trait]
        impl<'x> Hook<BeforeInvocationEvent<'x>> for Rename {
            async fn handle(&self, event: &mut BeforeInvocationEvent<'x>) {
                event.agent_mut().set_system_prompt(Some("from impl".into()));
            }
        }

        let registry = HookRegistry::new();
        registry.add_hook_impl::<BeforeInvocationEvent<'_>>(Rename);
        let mut agent = local_agent();
        fire_before_invocation(&registry, &mut agent).await;
        assert_eq!(agent.system_prompt(), Some(&"from impl".into()));
    }
}

mod ordering {
    use super::*;

    #[tokio::test]
    async fn lower_order_runs_first() {
        let registry = HookRegistry::new();
        let calls = log();
        for (name, order) in [("late", 100), ("default", 0), ("early", -100)] {
            let c = Arc::clone(&calls);
            add_logging(&registry, order, || {
                move |_: &mut BeforeInvocationEvent<'_>| push(&c, name)
            });
        }
        fire_before_invocation(&registry, &mut local_agent()).await;
        assert_eq!(entries(&calls), ["early", "default", "late"]);
    }

    #[tokio::test]
    async fn same_order_preserves_registration_order() {
        let registry = HookRegistry::new();
        let calls = log();
        for (name, order) in [("a", 10), ("b", 10), ("early", -5), ("c", 10)] {
            let c = Arc::clone(&calls);
            add_logging(&registry, order, || {
                move |_: &mut BeforeInvocationEvent<'_>| push(&c, name)
            });
        }
        fire_before_invocation(&registry, &mut local_agent()).await;
        assert_eq!(entries(&calls), ["early", "a", "b", "c"]);
    }

    #[tokio::test]
    async fn negative_order_runs_before_default() {
        let registry = HookRegistry::new();
        let calls = log();
        let c = Arc::clone(&calls);
        registry.add_hook(move |_: &mut BeforeInvocationEvent<'_>| push(&c, "default"));
        let c = Arc::clone(&calls);
        add_logging(&registry, -1, || {
            move |_: &mut BeforeInvocationEvent<'_>| push(&c, "negative")
        });
        fire_before_invocation(&registry, &mut local_agent()).await;
        assert_eq!(entries(&calls), ["negative", "default"]);
    }

    #[tokio::test]
    async fn after_events_lower_order_still_runs_first_across_groups() {
        let registry = HookRegistry::new();
        let calls = log();
        for (name, order) in [("early", -100), ("late", 100)] {
            let c = Arc::clone(&calls);
            add_logging(&registry, order, || {
                move |_: &mut AfterInvocationEvent<'_>| push(&c, name)
            });
        }
        fire_after_invocation(&registry, &mut local_agent()).await;
        assert_eq!(entries(&calls), ["early", "late"]);
    }

    #[test]
    fn hook_order_constants_match_ts() {
        assert_eq!(
            [
                HookOrder::SDK_FIRST,
                HookOrder::INTERVENTION_OUTPUT,
                HookOrder::DEFAULT,
                HookOrder::MODEL_ROUTING,
                HookOrder::INTERVENTION_INPUT,
                HookOrder::SDK_LAST,
            ],
            [-100, -90, 0, 50, 90, 100].map(HookOrder)
        );
        assert_eq!(HookOrder::SDK_FIRST - 1, HookOrder(-101));
        assert_eq!(HookOrder::SDK_LAST + 1, HookOrder(101));
    }
}

mod add_callback_cleanup_function {
    use super::*;

    #[tokio::test]
    async fn returns_cleanup_function_that_removes_the_callback() {
        let registry = HookRegistry::new();
        let calls = log();
        let c = Arc::clone(&calls);
        let cleanup = registry.add_hook(move |_: &mut BeforeInvocationEvent<'_>| push(&c, "x"));
        cleanup.remove();
        fire_before_invocation(&registry, &mut local_agent()).await;
        assert!(entries(&calls).is_empty());
        assert!(registry.is_empty());
    }

    #[tokio::test]
    async fn cleanup_function_does_not_affect_other_callbacks() {
        let registry = HookRegistry::new();
        let calls = log();
        let c = Arc::clone(&calls);
        let cleanup = registry.add_hook(move |_: &mut BeforeInvocationEvent<'_>| push(&c, "a"));
        let c = Arc::clone(&calls);
        registry.add_hook(move |_: &mut BeforeInvocationEvent<'_>| push(&c, "b"));
        cleanup.remove();
        fire_before_invocation(&registry, &mut local_agent()).await;
        assert_eq!(entries(&calls), ["b"]);
    }

    #[tokio::test]
    async fn allows_callback_to_be_re_registered_after_cleanup() {
        let registry = HookRegistry::new();
        let calls = log();
        let c = Arc::clone(&calls);
        let callback = move |_: &mut BeforeInvocationEvent<'_>| push(&c, "x");
        registry.add_hook(callback.clone()).remove();
        registry.add_hook(callback);
        fire_before_invocation(&registry, &mut local_agent()).await;
        assert_eq!(entries(&calls), ["x"]);
    }

    #[tokio::test]
    async fn cleanup_from_one_registration_does_not_affect_independent_registration_of_same_function() {
        let registry = HookRegistry::new();
        let calls = log();
        let c = Arc::clone(&calls);
        let callback = move |_: &mut BeforeInvocationEvent<'_>| push(&c, "x");
        let first = registry.add_hook(callback.clone());
        registry.add_hook(callback);
        first.remove();
        fire_before_invocation(&registry, &mut local_agent()).await;
        assert_eq!(entries(&calls), ["x"]);
    }

    #[test]
    fn cleanup_after_the_registry_is_dropped_is_a_no_op() {
        let registry = HookRegistry::new();
        let cleanup = registry.add_hook(|_: &mut BeforeInvocationEvent<'_>| {});
        drop(registry);
        cleanup.remove();
    }
}

mod interrupt_error_collection {
    use super::*;

    #[tokio::test]
    async fn collects_interrupt_errors_from_multiple_callbacks_and_invokes_all_of_them() {
        let registry = HookRegistry::new();
        let agent = local_agent();
        registry.add_hook(|event: &mut BeforeToolCallEvent| {
            event.interrupt(InterruptParams::new("first"));
        });
        registry.add_hook(|event: &mut BeforeToolCallEvent| {
            event.interrupt(InterruptParams::new("second"));
        });
        let mut event = tool_call_event(&agent);
        registry.invoke::<BeforeToolCallEvent>(&mut event).await.unwrap();
        let names: Vec<&str> = event.collected_interrupts().iter().map(Interrupt::name).collect();
        assert_eq!(names, ["first", "second"]);
        let state = agent.interrupt_state().lock();
        let ids: Vec<&String> = state.interrupts.keys().collect();
        assert_eq!(
            ids,
            [
                "hook:beforeToolCall:test-id:first",
                "hook:beforeToolCall:test-id:second"
            ]
        );
    }

    #[tokio::test]
    async fn runs_all_callbacks_when_only_some_raise_interrupts() {
        let registry = HookRegistry::new();
        let calls = log();
        let agent = local_agent();
        let c = Arc::clone(&calls);
        registry.add_hook(move |event: &mut BeforeToolCallEvent| {
            push(&c, "first");
            event.interrupt(InterruptParams::new("first"));
        });
        let c = Arc::clone(&calls);
        registry.add_hook(move |_: &mut BeforeToolCallEvent| push(&c, "second-no-interrupt"));
        let c = Arc::clone(&calls);
        registry.add_hook(move |event: &mut BeforeToolCallEvent| {
            push(&c, "third");
            event.interrupt(InterruptParams::new("third"));
        });
        let mut event = tool_call_event(&agent);
        registry.invoke::<BeforeToolCallEvent>(&mut event).await.unwrap();
        assert_eq!(entries(&calls), ["first", "second-no-interrupt", "third"]);
        assert_eq!(event.collected_interrupts().len(), 2);
    }

    #[tokio::test]
    async fn a_stored_response_is_returned_instead_of_raising() {
        let registry = HookRegistry::new();
        let agent = local_agent();
        let seen = Arc::new(Mutex::new(None));
        let s = Arc::clone(&seen);
        registry.add_hook(move |event: &mut BeforeToolCallEvent| {
            *s.lock().unwrap() = event.interrupt(InterruptParams::new("confirm"));
        });
        agent.interrupt_state().lock().interrupts.insert(
            "hook:beforeToolCall:test-id:confirm".into(),
            Interrupt::new("hook:beforeToolCall:test-id:confirm", "confirm").with_response(json!("yes")),
        );
        let mut event = tool_call_event(&agent);
        registry.invoke::<BeforeToolCallEvent>(&mut event).await.unwrap();
        assert_eq!(*seen.lock().unwrap(), Some(json!("yes")));
        assert!(event.collected_interrupts().is_empty());
    }

    #[tokio::test]
    async fn throws_when_two_callbacks_use_the_same_interrupt_name() {
        let registry = HookRegistry::new();
        let agent = local_agent();
        for reason in ["First", "Second"] {
            registry.add_hook(move |event: &mut BeforeToolCallEvent| {
                event.interrupt(InterruptParams {
                    reason: Some(json!(reason)),
                    ..InterruptParams::new("confirm")
                });
            });
        }
        let mut event = tool_call_event(&agent);
        let error = registry
            .invoke::<BeforeToolCallEvent>(&mut event)
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "interrupt_names=<confirm> | duplicate interrupt names"
        );
    }

    #[tokio::test]
    async fn reports_all_duplicate_interrupt_names_in_error() {
        let registry = HookRegistry::new();
        let agent = local_agent();
        for name in ["alpha", "alpha", "beta", "beta"] {
            registry.add_hook(move |event: &mut BeforeToolCallEvent| {
                event.interrupt(InterruptParams::new(name));
            });
        }
        let mut event = tool_call_event(&agent);
        let error = registry
            .invoke::<BeforeToolCallEvent>(&mut event)
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "interrupt_names=<alpha, beta> | duplicate interrupt names"
        );
    }
}
