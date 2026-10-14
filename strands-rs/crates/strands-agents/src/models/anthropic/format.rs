//! Request formatting for the Anthropic Messages API (`_formatRequest` in TS).

use base64::Engine;
use serde_json::{json, Map, Value};

use super::{AnthropicCacheConfig, AnthropicCacheSection, AnthropicCacheStrategy, AnthropicModelConfig};
use crate::error::{Error, Result};
use crate::logging::warn_once;
use crate::models::model::{CountTokensOptions, StreamOptions};
use crate::types::{
    ContentBlock, DocumentFormat, DocumentSource, ImageFormat, ImageSource, JsonValue, Message, Role,
    SystemContentBlock, SystemPrompt, ToolChoice, ToolResultContent, ToolResultStatus, ToolSpec,
};

/// Document formats inlined as text rather than sent as a document block (`TEXT_FILE_FORMATS` in TS).
const TEXT_FILE_FORMATS: &[DocumentFormat] = &[
    DocumentFormat::Txt,
    DocumentFormat::Md,
    DocumentFormat::Csv,
    DocumentFormat::Json,
    DocumentFormat::Xml,
    DocumentFormat::Html,
];

/// Which part of the request a cache setting applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    Tools,
    SystemPrompt,
    Messages,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ResolvedSection {
    enabled: bool,
    ttl: Option<String>,
}

fn cache_section(config: &AnthropicModelConfig, section: Section) -> ResolvedSection {
    let Some(cache) = config.cache_config.as_ref() else {
        return ResolvedSection::default();
    };
    match cache.strategy.unwrap_or(AnthropicCacheStrategy::Auto) {
        AnthropicCacheStrategy::Auto | AnthropicCacheStrategy::Anthropic => {}
    }
    let setting = match section {
        Section::Tools => cache.tools_ttl.as_ref(),
        Section::SystemPrompt => cache.system_prompt_ttl.as_ref(),
        Section::Messages => cache.messages_ttl.as_ref(),
    };
    if matches!(setting, Some(AnthropicCacheSection::Disabled)) {
        return ResolvedSection::default();
    }
    let ttl = match setting {
        Some(AnthropicCacheSection::Ttl(ttl)) => Some(ttl.clone()),
        _ => cache.ttl.clone(),
    };
    ResolvedSection { enabled: true, ttl }
}

fn cache_control(section: &ResolvedSection) -> Value {
    match &section.ttl {
        Some(ttl) => json!({ "type": "ephemeral", "ttl": ttl }),
        None => json!({ "type": "ephemeral" }),
    }
}

fn cache_enabled(config: &AnthropicModelConfig) -> bool {
    config
        .cache_config
        .as_ref()
        .is_some_and(|c: &AnthropicCacheConfig| {
            matches!(
                c.strategy.unwrap_or(AnthropicCacheStrategy::Auto),
                AnthropicCacheStrategy::Auto | AnthropicCacheStrategy::Anthropic
            )
        })
}

/// Format the body of a streaming `POST /v1/messages` request.
pub fn format_request(
    config: &AnthropicModelConfig,
    model_id: &str,
    max_tokens: u64,
    messages: &[Message],
    options: &StreamOptions,
) -> Result<JsonValue> {
    if model_id.is_empty() {
        return Err(Error::model("Model ID is required".to_owned()));
    }
    let mut request = Map::new();
    request.insert("model".to_owned(), json!(model_id));
    request.insert("max_tokens".to_owned(), json!(max_tokens));
    request.insert(
        "messages".to_owned(),
        Value::Array(format_messages(config, messages, options)?),
    );
    request.insert("stream".to_owned(), json!(true));

    if let Some(system) = format_system_prompt(config, options.system_prompt.as_ref()) {
        request.insert("system".to_owned(), system);
    }
    if let Some(tools) = format_tools(config, &options.tool_specs, options.tool_choice.as_ref()) {
        request.insert("tools".to_owned(), Value::Array(tools));
    }
    if let Some(choice) = format_tool_choice(options.tool_choice.as_ref(), &request) {
        request.insert("tool_choice".to_owned(), choice);
    }
    if let Some(t) = config.temperature {
        request.insert("temperature".to_owned(), json!(t));
    }
    if let Some(p) = config.top_p {
        request.insert("top_p".to_owned(), json!(p));
    }
    if let Some(stop) = config.stop_sequences.as_ref().filter(|s| !s.is_empty()) {
        request.insert("stop_sequences".to_owned(), json!(stop));
    }
    if let Some(params) = &config.params {
        for (k, v) in params {
            request.insert(k.clone(), v.clone());
        }
    }
    let tools_empty = request
        .get("tools")
        .and_then(|t| t.as_array())
        .is_none_or(|a| a.is_empty());
    if tools_empty {
        request.remove("tools");
        request.remove("tool_choice");
    }
    Ok(Value::Object(request))
}

