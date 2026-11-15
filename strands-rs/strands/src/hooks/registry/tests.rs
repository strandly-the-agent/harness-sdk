//! Ported from strands-ts/src/hooks/__tests__/registry.test.ts

use std::time::Duration;

use super::*;
use crate::agent::invocation_state::InvocationState;
use crate::hooks::events::Cancel;
use crate::hooks::fixtures::{Calls, before_tool_call, mock_local_agent};
use crate::interrupt::InterruptParams;
use crate::types::json::JsonValue;

async fn fire_before_invocation(registry: &HookRegistry) {
    let mut agent = mock_local_agent();
    let mut event = BeforeInvocationEvent::new(&mut agent, InvocationState::new(), None, Vec::new());
    registry
        .invoke::<BeforeInvocationEvent<'static>>(&mut event)
        .await;
}

async fn fire_after_invocation(registry: &HookRegistry) {
    let mut agent = mock_local_agent();
    let mut event = AfterInvocationEvent::new(&mut agent, InvocationState::new(), None);
    registry.invoke::<AfterInvocationEvent<'static>>(&mut event).await;
}

fn push_on_before(registry: &HookRegistry, calls: &Calls, order: i32, label: &str) -> HookCleanup {
    let (calls, label) = (calls.clone(), label.to_owned());
    registry.add_hook_with(
        HookOptions {
            order: HookOrder(order),
        },
        move |_: &mut BeforeInvocationEvent<'_>| calls.push(label.as_str()),
    )
}

fn push_on_after(registry: &HookRegistry, calls: &Calls, order: i32, label: &str) {
    let (calls, label) = (calls.clone(), label.to_owned());
    registry.add_hook_with(
        HookOptions {
            order: HookOrder(order),
        },
        move |_: &mut AfterInvocationEvent<'_>| calls.push(label.as_str()),
    );
}

fn interrupt_on_tool_call(registry: &HookRegistry, calls: &Calls, label: &str, name: Option<&str>) {
    let (calls, label, name) = (calls.clone(), label.to_owned(), name.map(str::to_owned));
    registry.add_hook(move |event: &mut BeforeToolCallEvent| {
        calls.push(label.as_str());
        if let Some(name) = &name {
            let _ = event.interrupt(InterruptParams {
                reason: Some(JsonValue::from(format!("Reason {name}"))),
                ..InterruptParams::new(name.as_str())
            });
        }
    });
}

#[tokio::test]
async fn registers_callback_for_event_type() {
    let registry = HookRegistry::new();
    let calls = Calls::default();
    push_on_before(&registry, &calls, 0, "called");
    fire_before_invocation(&registry).await;
    assert_eq!(calls.get(), ["called"]);
}

#[tokio::test]
async fn registers_multiple_callbacks_for_same_event_type() {
    let registry = HookRegistry::new();
    let calls = Calls::default();
    push_on_before(&registry, &calls, 0, "1");
    push_on_before(&registry, &calls, 0, "2");
    fire_before_invocation(&registry).await;
    assert_eq!(calls.get(), ["1", "2"]);
}

#[tokio::test]
async fn registers_callbacks_for_different_event_types_separately() {
    let registry = HookRegistry::new();
    let calls = Calls::default();
    push_on_before(&registry, &calls, 0, "before");
    push_on_after(&registry, &calls, 0, "after");
    fire_before_invocation(&registry).await;
    assert_eq!(calls.get(), ["before"]);
    fire_after_invocation(&registry).await;
    assert_eq!(calls.get(), ["before", "after"]);
}

#[tokio::test]
async fn calls_registered_callbacks_in_order() {
    let registry = HookRegistry::new();
    let calls = Calls::default();
    push_on_before(&registry, &calls, 0, "1");
    push_on_before(&registry, &calls, 0, "2");
    fire_before_invocation(&registry).await;
    assert_eq!(calls.get(), ["1", "2"]);
}

#[tokio::test]
async fn reverses_callback_order_for_after_events() {
    let registry = HookRegistry::new();
    let calls = Calls::default();
    push_on_after(&registry, &calls, 0, "1");
    push_on_after(&registry, &calls, 0, "2");
    fire_after_invocation(&registry).await;
    assert_eq!(calls.get(), ["2", "1"]);
}

