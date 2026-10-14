#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use parking_lot::Mutex;

    use crate::error::Error;
    use crate::middleware::{wrap_fn, EventSink, Middleware, MiddlewareRegistry, Next, Stage, Terminal};

    struct TestStage;
    impl Stage for TestStage {
        type Context = String;
        type Result = String;
        type Event = String;
        fn name() -> &'static str {
            "test"
        }
    }

    struct OtherStage;
    impl Stage for OtherStage {
        type Context = String;
        type Result = String;
        type Event = String;
        fn name() -> &'static str {
            "test"
        }
    }

    type Trace = Arc<Mutex<Vec<String>>>;

    fn trace() -> Trace {
        Arc::new(Mutex::new(Vec::new()))
    }

    /// Terminal that records the context it saw and returns `"terminal:<ctx>"`.
    fn terminal(trace: Trace) -> impl Terminal<TestStage> {
        move |ctx: String, _sink: EventSink<String>| {
            let trace = Arc::clone(&trace);
            async move {
                trace.lock().push(format!("terminal:{ctx}"));
                Ok(format!("terminal:{ctx}"))
            }
        }
    }

    /// Terminal that emits `events` and then returns `"done"`.
    fn emitting_terminal(events: Vec<&'static str>) -> impl Terminal<TestStage> {
        move |_ctx: String, sink: EventSink<String>| {
            let events = events.clone();
            async move {
                for event in events {
                    sink.send(event.to_owned()).await?;
                }
                Ok("done".to_owned())
            }
        }
    }

    fn recording_wrap(trace: Trace, tag: &'static str) -> impl Middleware<TestStage> {
        wrap_fn(
            move |ctx: String, sink: EventSink<String>, next: Next<TestStage>| {
                let trace = Arc::clone(&trace);
                async move {
                    trace.lock().push(format!("{tag}:before"));
                    let result = next.run(ctx, &sink).await;
                    trace.lock().push(format!("{tag}:after"));
                    result
                }
            },
        )
    }

    #[tokio::test]
    async fn with_no_handlers_returns_terminal_equivalent() {
        let registry = MiddlewareRegistry::new();
        let t = trace();
        let result = registry
            .invoke::<TestStage>("ctx".to_owned(), &EventSink::null(), terminal(Arc::clone(&t)))
            .await
            .unwrap();
        assert_eq!(result, "terminal:ctx");
        assert!(!registry.has::<TestStage>());
    }

    #[tokio::test]
    async fn executes_handlers_in_registration_order_outermost_first() {
        let registry = MiddlewareRegistry::new();
        let t = trace();
        registry.add::<TestStage>(recording_wrap(Arc::clone(&t), "first"));
        registry.add::<TestStage>(recording_wrap(Arc::clone(&t), "second"));
        registry
            .invoke::<TestStage>("ctx".to_owned(), &EventSink::null(), terminal(Arc::clone(&t)))
            .await
            .unwrap();
        assert_eq!(
            *t.lock(),
            [
                "first:before",
                "second:before",
                "terminal:ctx",
                "second:after",
                "first:after"
            ]
        );
    }

    #[tokio::test]
    async fn does_not_call_next_handlers_or_terminal_when_middleware_does_not_call_next() {
        let registry = MiddlewareRegistry::new();
        let t = trace();
        registry.add::<TestStage>(wrap_fn(
            |_ctx: String, _sink: EventSink<String>, _next: Next<TestStage>| async move {
                Ok("short-circuit".to_owned())
            },
        ));
        registry.add::<TestStage>(recording_wrap(Arc::clone(&t), "inner"));
        let result = registry
            .invoke::<TestStage>("ctx".to_owned(), &EventSink::null(), terminal(Arc::clone(&t)))
            .await
            .unwrap();
        assert_eq!(result, "short-circuit");
        assert!(t.lock().is_empty());
    }

    #[tokio::test]
    async fn forwards_all_events_from_terminal_and_returns_terminal_result() {
        let registry = MiddlewareRegistry::new();
        let t = trace();
        registry.add::<TestStage>(recording_wrap(Arc::clone(&t), "wrap"));
        let (sink, events) = EventSink::collector();
        let result = registry
            .invoke::<TestStage>("ctx".to_owned(), &sink, emitting_terminal(vec!["a", "b"]))
            .await
            .unwrap();
        assert_eq!(result, "done");
        assert_eq!(*events.lock(), ["a", "b"]);
    }

    #[tokio::test]
    async fn only_yields_events_matching_a_predicate() {
        let registry = MiddlewareRegistry::new();
        registry.add::<TestStage>(wrap_fn(
            |ctx: String, sink: EventSink<String>, next: Next<TestStage>| async move {
                let filtered = sink.filter(|event: &String| event.starts_with("keep"));
                next.run(ctx, &filtered).await
            },
        ));
        let (sink, events) = EventSink::collector();
        registry
            .invoke::<TestStage>(
                "ctx".to_owned(),
                &sink,
                emitting_terminal(vec!["keep-1", "drop-1", "keep-2"]),
            )
            .await
            .unwrap();
        assert_eq!(*events.lock(), ["keep-1", "keep-2"]);
    }

    #[tokio::test]
    async fn can_yield_events_without_calling_next() {
        let registry = MiddlewareRegistry::new();
        registry.add::<TestStage>(wrap_fn(
            |_ctx: String, sink: EventSink<String>, _next: Next<TestStage>| async move {
                sink.send("synthetic".to_owned()).await?;
                Ok("synthetic-result".to_owned())
            },
        ));
        let (sink, events) = EventSink::collector();
        let t = trace();
        let result = registry
            .invoke::<TestStage>("ctx".to_owned(), &sink, terminal(Arc::clone(&t)))
            .await
            .unwrap();
        assert_eq!(result, "synthetic-result");
        assert_eq!(*events.lock(), ["synthetic"]);
        assert!(t.lock().is_empty());
    }

    #[tokio::test]
    async fn each_middleware_can_further_modify_context() {
        let registry = MiddlewareRegistry::new();
        let t = trace();
        registry.add_input::<TestStage>(|ctx: String| Ok(format!("{ctx}+a")));
        registry.add_input::<TestStage>(|ctx: String| Ok(format!("{ctx}+b")));
        let result = registry
            .invoke::<TestStage>("ctx".to_owned(), &EventSink::null(), terminal(Arc::clone(&t)))
            .await
            .unwrap();
        assert_eq!(result, "terminal:ctx+a+b");
    }

    #[tokio::test]
    async fn output_transforms_result_after_the_call() {
        let registry = MiddlewareRegistry::new();
        let t = trace();
        registry.add_output::<TestStage>(|result: String| Ok(format!("{result}!")));
        let result = registry
            .invoke::<TestStage>("ctx".to_owned(), &EventSink::null(), terminal(Arc::clone(&t)))
            .await
            .unwrap();
        assert_eq!(result, "terminal:ctx!");
    }

    #[tokio::test]
    async fn input_wrap_output_execution_order() {
        let registry = MiddlewareRegistry::new();
        let t = trace();
        // Registered out of order on purpose: phases, not registration, decide layering.
        registry.add::<TestStage>(recording_wrap(Arc::clone(&t), "wrap"));
        registry.add_output::<TestStage>({
            let t = Arc::clone(&t);
            move |result: String| {
                t.lock().push("output".to_owned());
                Ok(result)
            }
        });
        registry.add_input::<TestStage>({
            let t = Arc::clone(&t);
            move |ctx: String| {
                t.lock().push("input".to_owned());
                Ok(ctx)
            }
        });
        registry
            .invoke::<TestStage>("ctx".to_owned(), &EventSink::null(), terminal(Arc::clone(&t)))
            .await
            .unwrap();
        assert_eq!(
            *t.lock(),
            ["input", "wrap:before", "terminal:ctx", "wrap:after", "output"]
        );
    }

    #[tokio::test]
    async fn errors_from_terminal_reach_middleware_and_caller() {
        let registry = MiddlewareRegistry::new();
        let seen = Arc::new(Mutex::new(None));
        registry.add::<TestStage>(wrap_fn({
            let seen = Arc::clone(&seen);
            move |ctx: String, sink: EventSink<String>, next: Next<TestStage>| {
                let seen = Arc::clone(&seen);
                async move {
                    let result = next.run(ctx, &sink).await;
                    if let Err(error) = &result {
                        *seen.lock() = Some(error.to_string());
                    }
                    result
                }
            }
        }));
        let failing = |_ctx: String, _sink: EventSink<String>| async move {
            Err::<String, Error>(Error::other("terminal boom"))
        };
        let error = registry
            .invoke::<TestStage>("ctx".to_owned(), &EventSink::null(), failing)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("terminal boom"));
        assert!(seen.lock().as_deref().unwrap().contains("terminal boom"));
    }

    #[tokio::test]
    async fn two_stages_with_the_same_name_are_distinct() {
        let registry = MiddlewareRegistry::new();
        let t = trace();
        registry.add::<OtherStage>(wrap_fn(
            |_ctx: String, _sink: EventSink<String>, _next: Next<OtherStage>| async move {
                Ok("other".to_owned())
            },
        ));
        assert_eq!(TestStage::name(), OtherStage::name());
        assert!(registry.has::<OtherStage>());
        assert!(!registry.has::<TestStage>());
        let result = registry
            .invoke::<TestStage>("ctx".to_owned(), &EventSink::null(), terminal(Arc::clone(&t)))
            .await
            .unwrap();
        assert_eq!(result, "terminal:ctx");
    }

    #[tokio::test]
    async fn remove_handler_so_it_no_longer_runs() {
        let registry = MiddlewareRegistry::new();
        let t = trace();
        let handle = registry.add::<TestStage>(recording_wrap(Arc::clone(&t), "removed"));
        registry.add::<TestStage>(recording_wrap(Arc::clone(&t), "kept"));
        registry.remove(handle);
        registry
            .invoke::<TestStage>("ctx".to_owned(), &EventSink::null(), terminal(Arc::clone(&t)))
            .await
            .unwrap();
        assert_eq!(*t.lock(), ["kept:before", "terminal:ctx", "kept:after"]);
        registry.remove(handle);
    }

    #[test]
    fn remove_is_a_no_op_when_stage_has_no_handlers() {
        let registry = MiddlewareRegistry::new();
        let handle = registry.add::<TestStage>(wrap_fn(
            |_ctx: String, _sink: EventSink<String>, _next: Next<TestStage>| async move { Ok(String::new()) },
        ));
        registry.remove(handle);
        registry.remove(handle);
        assert!(!registry.has::<TestStage>());
    }

    #[tokio::test]
    async fn cleanup_runs_when_caller_abandons_the_call_mid_stream() {
        struct Guard(Trace, &'static str);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.lock().push(self.1.to_owned());
            }
        }

        let registry = MiddlewareRegistry::new();
        let t = trace();
        registry.add::<TestStage>(wrap_fn({
            let t = Arc::clone(&t);
            move |ctx: String, sink: EventSink<String>, next: Next<TestStage>| {
                let t = Arc::clone(&t);
                async move {
                    let _guard = Guard(t, "outer-cleanup");
                    next.run(ctx, &sink).await
                }
            }
        }));
        let hanging = |_ctx: String, _sink: EventSink<String>| async move {
            futures::future::pending::<()>().await;
            Ok(String::new())
        };
        let sink = EventSink::null();
        let call = registry.invoke::<TestStage>("ctx".to_owned(), &sink, hanging);
        assert!(tokio::time::timeout(std::time::Duration::from_millis(20), call)
            .await
            .is_err());
        assert_eq!(*t.lock(), ["outer-cleanup"]);
    }
}
