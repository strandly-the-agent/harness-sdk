//! Formatting helpers shared by both OpenAI adapters (TS `models/openai/formatting.ts`).

use crate::mime::to_mime_type;
use crate::types::json::JsonObject;
use crate::types::media::{DocumentBlock, DocumentSource, ImageBlock, ImageSource, encode_base64};
use crate::types::messages::{ContentBlock, ToolResultContent};

/// Warns for each key of `params` that the provider manages and overwrites (TS `warnManagedParams`).
pub(crate) fn warn_managed_params(params: Option<&JsonObject>, managed: &[&str]) {
    let Some(params) = params else {
        return;
    };
    for key in params.keys().filter(|key| managed.contains(&key.as_str())) {
        tracing::warn!(
            "params_key=<{key}> | '{key}' is managed by the provider and will be ignored in params — use the dedicated config property instead"
        );
    }
}

/// A `data:<mime>;base64,<payload>` URL for image bytes, the URL of a URL image, else `None`
/// (TS `formatImageDataUrl`).
pub(crate) fn format_image_data_url(image: &ImageBlock) -> Option<String> {
    match image.source() {
        ImageSource::Bytes(bytes) => {
            let format = image.format().as_str();
            let mime = to_mime_type(format).map_or_else(|| format!("image/{format}"), str::to_owned);
            Some(format!("data:{mime};base64,{}", encode_base64(bytes)))
        }
        ImageSource::Url(url) => Some(url.clone()),
        _ => None,
    }
}

/// A `data:<mime>;base64,<payload>` URL for document bytes.
pub(crate) fn format_document_data_url(document: &DocumentBlock, bytes: &[u8]) -> String {
    let format = document.format().as_str();
    let mime = to_mime_type(format).map_or_else(|| format!("application/{format}"), str::to_owned);
    format!("data:{mime};base64,{}", encode_base64(bytes))
}

/// The TS `source.type` of a document source.
pub(crate) fn document_source_type(source: &DocumentSource) -> &'static str {
    match source {
        DocumentSource::Bytes(_) => "documentSourceBytes",
        DocumentSource::Text(_) => "documentSourceText",
        DocumentSource::Content(_) => "documentSourceContentBlock",
        _ => "documentSourceS3Location",
    }
}

/// The TS `type` of a content block.
pub(crate) fn block_type(block: &ContentBlock) -> &'static str {
    match block {
        ContentBlock::Text(_) => "textBlock",
        ContentBlock::ToolUse(_) => "toolUseBlock",
        ContentBlock::ToolResult(_) => "toolResultBlock",
        ContentBlock::Reasoning(_) => "reasoningBlock",
        ContentBlock::CachePoint(_) => "cachePointBlock",
        ContentBlock::GuardContent(_) => "guardContentBlock",
        ContentBlock::Audio(_) => "audioBlock",
        ContentBlock::Image(_) => "imageBlock",
        ContentBlock::Video(_) => "videoBlock",
        ContentBlock::Document(_) => "documentBlock",
        ContentBlock::Citations(_) => "citationsBlock",
        ContentBlock::Injected(_) => "injectedBlock",
        ContentBlock::Summary(_) => "summaryBlock",
        _ => "unknownBlock",
    }
}

/// The TS `type` of a tool result content block.
pub(crate) fn tool_result_content_type(content: &ToolResultContent) -> &'static str {
    match content {
        ToolResultContent::Text(_) => "textBlock",
        ToolResultContent::Json(_) => "jsonBlock",
        ToolResultContent::Image(_) => "imageBlock",
        ToolResultContent::Video(_) => "videoBlock",
        ToolResultContent::Document(_) => "documentBlock",
        _ => "unknownBlock",
    }
}
