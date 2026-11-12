//! Ported from strands-ts/src/hooks/__tests__/registry.test.ts
//!
//! Rust hook callbacks return `()` and can't fail, so the TS cases about callback errors
//! (`propagates callback errors`, `stops execution on first non-interrupt error`, `stops on
//! non-interrupt error even when interrupts were collected`) have no equivalent. Interrupts are
//! recorded on the event instead of thrown; the loop checks names after the hooks ran.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::*;
use crate::agent::Agent;
use crate::hooks::{
    AfterInvocationEvent, AfterModelCallEvent, BeforeInvocationEvent, BeforeToolsEvent, HookOptions,
    HookOrder,
};
use crate::testing::{MockModel, test_agent};
use crate::tools::executors::check_duplicate_interrupts;
use crate::types::{InterruptParams, Message, Role};

type Log = Arc<Mutex<Vec<String>>>;

fn log() -> Log {
    Arc::new(Mutex::new(Vec::new()))
}

fn entries(log: &Log) -> Vec<String> {
    log.lock().unwrap().clone()
}

fn agent() -> Agent {
    test_agent(MockModel::new())
}

async fn fire_before(registry: &HookRegistry, agent: &mut Agent) {
    let mut event = BeforeInvocationEvent::new(&mut agent.local, Default::default(), None, Vec::new());
    registry.fire::<kinds::BeforeInvocation>(&mut event).await;
}

async fn fire_after(registry: &HookRegistry, agent: &mut Agent) {
    let mut event = AfterInvocationEvent::new(&mut agent.local, Default::default(), None);
    registry.fire::<kinds::AfterInvocation>(&mut event).await;
}

fn push(log: &Log, entry: &str) -> impl Fn(&mut BeforeInvocationEvent<'_>) + Send + Sync + 'static {
    let log = Arc::clone(log);
    let entry = entry.to_owned();
    move |_: &mut BeforeInvocationEvent<'_>| log.lock().unwrap().push(entry.clone())
}

fn push_after(log: &Log, entry: &str) -> impl Fn(&mut AfterInvocationEvent<'_>) + Send + Sync + 'static {
    let log = Arc::clone(log);
    let entry = entry.to_owned();
    move |_: &mut AfterInvocationEvent<'_>| log.lock().unwrap().push(entry.clone())
}

fn ordered(order: i32) -> HookOptions {
    HookOptions {
        order: HookOrder(order),
    }
}

#[tokio::test]
async fn registers_callback_for_event_type() {
    let (registry, log, mut agent) = (HookRegistry::new(), log(), agent());
    registry.add_hook(push(&log, "called"));
    fire_before(&registry, &mut agent).await;
    assert_eq!(entries(&log), ["called"]);
}

#[tokio::test]
async fn registers_multiple_callbacks_for_same_event_type() {
    let (registry, log, mut agent) = (HookRegistry::new(), log(), agent());
    registry.add_hook(push(&log, "1"));
    registry.add_hook(push(&log, "2"));
    fire_before(&registry, &mut agent).await;
    assert_eq!(entries(&log), ["1", "2"]);
}

#[tokio::test]
async fn registers_callbacks_for_different_event_types_separately() {
    let (registry, log, mut agent) = (HookRegistry::new(), log(), agent());
    registry.add_hook(push(&log, "before"));
    registry.add_hook(push_after(&log, "after"));
    fire_before(&registry, &mut agent).await;
    assert_eq!(entries(&log), ["before"]);
    fire_after(&registry, &mut agent).await;
    assert_eq!(entries(&log), ["before", "after"]);
}

#[tokio::test]
async fn calls_registered_callbacks_in_order() {
    let (registry, log, mut agent) = (HookRegistry::new(), log(), agent());
    for name in ["first", "second", "third"] {
        registry.add_hook(push(&log, name));
    }
    fire_before(&registry, &mut agent).await;
    assert_eq!(entries(&log), ["first", "second", "third"]);
}

#[tokio::test]
async fn reverses_callback_order_for_after_events() {
    let (registry, log, mut agent) = (HookRegistry::new(), log(), agent());
    for name in ["first", "second", "third"] {
        registry.add_hook(push_after(&log, name));
    }
    fire_after(&registry, &mut agent).await;
    assert_eq!(entries(&log), ["third", "second", "first"]);
}

#[tokio::test]
async fn awaits_async_callbacks() {
    let (registry, log, mut agent) = (HookRegistry::new(), log(), agent());
    let inner = Arc::clone(&log);
    registry.add_async_hook::<BeforeInvocationEvent<'_>>(move |_| {
        let log = Arc::clone(&inner);
        Box::pin(async move {
            tokio::task::yield_now().await;
            log.lock().unwrap().push("async".into());
        })
    });
    fire_before(&registry, &mut agent).await;
    assert_eq!(entries(&log), ["async"]);
}

