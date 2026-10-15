//! Serving a local [`Agent`] over the A2A protocol, one execution per A2A task
//! (`a2a/executor.ts`).

use std::sync::Arc;

use futures::StreamExt;
use parking_lot::Mutex as SyncMutex;
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

use super::adapters::{content_blocks_to_parts, parts_to_content_blocks};
use super::types::{
    A2AEventData, Artifact, JsonRpcError, Message, Part, Task, TaskArtifactUpdateEvent, TaskState,
    TaskStatus, TaskStatusUpdateEvent,
};
use crate::agent::{
    Agent, AgentStreamEvent, InvocationState, InvokeOptions, SnapshotPreset, TakeSnapshotOptions,
};
use crate::error::{Error, Result};
use crate::types::ContentBlock;

/// Invocation-state key under which the executor exposes the [`RequestContext`] to the agent.
pub const A2A_REQUEST_CONTEXT_KEY: &str = "a2aRequestContext";

/// Default cap on concurrently tracked A2A contexts.
pub const DEFAULT_MAX_CONTEXTS: usize = 1000;

/// Builds a fresh agent for an A2A `contextId`; called once per context.
pub type AgentFactory = Arc<dyn Fn(&str) -> Agent + Send + Sync>;

/// One A2A request: the task and conversation it belongs to, plus the user's message.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct RequestContext {
    /// Id of the task this request drives.
    pub task_id: String,
    /// Id of the conversation the task belongs to.
    pub context_id: String,
    /// The message the caller sent.
    pub user_message: Message,
    /// The task as the caller knows it, when continuing an existing one.
    pub task: Option<Task>,
}

impl RequestContext {
    /// A context for `user_message` within an existing task and conversation.
    pub fn new(task_id: impl Into<String>, context_id: impl Into<String>, user_message: Message) -> Self {
        Self {
            task_id: task_id.into(),
            context_id: context_id.into(),
            user_message,
            task: None,
        }
    }

    /// A context for `user_message` under freshly generated task and context ids.
    pub fn for_message(user_message: Message) -> Self {
        let task_id = user_message
            .task_id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let context_id = user_message
            .context_id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        Self::new(task_id, context_id, user_message)
    }
}

/// The sink an [`A2AExecutor`] publishes protocol events to.
///
/// Mirrors the `ExecutionEventBus` of `@a2a-js/sdk/server`: a server drains the receiver to
/// aggregate a task (`message/send`) or to forward events as SSE (`message/stream`).
#[derive(Clone, Debug)]
pub struct EventBus {
    tx: mpsc::UnboundedSender<A2AEventData>,
}

impl EventBus {
    /// A bus and the receiver of everything published to it.
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<A2AEventData>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }

    /// Publish one protocol event; dropped once the receiver is gone.
    pub fn publish(&self, event: A2AEventData) {
        let _ = self.tx.send(event);
    }
}

/// A context's dedicated agent; holding the lock both serializes its requests and grants access.
type ContextAgent = Arc<Mutex<Agent>>;

enum Mode {
    Factory {
        factory: AgentFactory,
        contexts: SyncMutex<Vec<(String, ContextAgent)>>,
    },
    Shared {
        agent: Agent,
        lock: Mutex<()>,
        template: crate::session::Snapshot,
        snapshots: SyncMutex<Vec<(String, crate::session::Snapshot)>>,
    },
}

/// Bridges a Strands [`Agent`] into the A2A protocol.
///
/// Converts the request's parts to content blocks, streams the agent, and publishes its text
/// deltas as artifact updates plus a terminal status update. Conversation state is isolated per
/// A2A `contextId`, in one of two modes:
///
/// - [`A2AExecutor::with_factory`] (recommended): a dedicated agent per context, so contexts run
///   concurrently and never share state — and per-context concerns such as a session manager
///   belong in the factory.
/// - [`A2AExecutor::new`]: one shared agent whose session state is swapped per context under a
///   lock, so all requests are serialized.
pub struct A2AExecutor {
    mode: Mode,
    max_contexts: usize,
}

