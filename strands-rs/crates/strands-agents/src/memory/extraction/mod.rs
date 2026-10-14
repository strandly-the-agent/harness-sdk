//! Automatic extraction of durable facts from a transcript
//! (`strands-ts/src/memory/extraction/**`).

pub mod key_aware;
pub mod model_extractor;
pub mod resolve;

pub use key_aware::{
    create_key_aware_extractor, key_aware_system_prompt, DEFAULT_EXTRACTION_GUIDANCE, EXTRACTION_CONTRACT,
};
pub use model_extractor::{ModelExtractor, DEFAULT_SYSTEM_PROMPT};
pub use resolve::{resolve_extraction_config, ExtractionSetting, ResolvedExtractionConfig, StoreWriteCaps};