#[tokio::test]
async fn handles_mixed_sync_and_async_callbacks() {
    let (registry, log, mut agent) = (HookRegistry::new(), log(), agent());
    registry.add_hook(push(&log, "sync1"));
    let inner = Arc::clone(&log);
    registry.add_async_hook::<BeforeInvocationEvent<'_>>(move |_| {
        let log = Arc::clone(&inner);
        Box::pin(async move {
            tokio::task::yield_now().await;
            log.lock().unwrap().push("async".into());
        })
    });
    registry.add_hook(push(&log, "sync2"));
    fire_before(&registry, &mut agent).await;
    assert_eq!(entries(&log), ["sync1", "async", "sync2"]);
}

#[tokio::test]
async fn returns_the_event_after_invocation() {
    let (registry, mut agent) = (HookRegistry::new(), agent());
    registry.add_hook(|event: &mut BeforeInvocationEvent<'_>| event.set_cancel("stop"));
    let mut event = BeforeInvocationEvent::new(&mut agent.local, Default::default(), None, Vec::new());
    registry.fire::<kinds::BeforeInvocation>(&mut event).await;
    assert_eq!(event.cancel(), Some(&crate::hooks::Cancel::from("stop")));
}

#[tokio::test]
async fn lower_order_runs_first() {
    let (registry, log, mut agent) = (HookRegistry::new(), log(), agent());
    registry.add_hook_with(ordered(10), push(&log, "late"));
    registry.add_hook_with(ordered(-10), push(&log, "early"));
    registry.add_hook(push(&log, "default"));
    fire_before(&registry, &mut agent).await;
    assert_eq!(entries(&log), ["early", "default", "late"]);
}

#[tokio::test]
async fn same_order_preserves_registration_order() {
    let (registry, log, mut agent) = (HookRegistry::new(), log(), agent());
    registry.add_hook_with(ordered(5), push(&log, "a"));
    registry.add_hook_with(ordered(5), push(&log, "b"));
    registry.add_hook_with(ordered(5), push(&log, "c"));
    fire_before(&registry, &mut agent).await;
    assert_eq!(entries(&log), ["a", "b", "c"]);
}

#[tokio::test]
async fn negative_order_runs_before_default() {
    let (registry, log, mut agent) = (HookRegistry::new(), log(), agent());
    registry.add_hook(push(&log, "default"));
    registry.add_hook_with(
        HookOptions {
            order: HookOrder::SDK_FIRST,
        },
        push(&log, "sdk"),
    );
    fire_before(&registry, &mut agent).await;
    assert_eq!(entries(&log), ["sdk", "default"]);
}

#[tokio::test]
async fn after_events_lower_order_still_runs_first_across_groups() {
    let (registry, log, mut agent) = (HookRegistry::new(), log(), agent());
    registry.add_hook_with(ordered(10), push_after(&log, "late-1"));
    registry.add_hook_with(ordered(10), push_after(&log, "late-2"));
    registry.add_hook_with(ordered(-10), push_after(&log, "early-1"));
    registry.add_hook_with(ordered(-10), push_after(&log, "early-2"));
    fire_after(&registry, &mut agent).await;
    assert_eq!(entries(&log), ["early-2", "early-1", "late-2", "late-1"]);
}

#[tokio::test]
async fn returns_cleanup_function_that_removes_the_callback() {
    let (registry, log, mut agent) = (HookRegistry::new(), log(), agent());
    let cleanup = registry.add_hook(push(&log, "called"));
    cleanup.remove();
    fire_before(&registry, &mut agent).await;
    assert!(entries(&log).is_empty());
}

#[tokio::test]
async fn cleanup_function_is_idempotent() {
    let (registry, log, mut agent) = (HookRegistry::new(), log(), agent());
    let cleanup = registry.add_hook(push(&log, "a"));
    registry.add_hook(push(&log, "b"));
    cleanup.remove();
    cleanup.remove();
    fire_before(&registry, &mut agent).await;
    assert_eq!(entries(&log), ["b"]);
}

#[tokio::test]
async fn cleanup_function_does_not_affect_other_callbacks() {
    let (registry, log, mut agent) = (HookRegistry::new(), log(), agent());
    registry.add_hook(push(&log, "a"));
    let cleanup = registry.add_hook(push(&log, "b"));
    registry.add_hook(push(&log, "c"));
    cleanup.remove();
    fire_before(&registry, &mut agent).await;
    assert_eq!(entries(&log), ["a", "c"]);
}

#[tokio::test]
async fn allows_callback_to_be_re_registered_after_cleanup() {
    let (registry, log, mut agent) = (HookRegistry::new(), log(), agent());
    registry.add_hook(push(&log, "a")).remove();
    registry.add_hook(push(&log, "a"));
    fire_before(&registry, &mut agent).await;
    assert_eq!(entries(&log), ["a"]);
}

#[tokio::test]
async fn cleanup_from_one_registration_does_not_affect_independent_registration_of_same_function() {
    let (registry, log, mut agent) = (HookRegistry::new(), log(), agent());
    let callback = Arc::new(push(&log, "same"));
    let first = Arc::clone(&callback);
    let second = Arc::clone(&callback);
    let cleanup = registry.add_hook(move |event: &mut BeforeInvocationEvent<'_>| first(event));
    registry.add_hook(move |event: &mut BeforeInvocationEvent<'_>| second(event));
    cleanup.remove();
    fire_before(&registry, &mut agent).await;
    assert_eq!(entries(&log), ["same"]);
}

async fn fire_before_tools(registry: &HookRegistry, agent: &mut Agent) -> Vec<crate::interrupt::Interrupt> {
    let message = Message::new(Role::Assistant, Vec::new());
    let mut event = BeforeToolsEvent::new(&mut agent.local, Default::default(), None, message);
    registry.fire::<kinds::BeforeTools>(&mut event).await;
    event.into_parts().2
}

#[tokio::test]
async fn collects_interrupt_errors_from_multiple_callbacks_and_invokes_all_of_them() {
    let (registry, log, mut agent) = (HookRegistry::new(), log(), agent());
    for name in ["first", "second"] {
        let log = Arc::clone(&log);
        registry.add_hook(move |event: &mut BeforeToolsEvent<'_>| {
            log.lock().unwrap().push(name.into());
            event.interrupt(InterruptParams::new(name));
        });
    }
    let interrupts = fire_before_tools(&registry, &mut agent).await;
    assert_eq!(entries(&log), ["first", "second"]);
    let names: Vec<_> = interrupts
        .iter()
        .map(|interrupt| interrupt.name.as_str())
        .collect();
    assert_eq!(names, ["first", "second"]);
}

#[tokio::test]
async fn throws_interrupt_error_with_all_collected_interrupts_after_all_callbacks_run() {
    let (registry, mut agent) = (HookRegistry::new(), agent());
    registry.add_hook(|event: &mut BeforeToolsEvent<'_>| {
        event.interrupt(InterruptParams::new("a"));
    });
    registry.add_hook(|event: &mut BeforeToolsEvent<'_>| {
        event.interrupt(InterruptParams::new("b"));
    });
    let interrupts = check_duplicate_interrupts(fire_before_tools(&registry, &mut agent).await).unwrap();
    let ids: Vec<_> = interrupts.iter().map(|interrupt| interrupt.id.as_str()).collect();
    assert_eq!(ids, ["hook:beforeTools:a", "hook:beforeTools:b"]);
}