impl A2AExecutor {
    /// An executor over one shared `agent`, swapping each context's state on and off it.
    ///
    /// All requests are serialized. Fails when the agent has a session manager, which would
    /// persist every context's messages into one interleaved session; use
    /// [`A2AExecutor::with_factory`] for a per-context agent with its own session manager.
    pub fn new(agent: Agent) -> Result<Self> {
        if agent
            .plugins()
            .get(crate::session::session_manager::PLUGIN_NAME)
            .is_some()
        {
            return Err(Error::other(
                "A single agent with a session manager is not supported: the session manager \
                 persists every context's messages into one interleaved session. Use \
                 A2AExecutor::with_factory to build a per-context agent with its own session manager.",
            ));
        }
        let template = capture_state(&agent);
        Ok(Self {
            mode: Mode::Shared {
                agent,
                lock: Mutex::new(()),
                template,
                snapshots: SyncMutex::new(Vec::new()),
            },
            max_contexts: DEFAULT_MAX_CONTEXTS,
        })
    }

    /// An executor that builds a dedicated agent per A2A `contextId`.
    pub fn with_factory(factory: impl Fn(&str) -> Agent + Send + Sync + 'static) -> Self {
        Self {
            mode: Mode::Factory {
                factory: Arc::new(factory),
                contexts: SyncMutex::new(Vec::new()),
            },
            max_contexts: DEFAULT_MAX_CONTEXTS,
        }
    }

    /// Cap on retained contexts; the least recently used is evicted beyond it. Zero means one.
    #[must_use]
    pub fn with_max_contexts(mut self, max_contexts: usize) -> Self {
        self.max_contexts = max_contexts.max(1);
        self
    }

    /// Execute the agent for one A2A request, publishing its events to `bus`.
    ///
    /// Publishes the task as `working`, then a text artifact streamed as chunks, any non-text
    /// content block as its own artifact, and finally a `completed` status update. An agent
    /// failure is returned after the initial task event, leaving the task non-final.
    pub async fn execute(&self, context: &RequestContext, bus: &EventBus) -> Result<()> {
        let blocks = parts_to_content_blocks(&context.user_message.parts);
        if blocks.is_empty() {
            return Err(json_rpc(JsonRpcError::invalid_request(
                "No content blocks available",
            )));
        }

        bus.publish(A2AEventData::Task(Task {
            id: context.task_id.clone(),
            context_id: context.context_id.clone(),
            status: TaskStatus::new(TaskState::Working),
            artifacts: None,
            history: None,
            metadata: None,
        }));

        match &self.mode {
            Mode::Factory { .. } => {
                let entry = self.context_agent(&context.context_id);
                let agent = entry.lock().await;
                self.stream_agent(&agent, context, blocks, bus).await
            }
            Mode::Shared {
                agent,
                lock,
                template,
                snapshots,
            } => {
                let _guard = lock.lock().await;
                let restored =
                    take_snapshot(snapshots, &context.context_id).unwrap_or_else(|| template.clone());
                agent.load_snapshot(&restored)?;
                let result = self.stream_agent(agent, context, blocks, bus).await;
                // Persist the updated history even on error, then reset for the next caller.
                put_recent(
                    snapshots,
                    context.context_id.clone(),
                    capture_state(agent),
                    self.max_contexts,
                );
                agent.load_snapshot(template)?;
                result
            }
        }
    }

    /// Cancel a running task: not supported by this executor.
    pub async fn cancel_task(&self, _task_id: &str, _bus: &EventBus) -> Result<()> {
        Err(json_rpc(JsonRpcError::unsupported_operation(
            "Task cancellation is not supported",
        )))
    }

    /// This context's agent and lock, built on first use and kept most-recently-used.
    fn context_agent(&self, context_id: &str) -> ContextAgent {
        let Mode::Factory { factory, contexts } = &self.mode else {
            unreachable!("factory mode only")
        };
        let mut contexts = contexts.lock();
        if let Some(index) = contexts.iter().position(|(id, _)| id == context_id) {
            let entry = contexts.remove(index);
            contexts.push(entry.clone());
            return entry.1;
        }
        let agent = Arc::new(Mutex::new(factory(context_id)));
        contexts.push((context_id.to_owned(), Arc::clone(&agent)));
        while contexts.len() > self.max_contexts {
            contexts.remove(0);
        }
        agent
    }