/// Format the body of a `POST /v1/messages/count_tokens` request (no `max_tokens`/`stream`).
pub fn format_count_tokens_request(
    config: &AnthropicModelConfig,
    model_id: &str,
    messages: &[Message],
    options: &CountTokensOptions,
) -> Result<JsonValue> {
    let stream_options = StreamOptions {
        system_prompt: options.system_prompt.clone(),
        tool_specs: options.tool_specs.clone(),
        ..Default::default()
    };
    let mut request = format_request(config, model_id, 1, messages, &stream_options)?;
    if let Some(map) = request.as_object_mut() {
        map.remove("max_tokens");
        map.remove("stream");
    }
    Ok(request)
}

fn format_tools(
    config: &AnthropicModelConfig,
    tool_specs: &[ToolSpec],
    tool_choice: Option<&ToolChoice>,
) -> Option<Vec<Value>> {
    let mut tools: Vec<Value> = tool_specs
        .iter()
        .map(|spec| {
            json!({
                "name": spec.name,
                "description": spec.description,
                "input_schema": spec.input_schema.clone().unwrap_or_else(|| json!({"type": "object"})),
            })
        })
        .collect();

    let server_tools: Vec<Value> = config.anthropic_tools.clone().unwrap_or_default();
    if !server_tools.is_empty() {
        match tool_choice {
            None => tools.extend(server_tools),
            Some(_) => {
                let names: Vec<String> = server_tools
                    .iter()
                    .map(|t| {
                        t.get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("unknown")
                            .to_owned()
                    })
                    .collect();
                warn_once(&format!(
                    "tool_choice=<{}>, server_tools=<{}> | forced tool call, omitting server tools",
                    tool_choice_name(tool_choice),
                    names.join(",")
                ));
            }
        }
    }
    if tools.is_empty() {
        return None;
    }
    let section = cache_section(config, Section::Tools);
    if section.enabled {
        if let Some(last) = tools.last_mut().and_then(|t| t.as_object_mut()) {
            last.insert("cache_control".to_owned(), cache_control(&section));
        }
    }
    Some(tools)
}

fn tool_choice_name(tool_choice: Option<&ToolChoice>) -> &'static str {
    match tool_choice {
        Some(ToolChoice::Auto {}) => "auto",
        Some(ToolChoice::Any {}) => "any",
        Some(ToolChoice::Tool { .. }) => "tool",
        None => "none",
    }
}

fn format_tool_choice(tool_choice: Option<&ToolChoice>, request: &Map<String, Value>) -> Option<Value> {
    let has_tools = request
        .get("tools")
        .and_then(|t| t.as_array())
        .is_some_and(|a| !a.is_empty());
    if !has_tools {
        return None;
    }
    match tool_choice? {
        ToolChoice::Auto {} => Some(json!({"type": "auto"})),
        ToolChoice::Any {} => Some(json!({"type": "any"})),
        ToolChoice::Tool { name } => Some(json!({"type": "tool", "name": name})),
    }
}

