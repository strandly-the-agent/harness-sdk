//! Buffers conversation messages and saves them to each extraction store in the background (TS
//! `memory/extraction/coordinator.ts`).

use std::collections::HashSet;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use futures::FutureExt;
use futures::future::{BoxFuture, Shared, join_all};
use tracing::Instrument;

use super::resolve_extraction_config::ResolvedExtractionConfig;
use super::types::{ExtractorContext, MemoryContentBlockType, MemoryMessageFilter};
use crate::agent::InvocationContext;
use crate::error::{Error, Result};
use crate::memory::{AddMessagesContext, MemoryStore};
use crate::models::model::Model;
use crate::telemetry::tracer::memory::{ExtractSpanStart, MemoryTracer, SpanLink, current_span_context};
use crate::types::messages::Message;

/// A store with its resolved extraction config (TS `ExtractionBinding`).
#[derive(Clone)]
pub(crate) struct ExtractionBinding {
    pub(crate) store: Arc<dyn MemoryStore>,
    pub(crate) config: ResolvedExtractionConfig,
}

/// Consecutive failed saves before a store's saves back off (TS `SAVE_FAILURES_BEFORE_BACKOFF`).
pub(crate) const SAVE_FAILURES_BEFORE_BACKOFF: u32 = 10;
/// While backing off, one trigger in this many is attempted (TS `BACKOFF_PROBE_INTERVAL`).
pub(crate) const BACKOFF_PROBE_INTERVAL: u32 = 3;

#[derive(Clone)]
struct BufferedMessage {
    seq: u64,
    message: Message,
}

type Chain = Shared<BoxFuture<'static, ()>>;

#[derive(Default)]
struct StoreState {
    /// The highest sequence number saved (or being saved) to this store.
    mark: Option<u64>,
    /// The tail of this store's queue of saves, which run one at a time.
    chain: Option<Chain>,
    consecutive_failures: u32,
    backoff_counter: u32,
}

#[derive(Default)]
struct State {
    pending: Vec<BufferedMessage>,
    next_seq: u64,
    stores: Vec<StoreState>,
    /// Bumped on every enqueue, so `flush` can tell whether a save started while it waited.
    generation: u64,
}

struct Inner {
    bindings: Vec<ExtractionBinding>,
    default_model: Arc<dyn Model>,
    /// Shared with the memory manager (TS `_tracer`).
    tracer: MemoryTracer,
    state: Mutex<State>,
}

/// Saves buffered messages to the extraction stores, one save at a time per store (TS
/// `ExtractionCoordinator`). Stores are addressed by their index in the bindings.
#[derive(Clone)]
pub(crate) struct ExtractionCoordinator {
    inner: Arc<Inner>,
}

fn filter_messages(buffered: &[BufferedMessage], filter: &MemoryMessageFilter) -> Vec<BufferedMessage> {
    let exclude: HashSet<MemoryContentBlockType> = filter.exclude.iter().copied().collect();
    buffered
        .iter()
        .filter_map(|buffered| {
            let content: Vec<_> = buffered
                .message
                .content()
                .iter()
                .filter(|block| MemoryContentBlockType::of(block).is_none_or(|kind| !exclude.contains(&kind)))
                .cloned()
                .collect();
            if content.is_empty() {
                return None;
            }
            let mut message = Message::new(buffered.message.role(), content);
            message.set_metadata(buffered.message.metadata().cloned());
            Some(BufferedMessage {
                seq: buffered.seq,
                message,
            })
        })
        .collect()
}

