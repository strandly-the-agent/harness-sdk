//! Text truncation for offloading (TS `context-manager/methods/truncate.ts`).

use crate::conversation_manager::token_usage_middleware::grouped;
use crate::types::messages::{TextBlock, ToolResultBlock, ToolResultContent};

/// The prefix of every truncation header (TS `TRUNCATED_PREFIX`).
pub(crate) const TRUNCATED_PREFIX: &str = "[Truncated:";
pub(crate) const DEFAULT_PREVIEW_TOKENS: u64 = 1000;
const CHARS_PER_TOKEN: u64 = 4;

/// Which part of the text a truncation keeps (TS `TruncateConfig.preview`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum PreviewMode {
    /// The start (TS `'head'`).
    Head,
    /// The end (TS `'tail'`).
    Tail,
    /// 60% start, 40% end (TS `'headTail'`, the default).
    #[default]
    HeadTail,
}

/// How a truncation previews the original (TS `TruncateConfig`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, Eq, bon::Builder)]
#[builder(state_mod(vis = "pub"), derive(Debug))]
pub struct TruncateConfig {
    /// Tokens of preview text to keep (default 1,000).
    pub preview_tokens: Option<u64>,
    /// Which part to keep (default [`PreviewMode::HeadTail`]).
    pub preview: Option<PreviewMode>,
}

impl TruncateConfig {
    pub(crate) fn preview_tokens(&self) -> u64 {
        self.preview_tokens.unwrap_or(DEFAULT_PREVIEW_TOKENS)
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "TS `Math.floor(previewChars * 0.6)` on a non-negative char count"
)]
fn head_share(preview_chars: usize) -> usize {
    (preview_chars as f64 * 0.6).floor() as usize
}

pub(crate) fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// The truncated form of `full_text`, or `full_text` itself when truncating wouldn't shrink it
/// (TS `buildPreview`).
pub(crate) fn build_preview(full_text: &str, block_count: usize, config: &TruncateConfig) -> String {
    let preview_chars =
        usize::try_from(config.preview_tokens().saturating_mul(CHARS_PER_TOKEN)).unwrap_or(usize::MAX);
    let total_chars = full_text.chars().count();
    if total_chars <= preview_chars {
        return full_text.to_owned();
    }
    let head_chars = match config.preview.unwrap_or_default() {
        PreviewMode::Head => preview_chars,
        PreviewMode::Tail => 0,
        PreviewMode::HeadTail => head_share(preview_chars),
    };
    let tail_chars = preview_chars - head_chars;
    let head: String = full_text.chars().take(head_chars).collect();
    let tail: String = full_text.chars().skip(total_chars - tail_chars).collect();
    let elided = total_chars - head_chars - tail_chars;
    let marker = format!("[... {} chars elided ...]", grouped(elided as u64));
    let preview = [head.as_str(), marker.as_str(), tail.as_str()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let tokens = (total_chars as u64).div_ceil(CHARS_PER_TOKEN);
    let result = format!(
        "{TRUNCATED_PREFIX} {}, ~{} tokens]\n\n{preview}",
        plural(block_count, "block", "blocks"),
        grouped(tokens)
    );
    if result.chars().count() >= total_chars {
        return full_text.to_owned();
    }
    result
}

/// The truncated tool result, or `None` when nothing shrinks (TS `truncateToolResultBlock`).
pub(crate) fn truncate_tool_result_block(
    block: &ToolResultBlock,
    config: &TruncateConfig,
) -> Option<ToolResultBlock> {
    let mut textual = Vec::new();
    let mut opaque = Vec::new();
    for content in block.content() {
        match content {
            ToolResultContent::Text(text) => textual.push(text.text().to_owned()),
            ToolResultContent::Json(_) => textual.push(serde_json::to_string(content).ok()?),
            other => opaque.push(other.clone()),
        }
    }
    if textual.is_empty() {
        return None;
    }
    let full_text = textual.join("\n");
    let preview = build_preview(&full_text, textual.len(), config);
    if preview == full_text {
        return None;
    }
    let content = std::iter::once(TextBlock::new(preview).into())
        .chain(opaque)
        .collect();
    Some(ToolResultBlock::new(
        block.tool_use_id(),
        block.status().clone(),
        content,
    ))
}

/// The truncated text block, or `None` when nothing shrinks (TS `truncateTextBlock`).
pub(crate) fn truncate_text_block(block: &TextBlock, config: &TruncateConfig) -> Option<TextBlock> {
    let preview = build_preview(block.text(), 1, config);
    (preview != block.text()).then(|| TextBlock::new(preview))
}