#[tokio::test(start_paused = true)]
async fn awaits_async_callbacks() {
    let registry = HookRegistry::new();
    let calls = Calls::default();
    let recorded = calls.clone();
    registry.add_async_hook::<BeforeInvocationEvent<'_>>(move |_| {
        let calls = recorded.clone();
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            calls.push("completed");
        })
    });
    fire_before_invocation(&registry).await;
    assert_eq!(calls.get(), ["completed"]);
}

#[tokio::test(start_paused = true)]
async fn handles_mixed_sync_and_async_callbacks() {
    let registry = HookRegistry::new();
    let calls = Calls::default();
    push_on_before(&registry, &calls, 0, "sync");
    let recorded = calls.clone();
    registry.add_async_hook::<BeforeInvocationEvent<'_>>(move |_| {
        let calls = recorded.clone();
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            calls.push("async");
        })
    });
    fire_before_invocation(&registry).await;
    assert_eq!(calls.get(), ["sync", "async"]);
}

#[tokio::test]
async fn returns_the_event_after_invocation() {
    let registry = HookRegistry::new();
    registry.add_hook(|event: &mut BeforeInvocationEvent<'_>| {
        event.set_cancel(Some(Cancel::new()));
    });
    let mut agent = mock_local_agent();
    let mut event = BeforeInvocationEvent::new(&mut agent, InvocationState::new(), None, Vec::new());
    registry
        .invoke::<BeforeInvocationEvent<'static>>(&mut event)
        .await;
    assert_eq!(event.cancel(), Some(&Cancel::new()));
}

#[tokio::test]
async fn lower_order_runs_first() {
    let registry = HookRegistry::new();
    let calls = Calls::default();
    push_on_before(&registry, &calls, 0, "0");
    push_on_before(&registry, &calls, 100, "100");
    push_on_before(&registry, &calls, -100, "-100");
    fire_before_invocation(&registry).await;
    assert_eq!(calls.get(), ["-100", "0", "100"]);
}

#[tokio::test]
async fn same_order_preserves_registration_order() {
    let registry = HookRegistry::new();
    let calls = Calls::default();
    for label in ["first", "second", "third"] {
        push_on_before(&registry, &calls, 10, label);
    }
    fire_before_invocation(&registry).await;
    assert_eq!(calls.get(), ["first", "second", "third"]);
}

#[tokio::test]
async fn negative_order_runs_before_default() {
    let registry = HookRegistry::new();
    let calls = Calls::default();
    push_on_before(&registry, &calls, 0, "default");
    push_on_before(&registry, &calls, -100, "early");
    fire_before_invocation(&registry).await;
    assert_eq!(calls.get(), ["early", "default"]);
}

#[tokio::test]
async fn after_events_lower_order_still_runs_first_across_groups() {
    let registry = HookRegistry::new();
    let calls = Calls::default();
    push_on_after(&registry, &calls, -100, "early");
    push_on_after(&registry, &calls, 100, "late");
    fire_after_invocation(&registry).await;
    assert_eq!(calls.get(), ["early", "late"]);
}

#[tokio::test]
async fn after_events_reverse_only_within_one_order() {
    let registry = HookRegistry::new();
    let calls = Calls::default();
    push_on_after(&registry, &calls, 0, "a");
    push_on_after(&registry, &calls, -10, "early");
    push_on_after(&registry, &calls, 0, "b");
    push_on_after(&registry, &calls, 10, "late");
    fire_after_invocation(&registry).await;
    assert_eq!(calls.get(), ["early", "b", "a", "late"]);
}

#[tokio::test]
async fn returns_cleanup_function_that_removes_the_callback() {
    let registry = HookRegistry::new();
    let calls = Calls::default();
    push_on_before(&registry, &calls, 0, "called").remove();
    fire_before_invocation(&registry).await;
    assert!(calls.get().is_empty());
}

#[tokio::test]
async fn cleanup_function_is_idempotent() {
    let registry = HookRegistry::new();
    let calls = Calls::default();
    let cleanup = push_on_before(&registry, &calls, 0, "called");
    cleanup.remove();
    cleanup.remove();
    cleanup.remove();
    fire_before_invocation(&registry).await;
    assert!(calls.get().is_empty());
}

#[tokio::test]
async fn cleanup_function_does_not_affect_other_callbacks() {
    let registry = HookRegistry::new();
    let calls = Calls::default();
    let cleanup = push_on_before(&registry, &calls, 0, "1");
    push_on_before(&registry, &calls, 0, "2");
    cleanup.remove();
    fire_before_invocation(&registry).await;
    assert_eq!(calls.get(), ["2"]);
}

