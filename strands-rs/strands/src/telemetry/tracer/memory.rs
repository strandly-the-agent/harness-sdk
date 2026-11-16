//! Memory spans (TS `Tracer.startMemory*Span`/`endMemory*Span` in `telemetry/tracer.ts`):
//! `memory.search`, `memory.add`, `memory.inject` and the detached `memory.extract`.

use tracing::Span;

use super::{LocalTracer, end_span, int, keys, now_iso, otel_span};
use crate::memory::MemoryEntry;
use crate::telemetry::config::{Semconv, process_env};
use crate::telemetry::json::to_json;

/// A memory span with every key it may record declared; `root` detaches it from the current span.
macro_rules! memory_span {
    ($name:literal) => {
        memory_span!(@open $name,)
    };
    ($name:literal, root) => {
        memory_span!(@open $name, parent: None,)
    };
    (@open $name:literal, $($parent:tt)*) => {
        tracing::info_span!(
            $($parent)* $name,
            otel.status_code = tracing::field::Empty,
            otel.status_description = tracing::field::Empty,
            gen_ai.event.start_time = tracing::field::Empty,
            gen_ai.event.end_time = tracing::field::Empty,
            gen_ai.operation.name = $name,
            gen_ai.system = tracing::field::Empty,
            gen_ai.provider.name = tracing::field::Empty,
            memory.store.names = tracing::field::Empty,
            memory.store.count = tracing::field::Empty,
            memory.max_search_results = tracing::field::Empty,
            memory.result.count = tracing::field::Empty,
            memory.store.failure_count = tracing::field::Empty,
            memory.max_entries = tracing::field::Empty,
            memory.injected = tracing::field::Empty,
            memory.entry.count = tracing::field::Empty,
            memory.inject.format_error = tracing::field::Empty,
            memory.store.name = tracing::field::Empty,
            memory.message.count = tracing::field::Empty,
            memory.message.filtered_count = tracing::field::Empty,
            memory.parent.trace_id = tracing::field::Empty,
            memory.parent.span_id = tracing::field::Empty,
        )
    };
}

fn count(value: usize) -> i64 {
    int(u64::try_from(value).unwrap_or(u64::MAX))
}

/// The span context of the agent run that scheduled an extraction (TS `SpanContext`). It exists
/// only with the `otel` feature.
#[cfg(feature = "otel")]
pub(crate) type SpanLink = opentelemetry::trace::SpanContext;
/// Without `otel` there is no span context to link.
#[cfg(not(feature = "otel"))]
#[derive(Debug, Clone)]
pub(crate) enum SpanLink {}

/// TS `currentSpanContext`: the current span's context when it is valid.
pub(crate) fn current_span_context() -> Option<SpanLink> {
    #[cfg(feature = "otel")]
    {
        use opentelemetry::trace::TraceContextExt as _;
        use tracing_opentelemetry::OpenTelemetrySpanExt as _;

        let span = Span::current();
        if span.is_disabled() {
            return None;
        }
        let context = span.context();
        let span_context = context.span().span_context().clone();
        span_context.is_valid().then_some(span_context)
    }
    #[cfg(not(feature = "otel"))]
    None
}

/// Records the scheduling run on the detached extract span: its ids as attributes and a link.
fn link_parent(span: &Span, link: Option<SpanLink>) {
    #[cfg(feature = "otel")]
    if let Some(link) = link.filter(SpanLink::is_valid) {
        use tracing_opentelemetry::OpenTelemetrySpanExt as _;

        span.record("memory.parent.trace_id", link.trace_id().to_string().as_str());
        span.record("memory.parent.span_id", link.span_id().to_string().as_str());
        span.add_link(link);
    }
    #[cfg(not(feature = "otel"))]
    let _ = (span, link);
}

/// What `start_extract` records (TS `StartMemoryExtractSpanOptions`).
pub(crate) struct ExtractSpanStart<'a> {
    pub(crate) store_name: &'a str,
    pub(crate) message_count: usize,
    pub(crate) filtered_count: usize,
    pub(crate) agent_span_context: Option<SpanLink>,
}

/// The tracer of a memory manager and its extraction coordinator (TS `MemoryManager._tracer`).
#[derive(Debug, Clone)]
pub(crate) struct MemoryTracer {
    semconv: Semconv,
}

impl Default for MemoryTracer {
    fn default() -> Self {
        Self::new(Semconv::from_env(&process_env))
    }
}

impl MemoryTracer {
    pub(crate) fn new(semconv: Semconv) -> Self {
        Self { semconv }
    }

    /// A tracer for the extractor's model call (TS hands the extractor the same `Tracer`).
    pub(crate) fn model_tracer(&self) -> LocalTracer {
        LocalTracer::new(self.semconv.clone())
    }

