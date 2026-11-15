//! Ported from strands-ts/src/middleware/__tests__/registry.test.ts and custom-stages.test.ts.

use std::sync::{Arc, Mutex};

use futures::StreamExt;

use super::{
    MiddlewareError, MiddlewareItem, MiddlewareNext, MiddlewarePhaseKind, MiddlewareRegistry,
    MiddlewareStage, MiddlewareStream,
};
use crate::error::Error;
use crate::interrupt::Interrupt;

#[derive(Debug)]
#[non_exhaustive]
struct TestStage;

impl MiddlewareStage for TestStage {
    type Context = String;
    type Result = String;
    type Event = String;
    const NAME: &'static str = "test";
}

/// A second stage with the same name, to check that stages are told apart by type.
#[derive(Debug)]
#[non_exhaustive]
struct SameNameStage;

impl MiddlewareStage for SameNameStage {
    type Context = String;
    type Result = String;
    type Event = String;
    const NAME: &'static str = "test";
}

type Item = Result<MiddlewareItem<String, String>, MiddlewareError>;
type Log = Arc<Mutex<Vec<String>>>;

fn push(log: &Log, entry: &str) {
    log.lock().unwrap().push(entry.to_owned());
}

fn entries(log: &Log) -> Vec<String> {
    log.lock().unwrap().clone()
}