#[tokio::test]
async fn allows_callback_to_be_re_registered_after_cleanup() {
    let registry = HookRegistry::new();
    let calls = Calls::default();
    let recorded = calls.clone();
    let callback = move |_: &mut BeforeInvocationEvent<'_>| recorded.push("called");
    registry.add_hook(callback.clone()).remove();
    registry.add_hook(callback);
    fire_before_invocation(&registry).await;
    assert_eq!(calls.get(), ["called"]);
}

#[tokio::test]
async fn cleanup_from_one_registration_does_not_affect_independent_registration_of_same_function() {
    let registry = HookRegistry::new();
    let calls = Calls::default();
    let recorded = calls.clone();
    let callback = move |_: &mut BeforeInvocationEvent<'_>| recorded.push("called");
    registry.add_hook(callback.clone());
    registry.add_hook(callback).remove();
    fire_before_invocation(&registry).await;
    assert_eq!(calls.get(), ["called"]);
}

#[tokio::test]
async fn cleanup_after_the_registry_is_gone_does_nothing() {
    let registry = HookRegistry::new();
    let cleanup = registry.add_hook(|_: &mut BeforeToolCallEvent| {});
    drop(registry);
    cleanup.remove();
}

#[tokio::test]
async fn a_hook_may_register_hooks_while_the_event_fires() {
    let registry = Arc::new(HookRegistry::new());
    let calls = Calls::default();
    let (inner, recorded) = (Arc::clone(&registry), calls.clone());
    registry.add_hook(move |_: &mut BeforeToolCallEvent| {
        recorded.push("outer");
        let recorded = recorded.clone();
        inner.add_hook(move |_: &mut BeforeToolCallEvent| recorded.push("inner"));
    });
    registry
        .invoke::<BeforeToolCallEvent>(&mut before_tool_call())
        .await;
    assert_eq!(calls.get(), ["outer"]);
    registry
        .invoke::<BeforeToolCallEvent>(&mut before_tool_call())
        .await;
    assert_eq!(calls.get(), ["outer", "outer", "inner"]);
}

#[tokio::test]
async fn collects_interrupt_errors_from_multiple_callbacks_and_invokes_all_of_them() {
    let registry = HookRegistry::new();
    let calls = Calls::default();
    interrupt_on_tool_call(&registry, &calls, "1", Some("interrupt_a"));
    interrupt_on_tool_call(&registry, &calls, "2", Some("interrupt_b"));
    let mut event = before_tool_call();
    registry.invoke::<BeforeToolCallEvent>(&mut event).await;
    assert_eq!(calls.get(), ["1", "2"]);
    let names: Vec<String> = event
        .take_interrupts()
        .unwrap()
        .into_iter()
        .map(|i| i.name)
        .collect();
    assert_eq!(names, ["interrupt_a", "interrupt_b"]);
}

#[tokio::test]
async fn throws_interrupt_error_with_all_collected_interrupts_after_all_callbacks_run() {
    let registry = HookRegistry::new();
    let calls = Calls::default();
    interrupt_on_tool_call(&registry, &calls, "1", Some("first"));
    interrupt_on_tool_call(&registry, &calls, "2", Some("second"));
    let mut event = before_tool_call();
    registry.invoke::<BeforeToolCallEvent>(&mut event).await;
    let interrupts = event.take_interrupts().unwrap();
    let ids: Vec<&str> = interrupts.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "hook:beforeToolCall:tool-1:first",
            "hook:beforeToolCall:tool-1:second"
        ]
    );
    assert_eq!(interrupts[0].reason, Some(JsonValue::from("Reason first")));
    assert!(event.take_interrupts().unwrap().is_empty());
}

#[tokio::test]
async fn runs_all_callbacks_when_only_some_raise_interrupts() {
    let registry = HookRegistry::new();
    let calls = Calls::default();
    interrupt_on_tool_call(&registry, &calls, "first", Some("interrupt_a"));
    interrupt_on_tool_call(&registry, &calls, "second-no-interrupt", None);
    interrupt_on_tool_call(&registry, &calls, "third", Some("interrupt_b"));
    let mut event = before_tool_call();
    registry.invoke::<BeforeToolCallEvent>(&mut event).await;
    assert_eq!(calls.get(), ["first", "second-no-interrupt", "third"]);
    assert_eq!(event.take_interrupts().unwrap().len(), 2);
}

