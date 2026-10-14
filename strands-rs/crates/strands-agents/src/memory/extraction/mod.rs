//! Automatic extraction of durable facts from a transcript
//! (`strands-ts/src/memory/extraction/**`).

pub mod model_extractor;

pub use model_extractor::{ModelExtractor, DEFAULT_SYSTEM_PROMPT};
