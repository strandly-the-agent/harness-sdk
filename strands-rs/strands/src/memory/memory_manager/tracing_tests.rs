//! Ports `strands-ts/src/memory/__tests__/memory-tracing.test.ts`. TS spies on the tracer's
//! methods; these tests read the `memory.*` spans a recording exporter received.
#![cfg(feature = "otel")]

use async_trait::async_trait;
use serde_json::json;

use super::*;
use crate::memory::test_support::{MockStore, Sink, tool_result, user};
use crate::memory::{
    ExtractionConfig, ExtractionResult, Extractor, ExtractorContext, InvocationTrigger, MemorySetting,
};
use crate::telemetry::tests::{recorded_with_status, recording};
use crate::testing::MockMessageModel;

fn manager(stores: Vec<MockStore>) -> MemoryManager {
    MemoryManager::new(MemoryManagerConfig {
        stores: stores
            .into_iter()
            .map(|store| Arc::new(store) as Arc<dyn MemoryStore>)
            .collect(),
        injection: Some(MemorySetting::Off),
        ..Default::default()
    })
    .expect("a manager")
}

fn named<'a>(spans: &'a [JsonValue], name: &str) -> Vec<&'a JsonValue> {
    spans.iter().filter(|span| span["name"] == name).collect()
}

fn event_content(span: &JsonValue, name: &str) -> Option<String> {
    span["events"]
        .as_array()?
        .iter()
        .find(|event| event["name"] == name)
        .and_then(|event| event["attributes"]["content"].as_str().map(str::to_owned))
}

mod search {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn starts_and_ends_a_search_span_with_the_retrieved_entries() {
        let (recorder, _guard) = recording();
        let manager = manager(vec![MockStore::new("personal").with_entries(&["x"])]);
        let results = manager
            .search("preferences", MemorySearchOptions::default())
            .await
            .unwrap();
        assert_eq!(results.len(), 1);

