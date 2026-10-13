//! Strands Agents SDK for Rust.
//!
//! A model-driven framework for building AI agents: a [`Model`](models::Model), a system prompt,
//! and a list of [`Tool`](tools::Tool)s composed into an agent loop.

pub mod agent;
pub mod error;
pub mod hooks;
pub mod logging;
pub mod models;
pub mod plugins;
pub mod session;
pub mod state_store;
pub mod tools;
pub mod types;

pub use agent::{Agent, AgentResult, InvocationState, InvokeInput};
pub use error::{Error, Result};
pub use models::Model;
pub use plugins::Plugin;
pub use session::{FileStorage, SessionManager, Snapshot, SnapshotStorage};
pub use tools::{tool, FunctionTool, Tool, ToolContext};
pub use types::{ContentBlock, Message, Role, StopReason, SystemPrompt};
