//! Doubles shared by the memory tests: a scriptable store and the hook drivers.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::{AddMessagesContext, ExtractionConfig, MemoryEntry, MemoryManager, MemoryStore, SearchOptions};
use crate::error::{Error, Result};
use crate::hooks::{AfterInvocationEvent, MessageAddedEvent};
use crate::plugins::Plugin;
use crate::testing::MockMessageModel;
use crate::tools::Tool;
use crate::types::agent::{InvocationState, LocalAgent};
use crate::types::json::JsonObject;
use crate::types::messages::{ContentBlock, Message, Role, TextBlock, ToolResultBlock, ToolUseBlock};
use crate::types::tools::ToolResultStatus;

type AddHook = Box<dyn FnMut(&str) -> Result<()> + Send>;

/// The write methods a [`MockStore`] implements (TS stores omit `add` or `addMessages`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sink {
    None,
    Add,
    AddMessages,
    Both,
}

/// A store that records its calls (the TS `createMockStore` and `createExtractionStore`).
pub(crate) struct MockStore {
    pub(crate) name: String,
    pub(crate) writable: bool,
    pub(crate) description: Option<String>,
    pub(crate) max_search_results: Option<usize>,
    pub(crate) entries: Vec<MemoryEntry>,
    pub(crate) tools: Vec<Arc<dyn Tool>>,
    pub(crate) sink: Sink,
    pub(crate) extraction: Option<ExtractionConfig>,
    pub(crate) search_error: Option<String>,
    /// While set, `add` and `add_messages` fail with `write error`.
    pub(crate) failing: AtomicBool,
    /// The next this many writes fail with `backend down` (TS `mockRejectedValueOnce`).
    pub(crate) fail_times: AtomicUsize,
    /// The next this many writes panic.
    pub(crate) panic_times: AtomicUsize,
    /// When set, the first `add` waits until a later `add` starts.
    pub(crate) rendezvous: Option<Arc<tokio::sync::Notify>>,
    /// Runs before each `add`; an error fails that write.
    pub(crate) on_add: Mutex<Option<AddHook>>,
    /// When set, every write waits for a permit first.
    pub(crate) gate: Option<Arc<tokio::sync::Semaphore>>,
    pub(crate) search_calls: Mutex<Vec<(String, SearchOptions)>>,
    pub(crate) add_calls: Mutex<Vec<(String, Option<JsonObject>)>>,
    pub(crate) add_messages_calls: Mutex<Vec<(Vec<Message>, AddMessagesContext)>>,
}

impl MockStore {
    /// A read-only store.
    pub(crate) fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            writable: false,
            description: None,
            max_search_results: None,
            entries: Vec::new(),
            tools: Vec::new(),
            sink: Sink::None,
            extraction: None,
            search_error: None,
            failing: AtomicBool::new(false),
            fail_times: AtomicUsize::new(0),
            panic_times: AtomicUsize::new(0),
            rendezvous: None,
            on_add: Mutex::new(None),
            gate: None,
            search_calls: Mutex::default(),
            add_calls: Mutex::default(),
            add_messages_calls: Mutex::default(),
        }
    }

    /// A writable store with an `add` method.
    pub(crate) fn writable(name: &str) -> Self {
        Self {
            writable: true,
            sink: Sink::Add,
            ..Self::new(name)
        }
    }

    /// A writable store with `extraction` and the `sink` write methods.
    pub(crate) fn extracting(name: &str, extraction: Option<ExtractionConfig>, sink: Sink) -> Self {
        Self {
            writable: true,
            sink,
            extraction,
            ..Self::new(name)
        }
    }

    pub(crate) fn with_entries(mut self, contents: &[&str]) -> Self {
        self.entries = contents
            .iter()
            .map(|content| MemoryEntry::new(*content))
            .collect();
        self
    }

    pub(crate) fn with_description(mut self, description: &str) -> Self {
        self.description = Some(description.to_owned());
        self
    }

    pub(crate) fn set_failing(&self, failing: bool) {
        self.failing.store(failing, Ordering::SeqCst);
    }

    pub(crate) fn fail_next(&self, times: usize) {
        self.fail_times.store(times, Ordering::SeqCst);
    }

    /// Forgets the recorded writes (TS `mockClear`).
    pub(crate) fn clear_calls(&self) {
        self.add_calls.lock().unwrap().clear();
        self.add_messages_calls.lock().unwrap().clear();
    }

    fn write_outcome(&self) -> Result<()> {
        let panics = self.panic_times.load(Ordering::SeqCst);
        if panics > 0 {
            self.panic_times.store(panics - 1, Ordering::SeqCst);
            std::panic::panic_any("store panicked");
        }
        let left = self.fail_times.load(Ordering::SeqCst);
        let fail_once = left > 0;
        if fail_once {
            self.fail_times.store(left - 1, Ordering::SeqCst);
        }
        if fail_once {
            return Err(Error::other("backend down"));
        }
        if self.failing.load(Ordering::SeqCst) {
            return Err(Error::other("write error"));
        }
        Ok(())
    }

    pub(crate) fn search_count(&self) -> usize {
        self.search_calls.lock().unwrap().len()
    }

    pub(crate) fn added(&self) -> Vec<String> {
        self.add_calls
            .lock()
            .unwrap()
            .iter()
            .map(|(content, _)| content.clone())
            .collect()
    }

    pub(crate) fn add_messages_count(&self) -> usize {
        self.add_messages_calls.lock().unwrap().len()
    }

    /// The text of every message in each `add_messages` batch.
    pub(crate) fn batches(&self) -> Vec<Vec<String>> {
        self.add_messages_calls
            .lock()
            .unwrap()
            .iter()
            .map(|(messages, _)| messages.iter().map(text_of).collect())
            .collect()
    }

    /// The sequence numbers of each `add_messages` batch.
    pub(crate) fn sequence_numbers(&self) -> Vec<Option<Vec<u64>>> {
        self.add_messages_calls
            .lock()
            .unwrap()
            .iter()
            .map(|(_, context)| context.sequence_numbers.clone())
            .collect()
    }

    async fn pass_gate(&self) {
        if let Some(gate) = &self.gate {
            gate.acquire().await.unwrap().forget();
        }
    }
}