#[tokio::test]
async fn runs_all_callbacks_when_only_some_raise_interrupts() {
    let (registry, log, mut agent) = (HookRegistry::new(), log(), agent());
    let first = Arc::clone(&log);
    registry.add_hook(move |_: &mut BeforeToolsEvent<'_>| first.lock().unwrap().push("plain".into()));
    registry.add_hook(|event: &mut BeforeToolsEvent<'_>| {
        event.interrupt(InterruptParams::new("only"));
    });
    let last = Arc::clone(&log);
    registry.add_hook(move |_: &mut BeforeToolsEvent<'_>| last.lock().unwrap().push("after".into()));
    let interrupts = fire_before_tools(&registry, &mut agent).await;
    assert_eq!(entries(&log), ["plain", "after"]);
    assert_eq!(interrupts.len(), 1);
}

#[tokio::test]
async fn throws_when_two_callbacks_use_the_same_interrupt_name() {
    let (registry, mut agent) = (HookRegistry::new(), agent());
    for _ in 0..2 {
        registry.add_hook(|event: &mut BeforeToolsEvent<'_>| {
            event.interrupt(InterruptParams::new("dup"));
        });
    }
    let error = check_duplicate_interrupts(fire_before_tools(&registry, &mut agent).await).unwrap_err();
    assert_eq!(
        error.to_string(),
        "interrupt_names=<dup> | duplicate interrupt names"
    );
}

