//! Agent-to-Agent (A2A) protocol support: in-tree protocol types, adapters between the A2A
//! wire format and Strands content, and the hook events emitted while an agent serves a task.
//!
//! Mirrors `strands-ts/src/a2a/`. Behind the `a2a` cargo feature.

mod a2a_agent;
mod adapters;
mod events;
mod executor;
mod server;
mod task_store;
pub mod types;
pub use a2a_agent::{A2AAgent, A2AAgentBuilder, DEFAULT_AGENT_CARD_PATH};
pub use adapters::{content_blocks_to_parts, parts_to_content_blocks};
pub use events::{A2AResultEvent, A2AStreamEvent, A2AStreamUpdateEvent};
pub use executor::{
    A2AExecutor, AgentFactory, EventBus, RequestContext, A2A_REQUEST_CONTEXT_KEY, DEFAULT_MAX_CONTEXTS,
};
pub use server::{A2AServer, A2AServerBuilder, ServerHandle};
pub use task_store::{TaskStore, DEFAULT_MAX_TASKS};