#[tokio::test]
async fn throws_when_two_callbacks_use_the_same_interrupt_name() {
    let registry = HookRegistry::new();
    let calls = Calls::default();
    interrupt_on_tool_call(&registry, &calls, "1", Some("confirm"));
    interrupt_on_tool_call(&registry, &calls, "2", Some("confirm"));
    let mut event = before_tool_call();
    registry.invoke::<BeforeToolCallEvent>(&mut event).await;
    assert_eq!(
        event.take_interrupts().unwrap_err().to_string(),
        "interrupt_names=<confirm> | duplicate interrupt names"
    );
}

#[tokio::test]
async fn reports_all_duplicate_interrupt_names_in_error() {
    let registry = HookRegistry::new();
    let calls = Calls::default();
    for name in ["alpha", "alpha", "beta", "beta"] {
        interrupt_on_tool_call(&registry, &calls, name, Some(name));
    }
    let mut event = before_tool_call();
    registry.invoke::<BeforeToolCallEvent>(&mut event).await;
    assert_eq!(
        event.take_interrupts().unwrap_err().to_string(),
        "interrupt_names=<alpha, beta> | duplicate interrupt names"
    );
}

struct Counter(Calls);

#[async_trait::async_trait]
impl<'x> Hook<BeforeInvocationEvent<'x>> for Counter {
    async fn handle(&self, event: &mut BeforeInvocationEvent<'x>) {
        tokio::task::yield_now().await;
        self.0.push(format!("impl {}", event.agent().name()));
    }
}

struct ToolAudit(Calls);

#[async_trait::async_trait]
impl Hook<BeforeToolCallEvent> for ToolAudit {
    async fn handle(&self, event: &mut BeforeToolCallEvent) {
        self.0.push(format!("audit {}", event.tool_use().name));
    }
}

#[tokio::test]
async fn hook_impls_infer_their_event_and_honour_the_order() {
    let registry = HookRegistry::new();
    let calls = Calls::default();
    push_on_before(&registry, &calls, 0, "sync");
    registry.add_hook_impl_with(
        HookOptions {
            order: HookOrder::SDK_FIRST,
        },
        Counter(calls.clone()),
    );
    let cleanup = registry.add_hook_impl(ToolAudit(calls.clone()));
    fire_before_invocation(&registry).await;
    registry
        .invoke::<BeforeToolCallEvent>(&mut before_tool_call())
        .await;
    cleanup.remove();
    registry
        .invoke::<BeforeToolCallEvent>(&mut before_tool_call())
        .await;
    assert_eq!(calls.get(), ["impl Strands Agent", "sync", "audit test"]);
}

#[tokio::test]
async fn async_hooks_may_change_the_borrowed_agent() {
    let registry = HookRegistry::new();
    registry.add_async_hook_with::<BeforeInvocationEvent<'_>>(
        HookOptions {
            order: HookOrder::SDK_LAST,
        },
        |event| {
            Box::pin(async move {
                tokio::task::yield_now().await;
                let count = event.agent().messages().len();
                event.agent_mut().app_state().set("seen", count).unwrap();
            })
        },
    );
    let mut agent = mock_local_agent();
    let mut event = BeforeInvocationEvent::new(&mut agent, InvocationState::new(), None, Vec::new());
    registry
        .invoke::<BeforeInvocationEvent<'static>>(&mut event)
        .await;
    assert_eq!(agent.app_state().get::<usize>("seen").unwrap(), Some(0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn per_tool_events_fire_on_spawned_tasks() {
    let registry = Arc::new(HookRegistry::new());
    registry.add_hook(|event: &mut BeforeToolCallEvent| event.set_cancel(Some("no".into())));
    let spawned = Arc::clone(&registry);
    let cancel = tokio::spawn(async move {
        let mut event = before_tool_call();
        spawned.invoke::<BeforeToolCallEvent>(&mut event).await;
        event.cancel().cloned()
    })
    .await
    .unwrap();
    assert_eq!(cancel, Some(Cancel::from("no")));
}

#[test]
fn hook_order_presets_match_ts() {
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
    assert_eq!(HookOrder::SDK_LAST + 1, HookOrder(101));
    assert_eq!(HookOrder(i32::MIN) - 1, HookOrder(i32::MIN));
}

#[test]
fn the_registry_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>(_: &T) {}
    let registry = HookRegistry::new();
    assert_send_sync(&registry);
    assert_send_sync(&registry.add_hook(|_: &mut BeforeToolCallEvent| {}));
}