    /// Stream one invocation and translate its events into A2A artifact and status events.
    async fn stream_agent(
        &self,
        agent: &Agent,
        context: &RequestContext,
        blocks: Vec<ContentBlock>,
        bus: &EventBus,
    ) -> Result<()> {
        let task_id = &context.task_id;
        let context_id = &context.context_id;
        let artifact_id = Uuid::new_v4().to_string();
        let mut first_chunk = true;
        let mut last_delta = String::new();

        let state = InvocationState::new();
        state.insert(A2A_REQUEST_CONTEXT_KEY, context.clone());
        let mut stream = agent.stream_with(blocks, InvokeOptions::default().with_invocation_state(state));

        while let Some(event) = stream.next().await {
            let event = event?;
            if let Some(delta) = event.text_delta() {
                bus.publish(A2AEventData::ArtifactUpdate(TaskArtifactUpdateEvent {
                    task_id: task_id.clone(),
                    context_id: context_id.clone(),
                    artifact: Artifact::new(artifact_id.clone(), vec![Part::text(delta)]),
                    append: Some(!first_chunk),
                    last_chunk: None,
                    metadata: None,
                }));
                first_chunk = false;
                last_delta = delta.to_owned();
            }
            if let AgentStreamEvent::ContentBlock(event) = &event {
                if !matches!(event.content_block, ContentBlock::Text { .. }) {
                    let parts = content_blocks_to_parts(std::slice::from_ref(&event.content_block));
                    if !parts.is_empty() {
                        bus.publish(A2AEventData::ArtifactUpdate(TaskArtifactUpdateEvent {
                            task_id: task_id.clone(),
                            context_id: context_id.clone(),
                            artifact: Artifact::new(Uuid::new_v4().to_string(), parts),
                            append: Some(false),
                            last_chunk: Some(true),
                            metadata: None,
                        }));
                    }
                }
            }
        }

        // Close the text artifact; if nothing was streamed, publish the whole text in this chunk.
        bus.publish(A2AEventData::ArtifactUpdate(TaskArtifactUpdateEvent {
            task_id: task_id.clone(),
            context_id: context_id.clone(),
            artifact: Artifact::new(
                artifact_id,
                vec![Part::text(if first_chunk { last_delta.as_str() } else { "" })],
            ),
            append: Some(!first_chunk),
            last_chunk: Some(true),
            metadata: None,
        }));
        bus.publish(A2AEventData::StatusUpdate(TaskStatusUpdateEvent {
            task_id: task_id.clone(),
            context_id: context_id.clone(),
            status: TaskStatus::new(TaskState::Completed),
            final_: true,
            metadata: None,
        }));
        Ok(())
    }
}

/// A JSON-RPC protocol error as a crate error, so a server can recover the code and message.
fn json_rpc(error: JsonRpcError) -> Error {
    Error::other(error)
}

fn capture_state(agent: &Agent) -> crate::session::Snapshot {
    agent.take_snapshot(TakeSnapshotOptions::default().preset(SnapshotPreset::Session))
}

fn take_snapshot(
    snapshots: &SyncMutex<Vec<(String, crate::session::Snapshot)>>,
    context_id: &str,
) -> Option<crate::session::Snapshot> {
    let mut snapshots = snapshots.lock();
    let index = snapshots.iter().position(|(id, _)| id == context_id)?;
    Some(snapshots.remove(index).1)
}

fn put_recent(
    snapshots: &SyncMutex<Vec<(String, crate::session::Snapshot)>>,
    context_id: String,
    snapshot: crate::session::Snapshot,
    max_contexts: usize,
) {
    let mut snapshots = snapshots.lock();
    if let Some(index) = snapshots.iter().position(|(id, _)| *id == context_id) {
        snapshots.remove(index);
    }
    snapshots.push((context_id, snapshot));
    while snapshots.len() > max_contexts {
        snapshots.remove(0);
    }
}
