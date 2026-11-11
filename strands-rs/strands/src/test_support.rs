//! Helpers for the crate's own unit tests.

use std::sync::{Arc, Mutex};

use futures::StreamExt;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

use crate::models::{
    AggregatedEvent, BaseModelConfig, BoxStream, Model, ModelRequest, ModelStreamEvent,
    StreamAggregatedResult,
};
use crate::types::{ContentBlock, JsonValue, Message, Role, TextBlock};

/// Runs `f` and returns the `message` of every event it logged, prefixed with the level.
pub(crate) fn capture_logs<R>(f: impl FnOnce() -> R) -> (R, Vec<String>) {
    let logs = Arc::new(Mutex::new(Vec::new()));
    let result = tracing::subscriber::with_default(Capture(Arc::clone(&logs)), f);
    let logs = logs.lock().unwrap().clone();
    (result, logs)
}

struct Capture(Arc<Mutex<Vec<String>>>);

struct MessageVisitor(String);

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

impl Subscriber for Capture {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _: &Id, _: &Record<'_>) {}

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        self.0
            .lock()
            .unwrap()
            .push(format!("{} {}", event.metadata().level(), visitor.0));
    }

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}

/// A model that replays a scripted event list on every call and records each request (TS
/// `TestModelProvider`).
pub(crate) struct ScriptedModel {
    script: Box<dyn Fn() -> Vec<crate::Result<ModelStreamEvent>> + Send + Sync>,
    config: Mutex<BaseModelConfig>,
    pub(crate) requests: Mutex<Vec<ModelRequest>>,
}

impl ScriptedModel {
    /// Replays `events`, each the TS JSON of a model stream event.
    pub(crate) fn new(events: Vec<JsonValue>) -> Self {
        let events: Vec<ModelStreamEvent> = events
            .into_iter()
            .map(|event| serde_json::from_value(event).unwrap())
            .collect();
        Self::from_fn(move || events.iter().cloned().map(Ok).collect())
    }

    /// Replays what `script` returns on each call.
    pub(crate) fn from_fn(
        script: impl Fn() -> Vec<crate::Result<ModelStreamEvent>> + Send + Sync + 'static,
    ) -> Self {
        Self {
            script: Box::new(script),
            config: Mutex::new(BaseModelConfig {
                model_id: Some("test-model".into()),
                ..Default::default()
            }),
            requests: Mutex::new(Vec::new()),
        }
    }
}

impl Model for ScriptedModel {
    fn stream(&self, request: ModelRequest) -> BoxStream<'static, crate::Result<ModelStreamEvent>> {
        self.requests.lock().unwrap().push(request);
        Box::pin(futures::stream::iter((self.script)()))
    }

    fn get_config(&self) -> BaseModelConfig {
        self.config.lock().unwrap().clone()
    }

    fn update_config(&self, config: BaseModelConfig) {
        self.config.lock().unwrap().merge(config);
    }
}

/// Drains `stream`, returning the items before the result as JSON and the result (TS
/// `collectGenerator`).
pub(crate) async fn collect_aggregated(
    mut stream: BoxStream<'static, crate::Result<AggregatedEvent>>,
) -> (Vec<JsonValue>, crate::Result<StreamAggregatedResult>) {
    let mut items = Vec::new();
    while let Some(item) = stream.next().await {
        match item {
            Ok(AggregatedEvent::ModelStreamEvent(event)) => items.push(serde_json::to_value(event).unwrap()),
            Ok(AggregatedEvent::ContentBlock(block)) => items.push(serde_json::to_value(block).unwrap()),
            Ok(AggregatedEvent::Done(result)) => return (items, Ok(*result)),
            Err(error) => return (items, Err(error)),
        }
    }
    panic!("stream ended without a result");
}

/// A user message with one text block.
pub(crate) fn user_text(text: &str) -> Message {
    Message::new(Role::User, vec![ContentBlock::Text(TextBlock::new(text))])
}

/// A request with one user text message.
pub(crate) fn hi_request() -> ModelRequest {
    ModelRequest {
        messages: vec![user_text("Hi")],
        ..Default::default()
    }
}