/// Pushes `entry` when dropped, like a TS `finally` block.
struct Finally(Log, &'static str);

impl Drop for Finally {
    fn drop(&mut self) {
        push(&self.0, self.1);
    }
}

fn stream<'a>(items: Vec<Item>) -> MiddlewareStream<'a, TestStage> {
    Box::pin(futures::stream::iter(items))
}

fn result(output: &str) -> Item {
    Ok(MiddlewareItem::Result(output.to_owned()))
}

fn event(data: &str) -> Item {
    Ok(MiddlewareItem::Event(data.to_owned()))
}

fn failed(message: &str) -> Item {
    Err(Error::other(message.to_owned()).into())
}

/// Collects a chain into its events and its outcome.
async fn collect(
    mut chain: MiddlewareStream<'_, TestStage>,
) -> (Vec<String>, Result<String, MiddlewareError>) {
    let mut events = Vec::new();
    while let Some(item) = chain.next().await {
        match item {
            Ok(MiddlewareItem::Event(event)) => events.push(event),
            Ok(MiddlewareItem::Result(result)) => return (events, Ok(result)),
            Err(error) => return (events, Err(error)),
        }
    }
    panic!("the chain ended without a result");
}

/// A Wrap handler that logs `name` and passes through.
fn logging(log: &Log, name: &'static str) -> super::Middleware<TestStage> {
    let log = Arc::clone(log);
    TestStage::wrap(move |context, next| {
        push(&log, name);
        next.run(context)
    })
}

/// A Wrap handler that logs `enter` before and keeps a `finally` guard while the rest runs.
fn guarded(log: &Log, enter: Option<&'static str>, finally: &'static str) -> super::Middleware<TestStage> {
    let log = Arc::clone(log);
    TestStage::wrap(move |context, next| {
        if let Some(enter) = enter {
            push(&log, enter);
        }
        let guard = Finally(Arc::clone(&log), finally);
        Box::pin(next.run(context).map(move |item| {
            let _guard = &guard;
            item
        }))
    })
}

fn interrupt() -> Interrupt {
    Interrupt::new("test-id", "test-interrupt")
}

#[tokio::test]
async fn stores_handlers_in_registration_order() {
    let registry = MiddlewareRegistry::new();
    let log = Log::default();
    registry.add(logging(&log, "1"));
    registry.add(logging(&log, "2"));
    let terminal_log = Arc::clone(&log);
    let chain = registry.invoke::<TestStage>("test".to_owned(), move |_| {
        push(&terminal_log, "3");
        stream(vec![result("done")])
    });
    collect(chain).await.1.unwrap();
    assert_eq!(entries(&log), ["1", "2", "3"]);
}

#[tokio::test]
async fn with_no_handlers_returns_terminal_equivalent_function() {
    let registry = MiddlewareRegistry::new();
    let chain = registry.invoke::<TestStage>("hello".to_owned(), |context| {
        stream(vec![event("event-1"), result(&format!("got: {context}"))])
    });
    let (events, outcome) = collect(chain).await;
    assert_eq!(events, ["event-1"]);
    assert_eq!(outcome.unwrap(), "got: hello");
}

#[tokio::test]
async fn executes_handlers_in_registration_order_outermost_first() {
    let registry = MiddlewareRegistry::new();
    let log = Log::default();
    for name in ["a", "b"] {
        let log = Arc::clone(&log);
        registry.add(TestStage::wrap(move |context, next| {
            push(&log, &format!("{name}-before"));
            let log = Arc::clone(&log);
            let inner = next.run(context);
            Box::pin(async_stream::stream! {
                let mut inner = inner;
                while let Some(item) = inner.next().await {
                    yield item;
                }
                push(&log, &format!("{name}-after"));
            })
        }));
    }
    let terminal_log = Arc::clone(&log);
    let chain = registry.invoke::<TestStage>("test".to_owned(), move |_| {
        push(&terminal_log, "terminal");
        stream(vec![result("done")])
    });
    let (_, outcome) = collect_all(chain).await;
    assert_eq!(outcome.unwrap(), "done");
    assert_eq!(
        entries(&log),
        ["a-before", "b-before", "terminal", "b-after", "a-after"]
    );
}

/// Drains the whole chain, so handler code after the result runs.
async fn collect_all(
    mut chain: MiddlewareStream<'_, TestStage>,
) -> (Vec<String>, Result<String, MiddlewareError>) {
    let mut events = Vec::new();
    let mut outcome = None;
    while let Some(item) = chain.next().await {
        match item {
            Ok(MiddlewareItem::Event(event)) => events.push(event),
            Ok(MiddlewareItem::Result(result)) => outcome = Some(Ok(result)),
            Err(error) => outcome = Some(Err(error)),
        }
    }
    (events, outcome.unwrap())
}

#[tokio::test]
async fn does_not_call_next_handlers_or_terminal_when_middleware_does_not_call_next() {
    let registry = MiddlewareRegistry::new();
    let log = Log::default();
    registry.add(TestStage::wrap(|_, _| stream(vec![result("short-circuit")])));
    registry.add(logging(&log, "second"));
    let terminal_log = Arc::clone(&log);
    let chain = registry.invoke::<TestStage>("test".to_owned(), move |_| {
        push(&terminal_log, "terminal");
        stream(vec![result("terminal")])
    });
    let (_, outcome) = collect(chain).await;
    assert_eq!(outcome.unwrap(), "short-circuit");
    assert!(entries(&log).is_empty());
}

#[tokio::test]
async fn forwards_all_events_from_terminal_and_returns_terminal_result() {
    let registry = MiddlewareRegistry::new();
    registry.add(TestStage::wrap(|context, next| next.run(context)));
    let chain = registry.invoke::<TestStage>("test".to_owned(), |_| {
        stream(vec![event("a"), event("b"), event("c"), result("done")])
    });
    let (events, outcome) = collect(chain).await;
    assert_eq!(events, ["a", "b", "c"]);
    assert_eq!(outcome.unwrap(), "done");
}

#[tokio::test]
async fn only_yields_events_matching_a_predicate() {
    let registry = MiddlewareRegistry::new();
    registry.add(TestStage::wrap(|context, next| {
        Box::pin(next.run(context).filter(|item| {
            std::future::ready(!matches!(item, Ok(MiddlewareItem::Event(data)) if data.starts_with("skip")))
        }))
    }));
    let chain = registry.invoke::<TestStage>("test".to_owned(), |_| {
        stream(vec![
            event("keep-1"),
            event("skip-1"),
            event("keep-2"),
            event("skip-2"),
            result("done"),
        ])
    });
    let (events, _) = collect(chain).await;
    assert_eq!(events, ["keep-1", "keep-2"]);
}

#[tokio::test]
async fn terminal_receives_modified_context() {
    let registry = MiddlewareRegistry::new();
    registry.add(TestStage::wrap(|_, next| next.run("modified".to_owned())));
    let chain = registry.invoke::<TestStage>("original".to_owned(), |context| stream(vec![result(&context)]));
    assert_eq!(collect(chain).await.1.unwrap(), "modified");
}

#[tokio::test]
async fn each_middleware_can_further_modify_context() {
    let registry = MiddlewareRegistry::new();
    registry.add(TestStage::wrap(|context, next| next.run(format!("{context}-a"))));
    registry.add(TestStage::wrap(|context, next| next.run(format!("{context}-b"))));
    let chain = registry.invoke::<TestStage>("start".to_owned(), |context| stream(vec![result(&context)]));
    assert_eq!(collect(chain).await.1.unwrap(), "start-a-b");
}

#[tokio::test]
async fn errors_from_terminal_reach_middleware() {
    let registry = MiddlewareRegistry::new();
    let caught = Log::default();
    let log = Arc::clone(&caught);
    registry.add(TestStage::wrap(move |context, next| {
        let log = Arc::clone(&log);
        Box::pin(next.run(context).map(move |item| match item {
            Err(error) => {
                push(&log, &error.to_string());
                result("recovered")
            }
            other => other,
        }))
    }));
    let chain = registry.invoke::<TestStage>("test".to_owned(), |_| stream(vec![failed("terminal error")]));
    assert_eq!(collect(chain).await.1.unwrap(), "recovered");
    assert_eq!(entries(&caught), ["terminal error"]);
}

#[tokio::test]
async fn errors_from_middleware_reach_caller() {
    let registry = MiddlewareRegistry::new();
    registry.add(TestStage::wrap(|_, _| stream(vec![failed("middleware error")])));
    let chain = registry.invoke::<TestStage>("test".to_owned(), |_| stream(vec![result("done")]));
    assert_eq!(
        collect(chain).await.1.unwrap_err().to_string(),
        "middleware error"
    );
}

#[tokio::test]
async fn middleware_can_transform_errors_from_next() {
    let registry = MiddlewareRegistry::new();
    registry.add(TestStage::wrap(|context, next| {
        Box::pin(next.run(context).map(|item| match item {
            Err(_) => failed("transformed error"),
            other => other,
        }))
    }));
    let chain = registry.invoke::<TestStage>("test".to_owned(), |_| stream(vec![failed("original error")]));
    assert_eq!(
        collect(chain).await.1.unwrap_err().to_string(),
        "transformed error"
    );
}

#[tokio::test]
async fn cancellederror_propagates_without_being_swallowed() {
    let registry = MiddlewareRegistry::new();
    registry.add(TestStage::wrap(|context, next| next.run(context)));
    let chain = registry.invoke::<TestStage>("test".to_owned(), |_| {
        stream(vec![failed("Agent invocation cancelled")])
    });
    let error = collect(chain).await.1.unwrap_err();
    assert!(matches!(error, MiddlewareError::Failed(_)));
    assert_eq!(error.to_string(), "Agent invocation cancelled");
}

#[tokio::test]
async fn interrupterror_propagates_without_being_swallowed() {
    let registry = MiddlewareRegistry::new();
    registry.add(TestStage::wrap(|context, next| next.run(context)));
    let chain = registry.invoke::<TestStage>("test".to_owned(), |_| stream(vec![Err(interrupt().into())]));
    let error = collect(chain).await.1.unwrap_err();
    assert!(matches!(error, MiddlewareError::Interrupt(interrupt) if interrupt.name() == "test-interrupt"));
}

#[tokio::test]
async fn cancellederror_propagates_through_multiple_middleware_layers() {
    let registry = MiddlewareRegistry::new();
    let log = Log::default();
    registry.add(logging(&log, "outer"));
    registry.add(logging(&log, "inner"));
    let chain = registry.invoke::<TestStage>("test".to_owned(), |_| {
        stream(vec![failed("Agent invocation cancelled")])
    });
    assert_eq!(
        collect(chain).await.1.unwrap_err().to_string(),
        "Agent invocation cancelled"
    );
    assert_eq!(entries(&log), ["outer", "inner"]);
}

#[tokio::test]
async fn interrupterror_propagates_through_multiple_middleware_layers() {
    let registry = MiddlewareRegistry::new();
    let log = Log::default();
    registry.add(logging(&log, "outer"));
    registry.add(logging(&log, "inner"));
    let chain = registry.invoke::<TestStage>("test".to_owned(), |_| stream(vec![Err(interrupt().into())]));
    assert!(matches!(
        collect(chain).await.1,
        Err(MiddlewareError::Interrupt(_))
    ));
    assert_eq!(entries(&log), ["outer", "inner"]);
}

#[tokio::test]
async fn outer_finally_runs_when_inner_middleware_throws() {
    let registry = MiddlewareRegistry::new();
    let log = Log::default();
    registry.add(guarded(&log, None, "outer-finally"));
    let inner_log = Arc::clone(&log);
    registry.add(TestStage::wrap(move |_, _| {
        push(&inner_log, "inner-throw");
        stream(vec![failed("inner exploded")])
    }));
    let chain = registry.invoke::<TestStage>("test".to_owned(), |_| stream(vec![result("unreachable")]));
    assert_eq!(
        collect_all(chain).await.1.unwrap_err().to_string(),
        "inner exploded"
    );
    assert_eq!(entries(&log), ["inner-throw", "outer-finally"]);
}

#[tokio::test]
async fn inner_finally_runs_when_terminal_throws() {
    let registry = MiddlewareRegistry::new();
    let log = Log::default();
    registry.add(guarded(&log, None, "outer-finally"));
    registry.add(guarded(&log, None, "inner-finally"));
    let terminal_log = Arc::clone(&log);
    let chain = registry.invoke::<TestStage>("test".to_owned(), move |_| {
        push(&terminal_log, "terminal-throw");
        stream(vec![failed("terminal exploded")])
    });
    assert_eq!(
        collect_all(chain).await.1.unwrap_err().to_string(),
        "terminal exploded"
    );
    assert_eq!(
        entries(&log),
        ["terminal-throw", "inner-finally", "outer-finally"]
    );
}

#[tokio::test]
async fn all_finally_blocks_run_in_reverse_order_when_terminal_throws() {
    let registry = MiddlewareRegistry::new();
    let log = Log::default();
    registry.add(guarded(&log, Some("a-enter"), "a-finally"));
    registry.add(guarded(&log, Some("b-enter"), "b-finally"));
    registry.add(guarded(&log, Some("c-enter"), "c-finally"));
    let chain = registry.invoke::<TestStage>("test".to_owned(), |_| stream(vec![failed("boom")]));
    assert_eq!(collect_all(chain).await.1.unwrap_err().to_string(), "boom");
    assert_eq!(
        entries(&log),
        [
            "a-enter",
            "b-enter",
            "c-enter",
            "c-finally",
            "b-finally",
            "a-finally"
        ]
    );
}

#[tokio::test]
async fn finally_runs_even_when_caller_abandons_the_generator_mid_stream() {
    let registry = MiddlewareRegistry::new();
    let log = Log::default();
    registry.add(guarded(&log, None, "middleware-finally"));
    let mut chain = registry.invoke::<TestStage>("test".to_owned(), |_| {
        stream(vec![
            event("event-1"),
            event("event-2"),
            event("event-3"),
            result("done"),
        ])
    });
    chain.next().await;
    drop(chain);
    assert_eq!(entries(&log), ["middleware-finally"]);
}

#[tokio::test]
async fn finally_runs_in_both_middleware_when_caller_abandons_mid_stream() {
    let registry = MiddlewareRegistry::new();
    let log = Log::default();
    registry.add(guarded(&log, None, "outer-finally"));
    registry.add(guarded(&log, None, "inner-finally"));
    let mut chain = registry.invoke::<TestStage>("test".to_owned(), |_| {
        stream(vec![event("event-1"), event("event-2"), result("done")])
    });
    chain.next().await;
    drop(chain);
    let mut finished = entries(&log);
    finished.sort();
    assert_eq!(finished, ["inner-finally", "outer-finally"]);
}

#[tokio::test]
async fn removes_a_handler_so_it_no_longer_runs() {
    let registry = MiddlewareRegistry::new();
    let log = Log::default();
    let cleanup = registry.add(logging(&log, "handler"));
    cleanup.remove();
    collect(registry.invoke::<TestStage>("test".to_owned(), |_| stream(vec![result("done")])))
        .await
        .1
        .unwrap();
    assert!(entries(&log).is_empty());
}

#[tokio::test]
async fn only_removes_the_first_occurrence_of_a_duplicate_handler() {
    let registry = MiddlewareRegistry::new();
    let log = Log::default();
    let handler = logging(&log, "handler");
    let first = registry.add(handler.clone());
    registry.add(handler);
    first.remove();
    collect(registry.invoke::<TestStage>("test".to_owned(), |_| stream(vec![result("done")])))
        .await
        .1
        .unwrap();
    assert_eq!(entries(&log), ["handler"]);
}

#[tokio::test]
async fn is_a_no_op_when_handler_was_never_registered() {
    let registry = MiddlewareRegistry::new();
    let other = MiddlewareRegistry::new();
    let log = Log::default();
    registry.add(logging(&log, "kept"));
    other.add(logging(&log, "elsewhere")).remove();
    collect(registry.invoke::<TestStage>("test".to_owned(), |_| stream(vec![result("done")])))
        .await
        .1
        .unwrap();
    assert_eq!(entries(&log), ["kept"]);
}

#[test]
fn is_a_no_op_when_stage_has_no_handlers() {
    let registry = MiddlewareRegistry::new();
    let log = Log::default();
    let cleanup = registry.add(logging(&log, "handler"));
    cleanup.remove();
    assert!(!registry.has::<TestStage>());
}

#[tokio::test]
async fn phases_compose_input_then_wrap_then_output_whatever_the_registration_order() {
    let registry = MiddlewareRegistry::new();
    let log = Log::default();
    let output_log = Arc::clone(&log);
    registry.add(TestStage::output(move |result: String| {
        let log = Arc::clone(&output_log);
        async move {
            push(&log, "output");
            Ok(format!("{result}+output"))
        }
    }));
    registry.add(logging(&log, "wrap"));
    let input_log = Arc::clone(&log);
    registry.add(TestStage::input(move |context: String| {
        let log = Arc::clone(&input_log);
        async move {
            push(&log, "input");
            Ok(format!("{context}+input"))
        }
    }));
    let chain = registry.invoke::<TestStage>("ctx".to_owned(), |context| stream(vec![result(&context)]));
    assert_eq!(collect(chain).await.1.unwrap(), "ctx+input+output");
    assert_eq!(entries(&log), ["input", "wrap", "output"]);
}

#[tokio::test]
async fn an_input_handler_failure_skips_the_rest_of_the_chain() {
    let registry = MiddlewareRegistry::new();
    registry.add(TestStage::input(|_: String| async {
        Err(Error::other("bad input").into())
    }));
    let log = Log::default();
    let terminal_log = Arc::clone(&log);
    let chain = registry.invoke::<TestStage>("ctx".to_owned(), move |_| {
        push(&terminal_log, "terminal");
        stream(vec![result("done")])
    });
    assert_eq!(collect(chain).await.1.unwrap_err().to_string(), "bad input");
    assert!(entries(&log).is_empty());
}

#[test]
fn middleware_reports_its_phase() {
    assert_eq!(
        TestStage::wrap(|context, next| next.run(context)).phase(),
        MiddlewarePhaseKind::Wrap
    );
    assert_eq!(
        TestStage::input(|context: String| async { Ok(context) }).phase(),
        MiddlewarePhaseKind::Input
    );
    assert_eq!(
        TestStage::output(|result: String| async { Ok(result) }).phase(),
        MiddlewarePhaseKind::Output
    );
}

// custom-stages.test.ts

#[test]
fn returns_object_with_correct_name_property() {
    assert_eq!(TestStage::NAME, "test");
    assert_eq!(super::InvokeModelStage::NAME, "invokeModel");
    assert_eq!(super::ExecuteToolStage::NAME, "executeTool");
    assert_eq!(super::AgentStreamStage::NAME, "agentStream");
}

#[tokio::test]
async fn works_with_registry_add_and_compose_handlers_execute_correctly() {
    let registry = MiddlewareRegistry::new();
    registry.add(TestStage::wrap(|context, next| {
        Box::pin(futures::stream::iter([event("from-middleware")]).chain(next.run(context)))
    }));
    let next: MiddlewareNext<'_, TestStage> =
        registry.compose(|context| stream(vec![event("from-terminal"), result(&format!("{context}!"))]));
    let (events, outcome) = collect(next.run("custom".to_owned())).await;
    assert_eq!(events, ["from-middleware", "from-terminal"]);
    assert_eq!(outcome.unwrap(), "custom!");
}

#[tokio::test]
async fn two_stages_with_the_same_name_are_distinct_reference_identity() {
    let registry = MiddlewareRegistry::new();
    let log = Log::default();
    registry.add(logging(&log, "first"));
    let second_log = Arc::clone(&log);
    registry.add(SameNameStage::wrap(move |context, next| {
        push(&second_log, "second");
        next.run(context)
    }));
    collect(registry.invoke::<TestStage>("test".to_owned(), |_| stream(vec![result("done")])))
        .await
        .1
        .unwrap();
    assert_eq!(entries(&log), ["first"]);
}

#[tokio::test]
async fn custom_stage_middleware_can_be_composed_with_a_terminal_and_executes_correctly() {
    let registry = MiddlewareRegistry::new();
    registry.add(TestStage::wrap(|context, next| next.run(context.to_uppercase())));
    registry.add(TestStage::output(|result: String| async move {
        Ok(format!("<{result}>"))
    }));
    let chain = registry.invoke::<TestStage>("value".to_owned(), |context| stream(vec![result(&context)]));
    assert_eq!(collect(chain).await.1.unwrap(), "<VALUE>");
}

#[tokio::test]
async fn a_handler_can_call_next_again_to_retry() {
    let registry = MiddlewareRegistry::new();
    registry.add(TestStage::wrap(|context, next| {
        Box::pin(async_stream::stream! {
            for attempt in 0..2 {
                let mut inner = next.run(format!("{context}-{attempt}"));
                while let Some(item) = inner.next().await {
                    match item {
                        Err(_) if attempt == 0 => break,
                        other => yield other,
                    }
                }
            }
        })
    }));
    let calls = Arc::new(Mutex::new(0));
    let counter = Arc::clone(&calls);
    let chain = registry.invoke::<TestStage>("try".to_owned(), move |context| {
        let mut calls = counter.lock().unwrap();
        *calls += 1;
        if *calls == 1 {
            stream(vec![failed("transient")])
        } else {
            stream(vec![result(&context)])
        }
    });
    assert_eq!(collect(chain).await.1.unwrap(), "try-1");
    assert_eq!(*calls.lock().unwrap(), 2);
}
