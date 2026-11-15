//! Test doubles for code built on Strands (TS `src/__fixtures__/`).

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use futures::stream;

use crate::error::{Error, Result};
use crate::models::model::{BaseModelConfig, BoxStream, Model, ModelRequest};
use crate::models::streaming::ModelStreamEvent;

type EventGenerator = Arc<dyn Fn() -> BoxStream<'static, Result<ModelStreamEvent>> + Send + Sync>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A model that streams whatever its event generator produces (TS `TestModelProvider` in
/// `__fixtures__/model-test-helpers.ts`). Its config starts with the model id `test-model`.
///
/// ```
/// use futures::StreamExt;
/// use strands::models::model::{AggregatedEvent, Model, ModelRequest};
/// use strands::models::streaming::{
///     ModelContentBlockDeltaEvent, ModelContentBlockStartEvent, ModelContentBlockStopEvent,
///     ModelMessageStartEvent, ModelMessageStopEvent, TextDelta,
/// };
/// use strands::testing::TestModelProvider;
/// use strands::types::messages::Role;
/// use strands::StopReason;
///
/// # futures::executor::block_on(async {
/// let model = TestModelProvider::from_events(vec![
///     ModelMessageStartEvent::new(Role::Assistant).into(),
///     ModelContentBlockStartEvent::new().into(),
///     ModelContentBlockDeltaEvent::new(TextDelta::new("hi")).into(),
///     ModelContentBlockStopEvent::new().into(),
///     ModelMessageStopEvent::new(StopReason::EndTurn).into(),
/// ]);
/// let mut stream = model.stream_aggregated(ModelRequest::default());
/// let mut done = None;
/// while let Some(item) = stream.next().await {
///     if let AggregatedEvent::Done(result) = item? {
///         done = Some(result);
///     }
/// }
/// assert_eq!(done.map(|result| result.message().content().len()), Some(1));
/// # Ok::<(), strands::Error>(())
/// # })?;
/// # Ok::<(), strands::Error>(())
/// ```
pub struct TestModelProvider {
    generator: Mutex<Option<EventGenerator>>,
    config: Mutex<BaseModelConfig>,
}

impl TestModelProvider {
    /// A provider without an event generator; streaming fails until one is set.
    pub fn new() -> Self {
        Self {
            generator: Mutex::new(None),
            config: Mutex::new(BaseModelConfig {
                model_id: Some("test-model".to_owned()),
                ..Default::default()
            }),
        }
    }

    /// A provider whose every call streams what `generator` returns.
    pub fn with_event_generator(
        generator: impl Fn() -> BoxStream<'static, Result<ModelStreamEvent>> + Send + Sync + 'static,
    ) -> Self {
        let provider = Self::new();
        provider.set_event_generator(generator);
        provider
    }

    /// A provider whose every call streams `events`.
    pub fn from_events(events: Vec<ModelStreamEvent>) -> Self {
        Self::with_event_generator(move || Box::pin(stream::iter(events.clone().into_iter().map(Ok))))
    }

    /// Replaces the event generator (TS `setEventGenerator`).
    pub fn set_event_generator(
        &self,
        generator: impl Fn() -> BoxStream<'static, Result<ModelStreamEvent>> + Send + Sync + 'static,
    ) {
        *lock(&self.generator) = Some(Arc::new(generator));
    }
}

impl Default for TestModelProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for TestModelProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TestModelProvider")
            .field("config", &*lock(&self.config))
            .finish_non_exhaustive()
    }
}

impl Model for TestModelProvider {
    fn stream(&self, _request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>> {
        match lock(&self.generator).clone() {
            Some(generator) => generator(),
            None => Box::pin(stream::once(async {
                Err(Error::other("Event generator not set"))
            })),
        }
    }

    fn get_config(&self) -> BaseModelConfig {
        lock(&self.config).clone()
    }

    fn update_config(&self, config: BaseModelConfig) {
        lock(&self.config).merge(config);
    }
}
