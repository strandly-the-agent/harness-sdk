//! The harness's warnings are `tracing` events: an application sees them once it installs a
//! subscriber, which is what replaces `configureLogging` of the TypeScript harness
//! (harness-ts/src/logging.ts). A test binary of its own, because the subscriber is the
//! process's global one.
#![allow(clippy::unwrap_used, reason = "test code")]

use std::fmt::Debug;
use std::sync::{Arc, Mutex};

use strands::models::Model;
use strands::testing::MockMessageModel;
use strands_harness::{Effort, HarnessAgentOptions, create_harness};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

/// Keeps the level and message of every event.
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<(Level, String)>>>);

struct Message(Option<String>);

impl Visit for Message {
    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        if field.name() == "message" {
            self.0 = Some(format!("{value:?}"));
        }
    }
}

impl Subscriber for Recorder {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _: &Id, _: &Record<'_>) {}

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut message = Message(None);
        event.record(&mut message);
        if let Some(text) = message.0 {
            self.0.lock().unwrap().push((*event.metadata().level(), text));
        }
    }

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}

#[tokio::test]
async fn a_harness_warning_reaches_the_subscriber_the_application_installed() {
    let recorder = Recorder::default();
    tracing::subscriber::set_global_default(recorder.clone()).unwrap();

    // `effort` cannot be applied to a model instance: the harness warns and passes it through.
    let model: Arc<dyn Model> = Arc::new(MockMessageModel::new());
    let options = HarnessAgentOptions::builder()
        .model(model)
        .effort(Effort::High)
        .session(false)
        .skills(false)
        .memory(false)
        .build();
    create_harness(options).await.unwrap();

    let events = recorder.0.lock().unwrap();
    let warning = "effort \"high\" not applied to a pre-built Model instance or ModelRouter; configure reasoning on the instance";
    assert!(
        events
            .iter()
            .any(|(level, text)| *level == Level::WARN && text == warning),
        "{events:?}"
    );
}
