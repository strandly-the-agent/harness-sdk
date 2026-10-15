//! Agent-to-Agent (A2A) protocol support: in-tree protocol types, adapters between the A2A
//! wire format and Strands content, and the hook events emitted while an agent serves a task.
//!
//! Mirrors `strands-ts/src/a2a/`. Behind the `a2a` cargo feature.

mod adapters;
mod events;
pub mod types;
pub use adapters::{content_blocks_to_parts, parts_to_content_blocks};
pub use events::{A2AResultEvent, A2AStreamEvent, A2AStreamUpdateEvent};
