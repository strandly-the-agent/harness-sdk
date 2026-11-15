//! Ported from strands-ts/src/background-tasks/in-process/__tests__/manager.test.ts. Runs go
//! through the real background pipeline (`execute_background`).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use super::*;
use crate::agent::InvocationContext;
use crate::background_tasks::in_process::engine::system_clock;
use crate::hooks::HookRegistry;
use crate::interrupt::{Interrupt, InterruptParams};
use crate::test_support::{stream_tool, text_result};
use crate::testing::MockMessageModel;
use crate::tools::executors::executor::{BackgroundToolRun, error_text_result, execute_background};
use crate::tools::{ToolContext, ToolOutcome, ToolStream};
use crate::types::agent::LocalAgent;

type Body = dyn Fn(ToolContext) -> ToolOutcome + Send + Sync;

fn manager() -> Manager {
    let execute: ExecuteTool = Arc::new(|run: ToolRun| {
        Box::pin(execute_background(BackgroundToolRun {
            agent: run.agent,
            hooks: HookRegistry::new(),
            tool: run.tool,
            tool_use: run.tool_use,
            invocation_state: run.invocation_state,
            task_id: run.task_id,
            guard: None,
        }))
    });
    Manager::new(
        execute,
        ManagerConfig {
            max_concurrency: 4,
            timeout: None,
            on_task_updated: None,
            clock: system_clock(),
        },
    )
    .unwrap()
}

fn work(body: impl Fn(ToolContext) -> ToolOutcome + Send + Sync + 'static) -> Arc<dyn Tool> {
    let body: Arc<Body> = Arc::new(body);
    Arc::new(stream_tool("work", move |context| {
        let body = Arc::clone(&body);
        ToolStream::from_future(async move { body(context) })
    }))
}

fn agent() -> ToolAgent {
    LocalAgent::new(Arc::new(MockMessageModel::new())).tool_agent()
}

fn tool_use(value: &str, id: &str) -> ToolUse {
    ToolUse::new("work", id, json!({"value": value}))
}

fn submit(manager: &Manager, tool: &Arc<dyn Tool>, value: &str, id: &str, pass: &str) -> BackgroundTask {
    manager.submit(
        &tool_use(value, id),
        InvocationState::new(),
        pass,
        Arc::clone(tool),
        agent(),
    )
}

fn ok(context: &ToolContext, text: &str) -> ToolOutcome {
    ToolOutcome::Result(text_result(context.tool_use().tool_use_id(), text))
}