impl ExtractionCoordinator {
    pub(crate) fn new(
        bindings: Vec<ExtractionBinding>,
        default_model: Arc<dyn Model>,
        tracer: MemoryTracer,
    ) -> Self {
        let state = State {
            stores: bindings.iter().map(|_| StoreState::default()).collect(),
            ..State::default()
        };
        Self {
            inner: Arc::new(Inner {
                bindings,
                default_model,
                tracer,
                state: Mutex::new(state),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether a message is buffered that some store hasn't saved.
    pub(crate) fn has_unsaved(&self) -> bool {
        !self.lock().pending.is_empty()
    }

    /// Buffers a message the agent added.
    pub(crate) fn record(&self, message: Message) {
        let mut state = self.lock();
        let seq = state.next_seq;
        state.next_seq = seq.saturating_add(1);
        state.pending.push(BufferedMessage { seq, message });
    }

    /// Queues a save of the unsaved messages to store `index` and starts it in the background;
    /// the returned future resolves when that save has finished. Skipped while the store backs
    /// off.
    pub(crate) fn process(
        &self,
        index: usize,
        invocation_context: Option<InvocationContext>,
    ) -> BoxFuture<'static, ()> {
        if !self.should_attempt(index) {
            return futures::future::ready(()).boxed();
        }
        // Captured now: this runs inside the live agent span (through the trigger hook), and the
        // save runs detached, after that span may have ended.
        let link = current_span_context();
        self.enqueue(index, link, invocation_context).boxed()
    }

    fn enqueue(
        &self,
        index: usize,
        link: Option<SpanLink>,
        invocation_context: Option<InvocationContext>,
    ) -> Chain {
        let mut state = self.lock();
        let previous = state.stores.get(index).and_then(|store| store.chain.clone());
        let this = self.clone();
        let next = async move {
            if let Some(previous) = previous {
                previous.await;
            }
            this.extract(index, link, invocation_context).await;
        }
        .boxed()
        .shared();
        if let Some(store) = state.stores.get_mut(index) {
            store.chain = Some(next.clone());
        }
        state.generation = state.generation.wrapping_add(1);
        drop(state);
        // Driven off the caller's task, so a lifecycle hook never waits on a save.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            drop(runtime.spawn(next.clone().in_current_span()));
        }
        next
    }

    fn should_attempt(&self, index: usize) -> bool {
        let mut state = self.lock();
        let Some(store) = state.stores.get_mut(index) else {
            return false;
        };
        if store.consecutive_failures < SAVE_FAILURES_BEFORE_BACKOFF {
            return true;
        }
        store.backoff_counter = store.backoff_counter.wrapping_add(1);
        store.backoff_counter.is_multiple_of(BACKOFF_PROBE_INTERVAL)
    }

    /// Saves everything unsaved to every store and waits for all saves, including ones that
    /// start meanwhile (TS `flush`).
    pub(crate) async fn flush(&self) {
        for index in 0..self.inner.bindings.len() {
            drop(self.enqueue(index, None, None));
        }
        loop {
            let (generation, in_flight) = {
                let state = self.lock();
                let chains: Vec<Chain> = state
                    .stores
                    .iter()
                    .filter_map(|store| store.chain.clone())
                    .collect();
                (state.generation, chains)
            };
            join_all(in_flight).await;
            if self.lock().generation == generation {
                return;
            }
        }
    }

    async fn extract(
        &self,
        index: usize,
        link: Option<SpanLink>,
        invocation_context: Option<InvocationContext>,
    ) {
        let Some(binding) = self.inner.bindings.get(index) else {
            return;
        };
        let (mark, fresh) = {
            let mut state = self.lock();
            let mark = state.stores.get(index).and_then(|store| store.mark);
            let fresh: Vec<BufferedMessage> = state
                .pending
                .iter()
                .filter(|buffered| mark.is_none_or(|mark| buffered.seq > mark))
                .cloned()
                .collect();
            let Some(highest) = fresh.last().map(|buffered| buffered.seq) else {
                return;
            };
            // Marked before the save, so a save queued behind this one won't pick the same
            // messages up; a failed save puts the mark back and they retry.
            if let Some(store) = state.stores.get_mut(index) {
                store.mark = Some(highest);
            }
            (mark, fresh)
        };
        let filtered = filter_messages(&fresh, &binding.config.filter);
        // A turn filtered down to nothing never reaches the backend, so it leaves the failure
        // streak alone.
        let tracer = &self.inner.tracer;
        let span = tracer.start_extract(ExtractSpanStart {
            store_name: binding.store.name(),
            message_count: filtered.len(),
            filtered_count: fresh.len().saturating_sub(filtered.len()),
            agent_span_context: link,
        });
        let outcome = if filtered.is_empty() {
            None
        } else {
            // A panic is a failed save: unwinding here would poison this store's `Shared` queue,
            // so every later save and `flush` would panic too.
            let write = AssertUnwindSafe(
                self.write(binding, &filtered, invocation_context)
                    .instrument(span.clone()),
            );
            Some(
                write
                    .catch_unwind()
                    .await
                    .unwrap_or_else(|_| Err(Error::other("memory store save panicked"))),
            )
        };
        match &outcome {
            None => tracer.end_extract(&span, Some(0), None),
            Some(Ok(entry_count)) => tracer.end_extract(&span, Some(*entry_count), None),
            Some(Err(error)) => tracer.end_extract(&span, None, Some(&error.to_string())),
        }
        drop(span);
        let mut state = self.lock();
        if let Some(store) = state.stores.get_mut(index) {
            match outcome {
                Some(Ok(_)) => {
                    store.consecutive_failures = 0;
                    store.backoff_counter = 0;
                }
                Some(Err(error)) => {
                    store.consecutive_failures = store.consecutive_failures.saturating_add(1);
                    store.mark = mark;
                    let failures = store.consecutive_failures;
                    let name = binding.store.name();
                    if failures >= SAVE_FAILURES_BEFORE_BACKOFF {
                        tracing::error!(
                            "store=<{name}>, failures=<{failures}>, reason=<{error}> | memory store save failing repeatedly"
                        );
                    } else {
                        tracing::warn!("store=<{name}>, reason=<{error}> | memory extraction failed");
                    }
                }
                None => {}
            }
        }
        // Drop what every store has saved.
        let saved = state.stores.iter().map(|store| store.mark).min().flatten();
        if let Some(saved) = saved {
            state.pending.retain(|buffered| buffered.seq > saved);
        }
    }

    async fn write(
        &self,
        binding: &ExtractionBinding,
        buffered: &[BufferedMessage],
        invocation_context: Option<InvocationContext>,
    ) -> Result<usize> {
        let messages: Vec<Message> = buffered.iter().map(|buffered| buffered.message.clone()).collect();
        let Some(extractor) = &binding.config.extractor else {
            let context = AddMessagesContext::new(buffered.iter().map(|buffered| buffered.seq).collect());
            binding.store.add_messages(&messages, &context).await?;
            return Ok(messages.len());
        };
        let context = ExtractorContext {
            default_model: Some(Arc::clone(&self.inner.default_model)),
            invocation_context: InvocationContext::auxiliary(invocation_context.as_ref()),
            tracer: Some(self.inner.tracer.clone()),
        };
        let entries = extractor.extract(&messages, &context).await?;
        let settled = join_all(
            entries
                .iter()
                .map(|entry| binding.store.add(&entry.content, entry.metadata.as_ref())),
        )
        .await;
        let failures = settled.iter().filter(|result| result.is_err()).count();
        if failures > 0 {
            return Err(Error::other(format!(
                "failed to write {failures} of {} extracted entries",
                entries.len()
            )));
        }
        Ok(entries.len())
    }
}

#[cfg(test)]
mod tests;