        let spans = recorded_with_status(&recorder.spans());
        assert_eq!(spans.len(), 1);
        let span = &spans[0];
        assert_eq!(span["name"], "memory.search");
        assert_eq!(span["attributes"]["gen_ai.operation.name"], "memory.search");
        assert_eq!(span["attributes"]["memory.store.names"], r#"["personal"]"#);
        assert_eq!(span["attributes"]["memory.store.count"], 1);
        assert_eq!(span["attributes"]["memory.result.count"], 1);
        assert_eq!(span["attributes"]["memory.store.failure_count"], 0);
        assert_eq!(
            event_content(span, "memory.query").as_deref(),
            Some("preferences")
        );
        let entries: JsonValue =
            serde_json::from_str(&event_content(span, "memory.results").unwrap()).unwrap();
        assert_eq!(entries, json!([{ "content": "x", "storeName": "personal" }]));
        assert_eq!(span["status"], "OK");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn counts_a_per_store_failure_but_ends_the_span_ok_not_an_error() {
        let (recorder, _guard) = recording();
        let bad = MockStore {
            search_error: Some("boom".to_owned()),
            ..MockStore::new("bad")
        };
        let manager = manager(vec![MockStore::new("good").with_entries(&["hit"]), bad]);
        let results = manager.search("q", MemorySearchOptions::default()).await.unwrap();
        assert_eq!(results.len(), 1);

        let spans = recorded_with_status(&recorder.spans());
        assert_eq!(spans[0]["attributes"]["memory.store.failure_count"], 1);
        assert_eq!(spans[0]["status"], "OK");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ends_the_span_with_an_error_when_a_named_store_is_unknown() {
        let (recorder, _guard) = recording();
        let manager = manager(vec![MockStore::new("personal")]);
        let options = MemorySearchOptions {
            stores: Some(vec!["missing".to_owned()]),
            ..MemorySearchOptions::default()
        };
        let error = manager.search("q", options).await.unwrap_err();
        assert_eq!(error.to_string(), "MemoryManager: store 'missing' not found");

        let spans = recorded_with_status(&recorder.spans());
        assert_eq!(
            spans[0]["status"],
            json!({ "error": "MemoryManager: store 'missing' not found" })
        );
        assert_eq!(event_content(&spans[0], "memory.results"), None);
    }
}

mod add {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn starts_and_ends_an_add_span_on_success() {
        let (recorder, _guard) = recording();
        let manager = manager(vec![MockStore::writable("personal")]);
        manager
            .add("remember this", MemoryAddOptions::default())
            .await
            .unwrap();

        let spans = recorded_with_status(&recorder.spans());
        assert_eq!(spans.len(), 1);
        let span = &spans[0];
        assert_eq!(span["name"], "memory.add");
        assert_eq!(span["attributes"]["memory.store.names"], r#"["personal"]"#);
        assert_eq!(span["attributes"]["memory.store.failure_count"], 0);
        assert_eq!(
            event_content(span, "memory.content").as_deref(),
            Some("remember this")
        );
        assert_eq!(span["status"], "OK");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ends_the_span_with_the_aggregate_error_when_a_write_fails() {
        let (recorder, _guard) = recording();
        let store = MockStore::writable("personal");
        store.set_failing(true);
        let manager = manager(vec![store]);
        let error = manager
            .add("remember this", MemoryAddOptions::default())
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "MemoryManager: store writes failed: personal");

        let spans = recorded_with_status(&recorder.spans());
        assert_eq!(spans[0]["attributes"]["memory.store.failure_count"], 1);
        assert_eq!(
            spans[0]["status"],
            json!({ "error": "MemoryManager: store writes failed: personal" })
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn forces_a_root_span_for_the_detached_fire_and_forget_path() {
        let (recorder, _guard) = recording();
        let manager = manager(vec![MockStore::writable("personal")]);
        let options = MemoryAddOptions {
            stores: Some(vec!["personal".to_owned()]),
            ..MemoryAddOptions::default()
        };
        let caller = tracing::info_span!("caller");
        manager
            .add_inner("attached", &options, false)
            .instrument(caller.clone())
            .await
            .map_err(AddFailure::into_error)
            .unwrap();
        manager
            .add_inner("detached", &options, true)
            .instrument(caller.clone())
            .await
            .map_err(AddFailure::into_error)
            .unwrap();
        drop(caller);

        let spans = recorded_with_status(&recorder.spans());
        let adds = named(&spans, "memory.add");
        assert_eq!(adds[0]["parent"], "caller");
        assert_eq!(adds[1]["parent"], JsonValue::Null);
    }
}

mod injection {
    use super::*;

    fn inject_span(spans: &[JsonValue]) -> &JsonValue {
        named(spans, "memory.inject")[0]
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ends_the_inject_span_injected_true_with_the_entry_count_on_the_happy_path() {
        let (recorder, _guard) = recording();
        let manager = manager(vec![
            MockStore::new("personal").with_entries(&["likes dark mode"]),
        ]);
        let rendered = manager
            .provide_memory_context(&[user("hi")], &MemoryInjectionConfig::default())
            .await
            .unwrap()
            .expect("injected text");
        assert!(rendered.contains("dark mode"));

        let spans = recorded_with_status(&recorder.spans());
        let inject = inject_span(&spans);
        assert_eq!(inject["attributes"]["memory.injected"], true);
        assert_eq!(inject["attributes"]["memory.entry.count"], 1);
        assert_eq!(inject["attributes"]["memory.max_entries"], 5);
        assert_eq!(inject["status"], "OK");
        // TS runs the search inside the inject span's context.
        assert_eq!(named(&spans, "memory.search")[0]["parent"], "memory.inject");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ends_injected_false_when_no_query_can_be_derived() {
        let (recorder, _guard) = recording();
        let manager = manager(vec![MockStore::new("personal")]);
        let rendered = manager
            .provide_memory_context(&[], &MemoryInjectionConfig::default())
            .await
            .unwrap();
        assert_eq!(rendered, None);

        let spans = recorded_with_status(&recorder.spans());
        assert_eq!(inject_span(&spans)["attributes"]["memory.injected"], false);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fails_open_with_format_error_true_when_the_format_callback_throws() {
        let (recorder, _guard) = recording();
        let manager = manager(vec![MockStore::new("personal").with_entries(&["x"])]);
        let config = MemoryInjectionConfig {
            format: Some(Arc::new(|_| Err(Error::other("bad format")))),
            ..MemoryInjectionConfig::default()
        };
        let rendered = manager
            .provide_memory_context(&[user("hi")], &config)
            .await
            .unwrap();
        assert_eq!(rendered, None);

        let spans = recorded_with_status(&recorder.spans());
        let inject = inject_span(&spans);
        assert_eq!(inject["attributes"]["memory.injected"], false);
        assert_eq!(inject["attributes"]["memory.inject.format_error"], true);
        assert_eq!(inject["status"], "OK");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fails_open_ends_injected_false_when_the_query_callback_throws() {
        let (recorder, _guard) = recording();
        let manager = manager(vec![MockStore::new("personal")]);
        let config = MemoryInjectionConfig {
            query: Some(Arc::new(|_| Err(Error::other("query exploded")))),
            ..MemoryInjectionConfig::default()
        };
        let rendered = manager
            .provide_memory_context(&[user("hi")], &config)
            .await
            .unwrap();
        assert_eq!(rendered, None);

        let spans = recorded_with_status(&recorder.spans());
        assert_eq!(inject_span(&spans)["attributes"]["memory.injected"], false);
    }
}

mod extraction_coordinator {
    use super::*;

    struct TwoFacts;

    #[async_trait]
    impl Extractor for TwoFacts {
        async fn extract(&self, _: &[Message], _: &ExtractorContext) -> Result<Vec<ExtractionResult>> {
            Ok(vec![
                ExtractionResult::new("fact one"),
                ExtractionResult::new("fact two"),
            ])
        }
    }

    fn extraction(extractor: Option<Arc<dyn Extractor>>) -> ExtractionConfig {
        ExtractionConfig {
            trigger: Some(vec![Arc::new(InvocationTrigger::new())]),
            extractor,
            ..Default::default()
        }
    }

    fn coordinator(store: &Arc<MockStore>) -> ExtractionCoordinator {
        let binding = ExtractionBinding {
            store: store.clone(),
            config: resolve_extraction_config(store.extraction(), store.as_ref())
                .expect("an extraction config"),
        };
        ExtractionCoordinator::new(
            vec![binding],
            Arc::new(MockMessageModel::new()),
            MemoryTracer::default(),
        )
    }

    fn passthrough_store() -> Arc<MockStore> {
        Arc::new(MockStore::extracting(
            "personal",
            Some(extraction(None)),
            Sink::AddMessages,
        ))
    }

    #[tokio::test(flavor = "current_thread")]
    async fn starts_a_root_extract_span_and_ends_it_ok_on_a_successful_save() {
        let (recorder, _guard) = recording();
        let store = passthrough_store();
        let coordinator = coordinator(&store);
        coordinator.record(user("remember dark mode"));
        let caller = tracing::info_span!("caller");
        coordinator.process(0, None).instrument(caller.clone()).await;
        drop(caller);

        let spans = recorded_with_status(&recorder.spans());
        let extracts = named(&spans, "memory.extract");
        assert_eq!(extracts.len(), 1);
        let extract = extracts[0];
        assert_eq!(extract["parent"], JsonValue::Null);
        assert_eq!(extract["attributes"]["memory.store.name"], "personal");
        assert_eq!(extract["attributes"]["memory.message.count"], 1);
        assert_eq!(extract["attributes"]["memory.message.filtered_count"], 0);
        assert_eq!(extract["attributes"]["memory.entry.count"], 1);
        assert_eq!(extract["status"], "OK");
        assert_eq!(store.add_messages_count(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ends_the_extract_span_with_the_error_and_swallows_the_failure() {
        let (recorder, _guard) = recording();
        let store = passthrough_store();
        store.set_failing(true);
        let coordinator = coordinator(&store);
        coordinator.record(user("remember dark mode"));
        coordinator.process(0, None).await;

        let spans = recorded_with_status(&recorder.spans());
        assert_eq!(
            named(&spans, "memory.extract")[0]["status"],
            json!({ "error": "write error" })
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn creates_no_span_when_there_are_no_fresh_messages() {
        let (recorder, _guard) = recording();
        let store = passthrough_store();
        coordinator(&store).process(0, None).await;
        assert!(recorder.spans().is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn passes_the_written_entry_count_to_the_span_end_extractor_route() {
        let (recorder, _guard) = recording();
        let config = extraction(Some(Arc::new(TwoFacts)));
        let store = Arc::new(MockStore::extracting("personal", Some(config), Sink::Add));
        let coordinator = coordinator(&store);
        coordinator.record(user("remember two things"));
        coordinator.process(0, None).await;

        let spans = recorded_with_status(&recorder.spans());
        assert_eq!(
            named(&spans, "memory.extract")[0]["attributes"]["memory.entry.count"],
            2
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn records_entry_count_0_when_all_messages_are_filtered_out() {
        let (recorder, _guard) = recording();
        let store = passthrough_store();
        let coordinator = coordinator(&store);
        // The default filter drops tool result blocks, so nothing is left to save.
        coordinator.record(tool_result());
        coordinator.process(0, None).await;

        assert_eq!(store.add_messages_count(), 0);
        let spans = recorded_with_status(&recorder.spans());
        let extract = named(&spans, "memory.extract")[0];
        assert_eq!(extract["attributes"]["memory.entry.count"], 0);
        assert_eq!(extract["attributes"]["memory.message.count"], 0);
        assert_eq!(extract["attributes"]["memory.message.filtered_count"], 1);
    }

    /// TS `startMemoryExtractSpan attaches a valid agent span context as a link`
    /// (`tracer.test.node.ts`): the run that scheduled the save is linked and its ids recorded.
    #[tokio::test(flavor = "current_thread")]
    async fn the_extract_span_links_the_span_that_scheduled_it() {
        let (recorder, _guard) = recording();
        let store = passthrough_store();
        let coordinator = coordinator(&store);
        coordinator.record(user("remember dark mode"));
        let caller = tracing::info_span!("caller");
        let save = caller.in_scope(|| coordinator.process(0, None));
        save.await;
        drop(caller);

        let spans = recorder.spans();
        let caller = spans
            .iter()
            .find(|span| span.name == "caller")
            .expect("the caller span");
        let extract = spans
            .iter()
            .find(|span| span.name == "memory.extract")
            .expect("the extract span");
        assert_eq!(extract.links.len(), 1);
        assert_eq!(
            extract.links[0].span_context.span_id(),
            caller.span_context.span_id()
        );
        let json = recorded_with_status(&spans);
        let attributes = &named(&json, "memory.extract")[0]["attributes"];
        assert_eq!(
            attributes["memory.parent.trace_id"],
            caller.span_context.trace_id().to_string()
        );
        assert_eq!(
            attributes["memory.parent.span_id"],
            caller.span_context.span_id().to_string()
        );
    }

    // No TS case: the detached driver of a save doesn't keep the span that scheduled it open.
    #[tokio::test(flavor = "current_thread")]
    async fn the_callers_span_is_exported_before_the_detached_save_runs() {
        let (recorder, _guard) = recording();
        let store = passthrough_store();
        let coordinator = coordinator(&store);
        coordinator.record(user("remember dark mode"));
        let caller = tracing::info_span!("caller");
        drop(caller.in_scope(|| coordinator.process(0, None)));
        drop(caller);

        let names: Vec<_> = recorder
            .spans()
            .iter()
            .map(|span| span.name.to_string())
            .collect();
        assert_eq!(names, ["caller"]);
    }
}