fn value(context: &ToolContext) -> String {
    context.tool_use().input()["value"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test]
async fn executes_the_selected_tool_with_transient_invocation_state() {
    #[derive(Clone)]
    struct RequestId(&'static str);
    let manager = manager();
    let tool = work(|context| {
        assert_eq!(
            context.invocation_state().get::<RequestId>().map(|id| id.0),
            Some("request-1")
        );
        assert_eq!(context.tool_use().name(), "hook-alias");
        ok(&context, &value(&context).to_uppercase())
    });
    let state = InvocationState::new();
    state.insert(RequestId("request-1"));
    let call = ToolUse::new("hook-alias", "tool-use-1", json!({"value": "hello"}));
    let admitted = manager.submit(&call, state, "pass-1", tool, agent());
    manager.wait_for_idle(None).await.unwrap();
    let completed = manager.get(&admitted.task_id).unwrap();
    assert_eq!(completed.status, TaskStatus::Completed);
    assert_eq!(completed.tool_name, "hook-alias");
    assert_eq!(
        completed.result.as_ref().unwrap().content,
        [json!({"text": "HELLO"})]
    );
    assert_eq!(manager.list(), [completed]);
    manager.remove(&[admitted.task_id.clone()]).unwrap();
    assert_eq!(manager.get(&admitted.task_id), None);
}

#[tokio::test]
async fn runs_a_task_under_the_invocation_context_captured_when_it_was_submitted() {
    let manager = manager();
    let seen: Arc<Mutex<Option<bool>>> = Arc::default();
    let slot = Arc::clone(&seen);
    let tool = work(move |context| {
        *slot.lock().unwrap() = Some(context.invocation_context().is_some());
        ok(&context, "done")
    });
    let mut local = LocalAgent::new(Arc::new(MockMessageModel::new()));
    local.set_invocation_context(Some(InvocationContext::new(None, None, false)));
    manager.submit(
        &tool_use("x", "tool-use-1"),
        InvocationState::new(),
        "pass-1",
        tool,
        local.tool_agent(),
    );
    manager.wait_for_idle(None).await.unwrap();
    assert_eq!(*seen.lock().unwrap(), Some(true));
}

#[tokio::test]
async fn deduplicates_repeated_submissions_within_one_pass() {
    let manager = manager();
    let count = Arc::new(Mutex::new(0));
    let counter = Arc::clone(&count);
    let tool = work(move |context| {
        *counter.lock().unwrap() += 1;
        ok(&context, &value(&context))
    });
    let first = submit(&manager, &tool, "same", "tool-use-1", "pass-1");
    let duplicate = submit(&manager, &tool, "same", "tool-use-1", "pass-1");
    let later = submit(&manager, &tool, "same", "tool-use-1", "pass-2");
    assert_eq!(duplicate.task_id, first.task_id);
    assert_ne!(later.task_id, first.task_id);
    manager.wait_for_idle(None).await.unwrap();
    assert_eq!(*count.lock().unwrap(), 2);
}

#[tokio::test]
async fn projects_error_name_as_status() {
    // A failed result is a cancel only when the task's signal fired (TS checks for `AbortError`).
    let manager = manager();
    let tool = work(|context| {
        let message = "timed out";
        ToolOutcome::Result(
            error_text_result(context.tool_use().tool_use_id(), format!("Error: {message}"))
                .with_error(crate::Error::other(message)),
        )
    });
    let failed = submit(&manager, &tool, "error", "tool-use-error", "pass-error");
    let failed = manager.wait(&failed.task_id).await.unwrap();
    assert_eq!(failed.status, TaskStatus::Failed);
    assert_eq!(
        failed.error,
        Some(Failure::new(FailureType::ToolError, "timed out"))
    );
    assert_eq!(
        failed.result.unwrap().content,
        [json!({"text": "Error: timed out"})]
    );

    let aborting = work(|context| {
        context.agent().cancel(crate::CancelOptions::default());
        ToolOutcome::Result(error_text_result(
            context.tool_use().tool_use_id(),
            "Error: aborted",
        ))
    });
    let cancelled = submit(&manager, &aborting, "abort", "tool-use-abort", "pass-abort");
    assert_eq!(
        manager.wait(&cancelled.task_id).await.unwrap().status,
        TaskStatus::Cancelled
    );
    manager.wait_for_idle(None).await.unwrap();
}

#[tokio::test]
async fn cancels_tool_execution() {
    let manager = manager();
    let started = Arc::new(tokio::sync::Notify::new());
    let seen: Arc<Mutex<Option<crate::CancelSignal>>> = Arc::default();
    let (notify, slot) = (Arc::clone(&started), Arc::clone(&seen));
    let tool: Arc<dyn Tool> = Arc::new(stream_tool("work", move |context| {
        let (notify, slot) = (Arc::clone(&notify), Arc::clone(&slot));
        ToolStream::from_future(async move {
            *slot.lock().unwrap() = Some(context.cancel_signal().clone());
            notify.notify_one();
            context.cancel_signal().cancelled().await;
            ok(&context, "late")
        })
    }));
    let admitted = submit(&manager, &tool, "cancel", "tool-use-cancel", "pass-cancel");
    started.notified().await;
    let cancelled = manager.cancel(&admitted.task_id).unwrap();
    assert_eq!(cancelled.status, TaskStatus::Cancelled);
    assert_eq!(manager.wait(&admitted.task_id).await.unwrap(), cancelled);
    let signal = seen.lock().unwrap().clone().unwrap();
    assert!(signal.is_cancelled());
    assert_eq!(signal.reason().as_deref(), Some("Cancellation requested"));
    manager.wait_for_idle(None).await.unwrap();
}

#[tokio::test]
async fn rejects_an_invalid_wait_timeout() {
    let error = manager().wait_for_idle(Some(Duration::ZERO)).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "wait timeout must be a positive finite integer, got 0"
    );
}

#[tokio::test(start_paused = true)]
async fn throws_backgroundtasktimeouterror_when_the_wait_exceeds_the_timeout() {
    let manager = manager();
    let tool: Arc<dyn Tool> = Arc::new(stream_tool("work", |context| {
        ToolStream::from_future(async move {
            context.cancel_signal().cancelled().await;
            ok(&context, "cancelled")
        })
    }));
    let admitted = submit(&manager, &tool, "slow", "tool-use-slow", "pass-slow");
    let error = manager
        .wait_for_idle(Some(Duration::from_millis(10)))
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Timed out waiting for background tasks after 10ms"
    );
    manager.cancel(&admitted.task_id).unwrap();
    manager.wait_for_idle(None).await.unwrap();
}