fn format_system_prompt(config: &AnthropicModelConfig, prompt: Option<&SystemPrompt>) -> Option<Value> {
    let prompt = prompt?;
    let section = cache_section(config, Section::SystemPrompt);
    match prompt {
        SystemPrompt::Text(text) if !section.enabled => Some(json!(text)),
        SystemPrompt::Text(text) => Some(json!([{
            "type": "text",
            "text": text,
            "cache_control": cache_control(&section),
        }])),
        SystemPrompt::Blocks(blocks) => {
            let mut out: Vec<Value> = Vec::new();
            let mut placed = false;
            for block in blocks {
                match block {
                    SystemContentBlock::Text(text) => {
                        out.push(json!({"type": "text", "text": text}));
                    }
                    SystemContentBlock::CachePoint(point) => {
                        if let Some(last) = out.last_mut().and_then(|b| b.as_object_mut()) {
                            let ttl = point.ttl.clone().or_else(|| section.ttl.clone());
                            last.insert(
                                "cache_control".to_owned(),
                                cache_control(&ResolvedSection { enabled: true, ttl }),
                            );
                            placed = true;
                        }
                    }
                    SystemContentBlock::GuardContent(_) => {
                        warn_once(
                            "block_type=<guardContentBlock> | guard content not supported in anthropic \
                             system prompt | skipping",
                        );
                    }
                }
            }
            if out.is_empty() {
                return None;
            }
            if section.enabled && !placed {
                if let Some(last) = out.last_mut().and_then(|b| b.as_object_mut()) {
                    last.insert("cache_control".to_owned(), cache_control(&section));
                }
            }
            Some(Value::Array(out))
        }
    }
}

fn is_cacheable(block: &Value) -> bool {
    matches!(
        block.get("type").and_then(|t| t.as_str()),
        Some("text" | "image" | "tool_use" | "tool_result" | "document")
    )
}

fn format_messages(
    config: &AnthropicModelConfig,
    messages: &[Message],
    options: &StreamOptions,
) -> Result<Vec<Value>> {
    let section = cache_section(config, Section::Messages);
    let cache_target = messages.iter().rposition(|m| {
        m.role == Role::User
            && m.content
                .iter()
                .any(|b| !matches!(b, ContentBlock::CachePoint(_)))
    });
    let mut stripped = 0usize;
    let mut out = Vec::new();

    for (index, message) in messages.iter().enumerate() {
        let role = match message.role {
            Role::Assistant => "assistant",
            _ => "user",
        };
        let is_target = Some(index) == cache_target;
        let mut blocks: Vec<Value> = Vec::new();
        let mut placed = false;
        for block in &message.content {
            if let ContentBlock::CachePoint(point) = block {
                let honored = if cache_enabled(config) {
                    is_target && !placed
                } else {
                    true
                };
                if !honored {
                    stripped += 1;
                    continue;
                }
                let ttl = point.ttl.clone().or_else(|| section.ttl.clone());
                if attach_cache_control(&mut blocks, &ResolvedSection { enabled: true, ttl }) {
                    placed = true;
                }
                continue;
            }
            if let Some(formatted) = format_content_block(block)? {
                blocks.extend(formatted);
            }
        }
        if is_target && section.enabled && !placed {
            let skip = options.dynamic_trailing_blocks.unwrap_or(0);
            let end = blocks.len().saturating_sub(skip);
            let candidates = &mut blocks[..end];
            attach_cache_control(candidates, &section);
        }
        if blocks.is_empty() {
            continue;
        }
        out.push(json!({"role": role, "content": blocks}));
    }
    if stripped > 0 {
        warn_once(&format!(
            "count=<{stripped}> | stripped extra cache points, only one cache point per request is used"
        ));
    }
    Ok(out)
}

fn attach_cache_control(blocks: &mut [Value], section: &ResolvedSection) -> bool {
    for block in blocks.iter_mut().rev() {
        if is_cacheable(block) {
            if let Some(map) = block.as_object_mut() {
                map.insert("cache_control".to_owned(), cache_control(section));
                return true;
            }
        }
    }
    false
}