#[async_trait]
impl MemoryStore for MockStore {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    fn max_search_results(&self) -> Option<usize> {
        self.max_search_results
    }

    fn writable(&self) -> bool {
        self.writable
    }

    fn extraction(&self) -> Option<ExtractionConfig> {
        self.extraction.clone()
    }

    async fn search(&self, query: &str, options: SearchOptions) -> Result<Vec<MemoryEntry>> {
        self.search_calls
            .lock()
            .unwrap()
            .push((query.to_owned(), options));
        match &self.search_error {
            Some(error) => Err(Error::other(error.clone())),
            None => Ok(self.entries.clone()),
        }
    }

    fn supports_add(&self) -> bool {
        matches!(self.sink, Sink::Add | Sink::Both)
    }

    async fn add(&self, content: &str, metadata: Option<&JsonObject>) -> Result<()> {
        let earlier = {
            let mut calls = self.add_calls.lock().unwrap();
            calls.push((content.to_owned(), metadata.cloned()));
            calls.len() - 1
        };
        if let Some(rendezvous) = &self.rendezvous {
            if earlier == 0 {
                rendezvous.notified().await;
            } else {
                rendezvous.notify_one();
            }
        }
        self.pass_gate().await;
        if let Some(hook) = self.on_add.lock().unwrap().as_mut() {
            hook(content)?;
        }
        self.write_outcome()
    }

    fn supports_add_messages(&self) -> bool {
        matches!(self.sink, Sink::AddMessages | Sink::Both)
    }

    async fn add_messages(&self, messages: &[Message], context: &AddMessagesContext) -> Result<()> {
        self.add_messages_calls
            .lock()
            .unwrap()
            .push((messages.to_vec(), context.clone()));
        self.pass_gate().await;
        self.write_outcome()
    }

    fn get_tools(&self) -> Vec<Arc<dyn Tool>> {
        self.tools.clone()
    }
}

/// The text blocks of `message`, joined.
pub(crate) fn text_of(message: &Message) -> String {
    message
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

pub(crate) fn user(text: &str) -> Message {
    Message::new(Role::User, vec![TextBlock::new(text).into()])
}

pub(crate) fn assistant(text: &str) -> Message {
    Message::new(Role::Assistant, vec![TextBlock::new(text).into()])
}

pub(crate) fn tool_use() -> Message {
    Message::new(
        Role::Assistant,
        vec![ToolUseBlock::new("x", "t1", serde_json::json!({})).into()],
    )
}

pub(crate) fn tool_result() -> Message {
    Message::new(
        Role::User,
        vec![ToolResultBlock::new("t1", ToolResultStatus::Success, vec!["done".into()]).into()],
    )
}

/// An agent over a mock model (the TS `createMockAgent`).
pub(crate) fn mock_agent() -> LocalAgent {
    LocalAgent::new(Arc::new(MockMessageModel::new()))
}

/// Attaches `manager` to `agent`, as the plugin registry does.
pub(crate) async fn init_agent(manager: &MemoryManager, agent: &mut LocalAgent) {
    Arc::new(manager.clone()).init_agent(agent).await.unwrap();
}

/// Fires `MessageAddedEvent` for each message.
pub(crate) async fn add_messages(agent: &mut LocalAgent, messages: Vec<Message>) {
    let hooks = agent.hooks().clone();
    for message in messages {
        let mut event = MessageAddedEvent::new(agent, message, InvocationState::new());
        hooks
            .invoke::<MessageAddedEvent<'static>>(&mut event)
            .await
            .unwrap();
    }
}

/// Fires `AfterInvocationEvent` without waiting for the extraction it starts.
pub(crate) async fn fire_hooks(agent: &mut LocalAgent) {
    let hooks = agent.hooks().clone();
    let mut event = AfterInvocationEvent::new(agent, InvocationState::new());
    hooks
        .invoke::<AfterInvocationEvent<'static>>(&mut event)
        .await
        .unwrap();
}

/// Fires `AfterInvocationEvent`, then flushes so the background extraction has finished.
pub(crate) async fn fire_invocation(agent: &mut LocalAgent, manager: &MemoryManager) {
    fire_hooks(agent).await;
    manager.flush().await;
}