#[tokio::test]
async fn requests_input_and_resumes_tool_interrupts() {
    let manager = manager();
    let tool = work(|context| {
        if value(&context) == "complete" {
            return ok(&context, "complete");
        }
        let params = InterruptParams {
            name: "approve_work".to_owned(),
            reason: Some(json!("Approve work?")),
            response: None,
        };
        match context.interrupt(params) {
            Ok(response) => ok(
                &context,
                &format!("approved:{}", response.as_str().unwrap_or_default()),
            ),
            Err(interrupt) => ToolOutcome::Interrupt(interrupt),
        }
    });
    let completed = submit(&manager, &tool, "complete", "tool-use-complete", "pass-complete");
    manager.wait(&completed.task_id).await.unwrap();
    let admitted = submit(&manager, &tool, "interrupt", "tool-use-1", "pass-interrupt");
    let input_required = manager.wait(&admitted.task_id).await.unwrap();
    assert_eq!(input_required.status, TaskStatus::InputRequired);
    let interrupts = input_required.interrupts.clone().unwrap();
    assert_eq!(interrupts.len(), 1);
    assert_eq!(
        interrupts[0].id(),
        format!("tool:{}:approve_work", admitted.task_id)
    );
    assert_eq!(interrupts[0].name(), "approve_work");
    assert_eq!(interrupts[0].reason(), Some(&json!("Approve work?")));
    assert_eq!(
        manager
            .remove(&[completed.task_id.clone(), admitted.task_id.clone()])
            .unwrap_err()
            .to_string(),
        format!(
            "Background task '{}' cannot be removed before reaching a terminal status",
            admitted.task_id
        )
    );
    assert!(manager.get(&completed.task_id).is_some());
    let response = InterruptResponse::new(interrupts[0].id(), json!("yes"));
    assert_eq!(
        manager.resume(&admitted.task_id, &[response]).unwrap().status,
        TaskStatus::Queued
    );
    let done = manager.wait(&admitted.task_id).await.unwrap();
    assert_eq!(done.status, TaskStatus::Completed);
    assert_eq!(done.result.unwrap().content, [json!({"text": "approved:yes"})]);
}

#[tokio::test]
async fn fails_tasks_interrupted_outside_their_tool_context() {
    let manager = manager();
    let tool = work(|_| {
        ToolOutcome::Interrupt(
            Interrupt::new("hook:beforeToolCall:tool-use-1:approve_hook", "approve_hook")
                .with_source(InterruptSource::Hook),
        )
    });
    let admitted = submit(&manager, &tool, "hook-interrupt", "tool-use-1", "pass-hook");
    let failed = manager.wait(&admitted.task_id).await.unwrap();
    assert_eq!(failed.status, TaskStatus::Failed);
    assert_eq!(
        failed.error,
        Some(Failure::new(
            FailureType::ExecutionError,
            "Interrupt raised: approve_hook"
        ))
    );
}
