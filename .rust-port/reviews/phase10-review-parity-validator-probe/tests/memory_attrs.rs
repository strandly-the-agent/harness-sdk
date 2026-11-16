//! Run with OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_span_attributes_only.
//! TS tracer.ts:672 and :698-712 both call `span.setAttributes({ content })` on the `memory.search` span, so the
//! results replace the query and the span ends with one `content` attribute.
use std::sync::Arc;

use pv_probe::{attrs, dump, recording};
use strands::memory::{MemoryEntry, MemoryManager, MemoryManagerConfig, MemorySearchOptions, MemoryStore, SearchOptions};

struct Fixed;

#[strands::async_trait]
impl MemoryStore for Fixed {
    fn name(&self) -> &str {
        "fixed"
    }

    fn writable(&self) -> bool {
        false
    }

    async fn search(&self, query: &str, _options: SearchOptions) -> strands::Result<Vec<MemoryEntry>> {
        Ok(vec![MemoryEntry::new(format!("echo {query}"))])
    }
}

#[tokio::test(flavor = "current_thread")]
async fn the_memory_search_span_ends_with_one_content_attribute() {
    assert_eq!(std::env::var("OTEL_SEMCONV_STABILITY_OPT_IN").as_deref(), Ok("gen_ai_span_attributes_only"));
    let (recorder, _guard) = recording();
    let manager = MemoryManager::new(
        MemoryManagerConfig::builder()
            .stores(vec![Arc::new(Fixed) as Arc<dyn MemoryStore>])
            .injection(false)
            .build(),
    )
    .unwrap();
    let found = manager.search("dark mode", MemorySearchOptions::default()).await.unwrap();
    assert_eq!(found.len(), 1);
    let spans = recorder.spans();
    dump(&spans);
    let search = spans.iter().find(|s| s.name == "memory.search").expect("memory.search span");
    let content = attrs(search, "content");
    assert_eq!(content.len(), 1, "TS leaves one `content` (the results); Rust exported {content:?}");
}
