//! Wire types: messages, content blocks, media, citations, tools and snapshots (TS `types/`).
//!
//! Serde matches the TS JSON exactly: camelCase keys, unset optional fields omitted, single-key
//! block wrappers and the TS key order.

pub mod citations;
pub mod json;
pub mod media;
pub mod messages;
pub mod snapshot;
pub mod tools;
pub(crate) mod validation;