    fn start(&self, span: &Span, store_names: Option<&[String]>) {
        let provider_key = if self.semconv.latest {
            keys::PROVIDER_NAME
        } else {
            keys::SYSTEM
        };
        span.record(provider_key, self.semconv.service_name.as_str());
        span.record(keys::START_TIME, now_iso().as_str());
        if let Some(store_names) = store_names {
            span.record("memory.store.names", to_json(&store_names).as_str());
            span.record("memory.store.count", count(store_names.len()));
        }
    }

    /// TS `_addEvent(.., this._spanAttributesOnly)`: the content as an event, or as the span's
    /// `content` attribute under Langfuse and `gen_ai_span_attributes_only`.
    fn add_content(&self, span: &Span, event: &'static str, content: String) {
        if self.semconv.span_attributes_only {
            otel_span::set_attribute(span, "content", &content.into());
        } else {
            otel_span::add_event(span, event, vec![("content", content)]);
        }
    }

    /// TS `startMemorySearchSpan`.
    pub(crate) fn start_search(
        &self,
        query: &str,
        store_names: &[String],
        max_search_results: Option<usize>,
    ) -> Span {
        let span = memory_span!("memory.search");
        if !span.is_disabled() {
            self.start(&span, Some(store_names));
            if let Some(max_search_results) = max_search_results {
                span.record("memory.max_search_results", count(max_search_results));
            }
            // As a span attribute the results replace the query in TS, so `end_search` writes the
            // one `content` the span ends with.
            if !self.semconv.span_attributes_only {
                self.add_content(&span, "memory.query", query.to_owned());
            }
        }
        span
    }

    /// TS `endMemorySearchSpan`: the results event is skipped on an error.
    pub(crate) fn end_search(
        &self,
        span: &Span,
        query: &str,
        entries: &[MemoryEntry],
        store_failure_count: usize,
        error: Option<&str>,
    ) {
        if span.is_disabled() {
            return;
        }
        span.record("memory.result.count", count(entries.len()));
        span.record("memory.store.failure_count", count(store_failure_count));
        if error.is_none() {
            self.add_content(span, "memory.results", to_json(&entries));
        } else if self.semconv.span_attributes_only {
            self.add_content(span, "memory.query", query.to_owned());
        }
        end_span(span, error);
    }

    /// TS `startMemoryAddSpan`; `detached` forces a root span (TS `forceRoot`), for a write that
    /// outlives the tool call that scheduled it.
    pub(crate) fn start_add(&self, content: &str, store_names: &[String], detached: bool) -> Span {
        let span = if detached {
            memory_span!("memory.add", root)
        } else {
            memory_span!("memory.add")
        };
        if !span.is_disabled() {
            self.start(&span, Some(store_names));
            self.add_content(&span, "memory.content", content.to_owned());
        }
        span
    }

    /// TS `endMemoryAddSpan`.
    pub(crate) fn end_add(&self, span: &Span, store_failure_count: usize, error: Option<&str>) {
        if span.is_disabled() {
            return;
        }
        span.record("memory.store.failure_count", count(store_failure_count));
        end_span(span, error);
    }

    /// TS `startMemoryInjectSpan`.
    pub(crate) fn start_inject(&self, max_entries: Option<usize>) -> Span {
        let span = memory_span!("memory.inject");
        if !span.is_disabled() {
            self.start(&span, None);
            if let Some(max_entries) = max_entries {
                span.record("memory.max_entries", count(max_entries));
            }
        }
        span
    }

    /// TS `endMemoryInjectSpan`: a format error fails open, so the span still ends OK.
    pub(crate) fn end_inject(&self, span: &Span, injected: bool, entry_count: usize, format_error: bool) {
        if span.is_disabled() {
            return;
        }
        span.record("memory.injected", injected);
        span.record("memory.entry.count", count(entry_count));
        if format_error {
            span.record("memory.inject.format_error", true);
        }
        end_span(span, None);
    }

    /// TS `startMemoryExtractSpan`: a root span, linked to the agent run that scheduled it.
    pub(crate) fn start_extract(&self, start: ExtractSpanStart<'_>) -> Span {
        let span = memory_span!("memory.extract", root);
        if !span.is_disabled() {
            self.start(&span, None);
            span.record("memory.store.name", start.store_name);
            span.record("memory.message.count", count(start.message_count));
            span.record("memory.message.filtered_count", count(start.filtered_count));
            link_parent(&span, start.agent_span_context);
        }
        span
    }

    /// TS `endMemoryExtractSpan`.
    pub(crate) fn end_extract(&self, span: &Span, entry_count: Option<usize>, error: Option<&str>) {
        if span.is_disabled() {
            return;
        }
        if let Some(entry_count) = entry_count {
            span.record("memory.entry.count", count(entry_count));
        }
        end_span(span, error);
    }
}