fn image_media_type(format: ImageFormat) -> Result<&'static str> {
    Ok(match format {
        ImageFormat::Jpeg | ImageFormat::Jpg => "image/jpeg",
        ImageFormat::Png => "image/png",
        ImageFormat::Gif => "image/gif",
        ImageFormat::Webp => "image/webp",
    })
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Map one Strands content block onto zero or more Anthropic content blocks.
fn format_content_block(block: &ContentBlock) -> Result<Option<Vec<Value>>> {
    let value = match block {
        ContentBlock::Text(text) => json!({"type": "text", "text": text}),
        ContentBlock::Image(image) => match &image.source {
            ImageSource::Bytes(bytes) => json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": image_media_type(image.format)?,
                    "data": b64(bytes),
                },
            }),
            ImageSource::Url(url) => json!({"type": "image", "source": {"type": "url", "url": url}}),
            ImageSource::Location(_) => {
                warn_once(
                    "source_type=<imageSourceS3Location> | s3 location sources are not supported by \
                     anthropic | skipping",
                );
                return Ok(None);
            }
        },
        ContentBlock::Document(doc) => match (&doc.format, &doc.source) {
            (DocumentFormat::Pdf, DocumentSource::Bytes(bytes)) => json!({
                "type": "document",
                "source": {"type": "base64", "media_type": "application/pdf", "data": b64(bytes)},
                "title": doc.name,
            }),
            (format, source) if TEXT_FILE_FORMATS.contains(format) => {
                let text = match source {
                    DocumentSource::Text(text) => text.clone(),
                    DocumentSource::Bytes(bytes) => String::from_utf8_lossy(bytes).into_owned(),
                    DocumentSource::Content(parts) => {
                        parts.iter().map(|p| p.text.clone()).collect::<Vec<_>>().join("")
                    }
                    _ => {
                        return Err(Error::model(format!(
                            "Unsupported document format or source for Anthropic: format={format:?}, \
                             source={source:?}"
                        )))
                    }
                };
                json!({"type": "text", "text": text})
            }
            (format, source) => {
                return Err(Error::model(format!(
                "Unsupported document format or source for Anthropic: format={format:?}, source={source:?}"
            )))
            }
        },
        ContentBlock::ToolUse(tool_use) => json!({
            "type": "tool_use",
            "id": tool_use.tool_use_id,
            "name": tool_use.name,
            "input": tool_use.input,
        }),
        ContentBlock::ToolResult(result) => {
            let mut content: Vec<Value> = Vec::new();
            for item in &result.content {
                match item {
                    ToolResultContent::Text(text) => content.push(json!({"type": "text", "text": text})),
                    ToolResultContent::Json(value) => content.push(json!({
                        "type": "text",
                        "text": serde_json::to_string(value).unwrap_or_default(),
                    })),
                    ToolResultContent::Image(image) => {
                        if let Some(mut formatted) =
                            format_content_block(&ContentBlock::Image(image.clone()))?
                        {
                            content.append(&mut formatted);
                        }
                    }
                    ToolResultContent::Document(doc) => {
                        if let Some(mut formatted) =
                            format_content_block(&ContentBlock::Document(doc.clone()))?
                        {
                            content.append(&mut formatted);
                        }
                    }
                    _ => {}
                }
            }
            let collapsed = match content.as_slice() {
                [only] if only.get("type").and_then(|t| t.as_str()) == Some("text") => {
                    only.get("text").cloned().unwrap_or_else(|| json!(""))
                }
                _ => Value::Array(content),
            };
            json!({
                "type": "tool_result",
                "tool_use_id": result.tool_use_id,
                "content": collapsed,
                "is_error": result.status == ToolResultStatus::Error,
            })
        }
        ContentBlock::Reasoning(reasoning) => {
            if let Some(signature) = &reasoning.signature {
                json!({
                    "type": "thinking",
                    "thinking": reasoning.text.clone().unwrap_or_default(),
                    "signature": signature,
                })
            } else if let Some(redacted) = &reasoning.redacted_content {
                json!({"type": "redacted_thinking", "data": b64(redacted)})
            } else {
                return Ok(None);
            }
        }
        ContentBlock::Citations(citations) => {
            let text: String = citations
                .content
                .iter()
                .map(|c| c.text.clone())
                .collect::<Vec<_>>()
                .join("");
            if text.is_empty() {
                return Ok(None);
            }
            json!({"type": "text", "text": text})
        }
        ContentBlock::Video(_) => {
            warn_once("block_type=<videoBlock> | video is not supported by anthropic | skipping");
            return Ok(None);
        }
        _ => return Ok(None),
    };
    Ok(Some(vec![value]))
}