#[tokio::test]
async fn reports_all_duplicate_interrupt_names_in_error() {
    let (registry, mut agent) = (HookRegistry::new(), agent());
    for name in ["x", "y", "x", "y"] {
        registry.add_hook(move |event: &mut BeforeToolsEvent<'_>| {
            event.interrupt(InterruptParams::new(name));
        });
    }
    let error = check_duplicate_interrupts(fire_before_tools(&registry, &mut agent).await).unwrap_err();
    assert_eq!(
        error.to_string(),
        "interrupt_names=<x, y> | duplicate interrupt names"
    );
}

struct Counter(Log);

#[async_trait::async_trait]
impl<'a> Hook<BeforeInvocationEvent<'a>> for Counter {
    async fn handle(&self, event: &mut BeforeInvocationEvent<'a>) {
        self.0.lock().unwrap().push("impl".into());
        event.agent_mut().app_state().set("seen", true).unwrap();
    }
}

#[tokio::test]
async fn hook_impls_run_with_the_borrowed_agent() {
    let (registry, log, mut agent) = (HookRegistry::new(), log(), agent());
    registry.add_hook_impl::<BeforeInvocationEvent<'_>, _>(Counter(Arc::clone(&log)));
    fire_before(&registry, &mut agent).await;
    assert_eq!(entries(&log), ["impl"]);
    assert_eq!(agent.app_state().get_value("seen"), Some(json!(true)));
}

#[tokio::test(flavor = "multi_thread")]
async fn async_hooks_on_borrowing_events_run_on_a_multi_threaded_runtime() {
    let (registry, mut agent) = (HookRegistry::new(), agent());
    registry.add_async_hook::<AfterModelCallEvent<'_>>(|event| {
        Box::pin(async move {
            tokio::task::yield_now().await;
            event.set_retry(true);
        })
    });
    let model = Arc::clone(agent.model());
    let error = crate::error::ModelError::new("boom").into();
    let mut event =
        AfterModelCallEvent::new(&mut agent.local, Default::default(), None, model, 1, Err(error));
    registry.fire::<kinds::AfterModelCall>(&mut event).await;
    assert!(event.retry());
}
