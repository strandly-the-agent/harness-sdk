//! System prompt (`SystemPrompt` in `strands-ts/src/types/messages.ts`).

use serde::{Deserialize, Serialize};

use super::{CachePointBlock, GuardContentBlock};

/// A system prompt block: text, cache point, or guard content.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum SystemContentBlock {
    /// Plain text.
    Text(String),
    /// Prompt-cache checkpoint.
    CachePoint(CachePointBlock),
    /// Guardrail-evaluated content.
    GuardContent(GuardContentBlock),
}

/// System prompt: a plain string or a list of blocks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SystemPrompt {
    /// Plain text.
    Text(String),
    /// Structured blocks.
    Blocks(Vec<SystemContentBlock>),
}

impl SystemPrompt {
    /// Concatenated text of all text parts.
    pub fn text(&self) -> String {
        match self {
            SystemPrompt::Text(t) => t.clone(),
            SystemPrompt::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    SystemContentBlock::Text(t) => Some(t.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        }
    }

    /// Normalized block form (a string becomes a single text block).
    pub fn to_blocks(&self) -> Vec<SystemContentBlock> {
        match self {
            SystemPrompt::Text(t) => vec![SystemContentBlock::Text(t.clone())],
            SystemPrompt::Blocks(b) => b.clone(),
        }
    }
}

impl From<&str> for SystemPrompt {
    fn from(s: &str) -> Self {
        SystemPrompt::Text(s.to_owned())
    }
}

impl From<String> for SystemPrompt {
    fn from(s: String) -> Self {
        SystemPrompt::Text(s)
    }
}

impl From<Vec<SystemContentBlock>> for SystemPrompt {
    fn from(blocks: Vec<SystemContentBlock>) -> Self {
        SystemPrompt::Blocks(blocks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn string_and_blocks_round_trip() {
        let s: SystemPrompt = serde_json::from_value(json!("be nice")).unwrap();
        assert_eq!(s, SystemPrompt::Text("be nice".into()));
        let b: SystemPrompt =
            serde_json::from_value(json!([{"text": "a"}, {"cachePoint": {"cacheType": "default"}}])).unwrap();
        assert_eq!(b.text(), "a");
        assert_eq!(b.to_blocks().len(), 2);
        assert!(serde_json::from_value::<SystemPrompt>(json!([{"bogus": 1}])).is_err());
    }
}
