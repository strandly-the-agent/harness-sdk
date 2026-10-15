//! temp
mod adapters;
mod events;
pub mod types;
pub use adapters::{content_blocks_to_parts, parts_to_content_blocks};
pub use events::{A2AResultEvent, A2AStreamEvent, A2AStreamUpdateEvent};
