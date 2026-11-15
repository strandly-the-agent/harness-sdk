//! Formatting a Strands request as an Anthropic Messages request (TS `_formatRequest` and the
//! helpers it calls).

use serde_json::json;

use super::AnthropicModelConfig;
use crate::error::{Error, Result};
use crate::models::model::{
    CacheConfig, CacheSection, CacheStrategy, CacheTtl, ResolvedCacheSection, resolve_cache_section,
};
use crate::models::model_info::{ModelInfo, PromptCaching};
use crate::types::citations::CitationGeneratedContent;
use crate::types::json::{JsonObject, JsonValue, to_js_string};
use crate::types::media::{DocumentBlock, DocumentSource, ImageBlock, ImageSource, encode_base64};
use crate::types::messages::{
    ContentBlock, Message, Role, SummaryBlock, SystemContentBlock, SystemPrompt, ToolResultContent,
};
use crate::types::tools::{ToolChoice, ToolResultStatus, ToolSpec};

/// `ephemeral` is the only cache type the Anthropic API supports.
const ANTHROPIC_CACHE_TYPE: &str = "ephemeral";
/// Beta that lets a mid-conversation system message carry `clear_at`.
pub(crate) const TURN_SCOPED_SYSTEM_BETA: &str = "mid-conversation-system-clear-at-2026-08-21";
/// API version 2026-10-21 removed the sampling params and rejects document blocks in assistant messages.
pub(crate) const SAMPLING_PARAMS_REMOVED_VERSION: &str = "2026-10-21";
const REMOVED_SAMPLING_PARAMS: [&str; 3] = ["temperature", "top_p", "top_k"];
const TEXT_FILE_FORMATS: [&str; 12] = [
    "txt", "md", "markdown", "csv", "json", "xml", "html", "yml", "yaml", "js", "ts", "py",
];
const CACHEABLE_BLOCK_TYPES: [&str; 5] = ["text", "image", "tool_use", "tool_result", "document"];

/// The API version a built client sends, and where it comes from, for error messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApiVersion {
    pub(crate) value: String,
    pub(crate) origin: String,
}

/// The request parts that come from the call rather than the config.
#[derive(Debug, Default)]
pub(crate) struct FormatOptions<'a> {
    pub(crate) system_prompt: Option<&'a SystemPrompt>,
    pub(crate) tool_specs: &'a [ToolSpec],
    pub(crate) tool_choice: Option<&'a ToolChoice>,
    pub(crate) dynamic_trailing_blocks: usize,
}

/// Everything formatting reads from the provider, snapshotted for one request.
pub(crate) struct Formatter<'a> {
    pub(crate) config: &'a AnthropicModelConfig,
    pub(crate) model_id: &'a str,
    pub(crate) max_tokens: u64,
    pub(crate) model_info: &'a ModelInfo,
    pub(crate) api_version: Option<&'a ApiVersion>,
    pub(crate) drops_stale_thinking: bool,
    pub(crate) warn_once: &'a (dyn Fn(&str) + Send + Sync),
}

/// The tool definitions `anthropic_tools` must not contain (TS `validateAnthropicTools`).
pub(crate) fn validate_anthropic_tools(tools: &[JsonValue]) -> Result<()> {
    if tools.iter().any(|tool| tool.get("input_schema").is_some()) {
        return Err(Error::other(
            "anthropicTools should not contain function tool definitions. Use the standard tools interface for function calling tools. anthropicTools is reserved for Anthropic server-side tools like web_search, web_fetch, and code_execution.",
        ));
    }
    Ok(())
}

/// A server tool's `name`, or its `type` for a toolset, which has none.
fn server_tool_name(tool: &JsonValue) -> &str {
    tool.get("name")
        .or_else(|| tool.get("type"))
        .and_then(JsonValue::as_str)
        .unwrap_or_default()
}

/// Whether `block` is a compaction block Anthropic returned.
pub(crate) fn is_compaction(block: &ContentBlock) -> bool {
    matches!(block, ContentBlock::Summary(summary) if summary.provider() == Some("anthropic"))
}

/// The messages from the last compaction block, which stands in for everything before it, or all
/// of them. The API rejects any content before a compaction block.
pub(crate) fn from_last_compaction(messages: &[Message]) -> Vec<Message> {
    for (index, message) in messages.iter().enumerate().rev() {
        let Some(position) = message.content().iter().rposition(is_compaction) else {
            continue;
        };
        let content = message.content().get(position..).unwrap_or_default().to_vec();
        let first = Message::new(message.role(), content);
        return std::iter::once(first)
            .chain(messages.iter().skip(index.saturating_add(1)).cloned())
            .collect();
    }
    messages.to_vec()
}

/// Builds an Anthropic `cache_control` value; an empty `ttl` leaves the API default.
fn cache_control(ttl: Option<&str>) -> JsonValue {
    let mut control = JsonObject::new();
    control.insert("type".to_owned(), json!(ANTHROPIC_CACHE_TYPE));
    if let Some(ttl) = ttl.filter(|ttl| !ttl.is_empty()) {
        control.insert("ttl".to_owned(), json!(ttl));
    }
    JsonValue::Object(control)
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

fn block_type(block: &JsonValue) -> &str {
    block.get("type").and_then(JsonValue::as_str).unwrap_or_default()
}

fn is_cacheable(block: &JsonValue) -> bool {
    CACHEABLE_BLOCK_TYPES.contains(&block_type(block))
}

/// Marks the last formatted block the API accepts `cache_control` on, skipping `skip_trailing`
/// blocks rebuilt every call; true when a block was marked.
fn attach_cache_control(content: &mut [JsonValue], ttl: Option<&str>, skip_trailing: usize) -> bool {
    let end = content.len().saturating_sub(skip_trailing);
    let Some(block) = content
        .get_mut(..end)
        .and_then(|head| head.iter_mut().rev().find(|block| is_cacheable(block)))
    else {
        return false;
    };
    if let Some(object) = block.as_object_mut() {
        object.insert("cache_control".to_owned(), cache_control(ttl));
    }
    true
}

fn ttl_str(section: &ResolvedCacheSection) -> Option<&str> {
    section.ttl.as_ref().map(CacheTtl::as_str)
}

fn image_media_type(format: &str) -> Option<&'static str> {
    match format {
        "jpeg" | "jpg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

fn format_image(image: &ImageBlock) -> Result<Option<JsonValue>> {
    let format = image.format().as_str();
    let media_type = image_media_type(format)
        .ok_or_else(|| Error::other(format!("Unsupported image format for Anthropic: {format}")))?;
    Ok(match image.source() {
        ImageSource::Bytes(bytes) => Some(json!({
            "type": "image",
            "source": { "type": "base64", "media_type": media_type, "data": encode_base64(bytes) },
        })),
        ImageSource::Url(url) => Some(json!({ "type": "image", "source": { "type": "url", "url": url } })),
        ImageSource::Location(_) => {
            tracing::warn!(
                "source_type=<imageSourceS3Location> | s3 location sources are not supported by anthropic | skipping"
            );
            None
        }
    })
}

fn document_source_type(source: &DocumentSource) -> &'static str {
    match source {
        DocumentSource::Bytes(_) => "documentSourceBytes",
        DocumentSource::Text(_) => "documentSourceText",
        DocumentSource::Content(_) => "documentSourceContentBlock",
        DocumentSource::Location(_) => "documentSourceS3Location",
    }
}

fn format_document(document: &DocumentBlock) -> Result<JsonValue> {
    let format = document.format().as_str();
    if let (DocumentSource::Bytes(bytes), "pdf") = (document.source(), format) {
        let mut block = json!({
            "type": "document",
            "source": { "type": "base64", "media_type": "application/pdf", "data": encode_base64(bytes) },
        });
        if let (Some(object), false) = (block.as_object_mut(), document.name().is_empty()) {
            object.insert("title".to_owned(), json!(document.name()));
        }
        return Ok(block);
    }
    if TEXT_FILE_FORMATS.contains(&format) {
        let text = match document.source() {
            DocumentSource::Text(text) => Some(text.clone()),
            DocumentSource::Bytes(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
            _ => None,
        };
        if let Some(text) = text.filter(|text| !text.is_empty()) {
            return Ok(json!({ "type": "text", "text": text }));
        }
    }
    Err(Error::other(format!(
        "Unsupported document format or source for Anthropic: format={format}, source={}",
        document_source_type(document.source())
    )))
}

fn format_tool_result_content(content: &ToolResultContent) -> Result<Option<JsonValue>> {
    Ok(match content {
        ToolResultContent::Text(text) => Some(json!({ "type": "text", "text": text.text() })),
        ToolResultContent::Json(block) => {
            Some(json!({ "type": "text", "text": to_js_string(block.json())? }))
        }
        ToolResultContent::Image(image) => format_image(image)?,
        ToolResultContent::Document(document) => Some(format_document(document)?),
        ToolResultContent::Video(_) => {
            warn_video();
            None
        }
        _ => None,
    })
}

fn warn_video() {
    tracing::warn!("block_type=<videoBlock> | video blocks not supported by anthropic, skipping");
}

/// The injected text of `message` as a system message the model reads only until a later user
/// message exists. Never carries `cache_control`: a cleared message is not part of any cache key.
fn turn_scoped_system_message(message: &Message) -> Option<JsonValue> {
    let content: Vec<JsonValue> = message
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Injected(injected) => Some(json!({ "type": "text", "text": injected.text() })),
            _ => None,
        })
        .collect();
    (!content.is_empty())
        .then(|| json!({ "role": "system", "content": content, "clear_at": "next_user_message" }))
}

fn formatted_role(message: &JsonValue) -> &str {
    message
        .get("role")
        .and_then(JsonValue::as_str)
        .unwrap_or_default()
}

fn has_content(message: &JsonValue) -> bool {
    message
        .get("content")
        .and_then(JsonValue::as_array)
        .is_some_and(|content| !content.is_empty())
}

/// Drops the formatted messages left empty, which the API rejects anywhere but the trailing
/// assistant turn. With temporary messages, each user message's injected text follows it as a
/// turn-scoped system message (TS `requestMessages`).
fn request_messages(
    messages: &[Message],
    formatted: Vec<JsonValue>,
    temporary_messages: bool,
) -> Vec<JsonValue> {
    let kept: Vec<(JsonValue, &Message)> = formatted
        .into_iter()
        .zip(messages)
        .filter(|(message, _)| has_content(message))
        .collect();
    if !temporary_messages {
        return kept.into_iter().map(|(message, _)| message).collect();
    }
    let next_is_user: Vec<bool> = (0..kept.len())
        .map(|index| {
            kept.get(index.saturating_add(1))
                .is_some_and(|(message, _)| formatted_role(message) == "user")
        })
        .collect();
    let mut result = Vec::with_capacity(kept.len());
    for ((message, source), next_is_user) in kept.into_iter().zip(next_is_user) {
        let system = (formatted_role(&message) == "user" && !next_is_user)
            .then(|| turn_scoped_system_message(source))
            .flatten();
        result.push(message);
        result.extend(system);
    }
    result
}

impl Formatter<'_> {
    fn server_side_summary(&self) -> bool {
        self.model_info
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.server_side_summary)
            == Some(true)
    }

    fn temporary_messages(&self) -> bool {
        self.model_info
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.temporary_messages)
            == Some(true)
    }

    /// Whether the request goes to an API version that dropped the sampling params and assistant documents.
    fn uses_current_api_version(&self) -> bool {
        self.api_version
            .is_some_and(|version| version.value.as_str() >= SAMPLING_PARAMS_REMOVED_VERSION)
    }

    /// Resolves a cache section, disabled when `cache_config` is unset.
    pub(crate) fn cache_section(
        &self,
        section: fn(&CacheConfig) -> Option<&CacheSection>,
    ) -> ResolvedCacheSection {
        let disabled = ResolvedCacheSection {
            enabled: false,
            ttl: None,
        };
        let Some(cache_config) = &self.config.cache_config else {
            return disabled;
        };
        let strategy = cache_config.strategy.unwrap_or(CacheStrategy::Auto);
        let unsupported = self
            .model_info
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.prompt_caching.as_ref())
            == Some(&PromptCaching::Unsupported);
        if strategy == CacheStrategy::Auto && unsupported {
            (self.warn_once)(&format!(
                "model_id=<{}> | cacheConfig is enabled but the model info says this model does not support prompt caching, ignoring",
                self.model_id
            ));
            return disabled;
        }
        resolve_cache_section(section(cache_config), &[cache_config.ttl.as_ref()])
    }

    /// The API rejects these params with a 400, so a request that carries them fails here with the fixes instead.
    fn reject_removed_sampling_params(&self, request: &JsonObject) -> Result<()> {
        let Some(version) = self.api_version.filter(|_| self.uses_current_api_version()) else {
            return Ok(());
        };
        let set: Vec<&str> = REMOVED_SAMPLING_PARAMS
            .into_iter()
            .filter(|name| request.contains_key(*name))
            .collect();
        if set.is_empty() {
            return Ok(());
        }
        let (verb, noun) = if set.len() == 1 {
            ("is", "param")
        } else {
            ("are", "params")
        };
        Err(Error::other(format!(
            "{} {verb} not supported by Anthropic API version {}, {}. Remove the {noun}, or pin API version 2023-06-01 for models that still honor them: set 'anthropic-version': '2023-06-01' in client_config.default_headers",
            set.join(", "),
            version.value,
            version.origin
        )))
    }

    /// Formats a summary as a compaction block when Anthropic wrote it and compaction is on, else as text.
    fn format_summary(&self, summary: &SummaryBlock) -> JsonValue {
        if summary.provider() != Some("anthropic") || !self.server_side_summary() {
            return json!({ "type": "text", "text": summary.text() });
        }
        let mut block = json!({ "type": "compaction", "content": summary.text() });
        if let (Some(object), Some(signature)) = (block.as_object_mut(), non_empty(summary.signature())) {
            object.insert("signature".to_owned(), json!(signature));
        }
        block
    }

    fn format_content_block(&self, block: &ContentBlock) -> Result<Option<JsonValue>> {
        Ok(match block {
            ContentBlock::Text(text) => Some(json!({ "type": "text", "text": text.text() })),
            ContentBlock::Summary(summary) => Some(self.format_summary(summary)),
            ContentBlock::Image(image) => format_image(image)?,
            ContentBlock::Document(document) => Some(format_document(document)?),
            ContentBlock::ToolUse(tool_use) => Some(json!({
                "type": "tool_use",
                "id": tool_use.tool_use_id(),
                "name": tool_use.name(),
                "input": tool_use.input(),
            })),
            ContentBlock::Video(_) => {
                warn_video();
                None
            }
            ContentBlock::ToolResult(result) => {
                let mut inner = Vec::new();
                for content in result.content() {
                    inner.extend(format_tool_result_content(content)?);
                }
                let content = match inner.as_slice() {
                    [only] if block_type(only) == "text" => only.get("text").cloned().unwrap_or_default(),
                    _ => JsonValue::Array(inner),
                };
                Some(json!({
                    "type": "tool_result",
                    "tool_use_id": result.tool_use_id(),
                    "content": content,
                    "is_error": result.status() == &ToolResultStatus::Error,
                }))
            }
            ContentBlock::Reasoning(reasoning) => {
                match (non_empty(reasoning.signature()), reasoning.redacted_content()) {
                    (Some(signature), _) => Some(json!({
                        "type": "thinking",
                        "thinking": reasoning.text().unwrap_or_default(),
                        "signature": signature,
                    })),
                    (None, Some(redacted)) => {
                        Some(json!({ "type": "redacted_thinking", "data": encode_base64(redacted) }))
                    }
                    (None, None) => None,
                }
            }
            ContentBlock::Citations(citations) => {
                let text: String = citations
                    .content()
                    .iter()
                    .map(CitationGeneratedContent::text)
                    .collect();
                (!text.is_empty()).then(|| json!({ "type": "text", "text": text }))
            }
            _ => None,
        })
    }

    /// Formats the system prompt, adding a cache point at its end when caching is on; `None`
    /// when nothing sendable remains.
    fn format_system_prompt(&self, system_prompt: &SystemPrompt) -> Option<JsonValue> {
        let system_cache = self.cache_section(|config| config.system_prompt_ttl.as_ref());
        let blocks = match system_prompt {
            SystemPrompt::Text(text) if !system_cache.enabled => return Some(json!(text)),
            SystemPrompt::Text(text) => {
                return Some(
                    json!([{ "type": "text", "text": text, "cache_control": cache_control(ttl_str(&system_cache)) }]),
                );
            }
            SystemPrompt::Blocks(blocks) => blocks,
        };
        let mut system_blocks = Vec::new();
        let mut placed_cache_point = false;
        let mut iter = blocks.iter().peekable();
        while let Some(block) = iter.next() {
            let text = match block {
                SystemContentBlock::Text(text) => text,
                SystemContentBlock::GuardContent(_) => {
                    tracing::warn!(
                        "block_type=<guardContentBlock> | guard content not supported in anthropic system prompt | skipping"
                    );
                    continue;
                }
                _ => continue,
            };
            let mut formatted = json!({ "type": "text", "text": text.text() });
            if let Some(SystemContentBlock::CachePoint(point)) = iter.peek() {
                let ttl = non_empty(point.ttl()).or(ttl_str(&system_cache));
                if let Some(object) = formatted.as_object_mut() {
                    object.insert("cache_control".to_owned(), cache_control(ttl));
                }
                placed_cache_point = true;
                iter.next();
            }
            system_blocks.push(formatted);
        }
        if system_cache.enabled
            && !placed_cache_point
            && let Some(object) = system_blocks.last_mut().and_then(JsonValue::as_object_mut)
        {
            object.insert("cache_control".to_owned(), cache_control(ttl_str(&system_cache)));
        }
        (!system_blocks.is_empty()).then_some(JsonValue::Array(system_blocks))
    }

    fn format_messages(
        &self,
        messages: &[Message],
        dynamic_trailing_blocks: usize,
    ) -> Result<Vec<JsonValue>> {
        let messages_cache = self.cache_section(|config| config.messages_ttl.as_ref());
        let cache_managed = messages_cache.enabled;
        // The cache point goes on the last user message with content, not counting cache points.
        let cache_target = cache_managed
            .then(|| {
                messages.iter().rposition(|message| {
                    message.role() == Role::User
                        && message
                            .content()
                            .iter()
                            .any(|block| !matches!(block, ContentBlock::CachePoint(_)))
                })
            })
            .flatten();
        let mut stripped_cache_points = 0usize;
        let mut formatted = Vec::with_capacity(messages.len());
        for (message_index, message) in messages.iter().enumerate() {
            let is_cache_target = cache_target == Some(message_index);
            let mut content: Vec<JsonValue> = Vec::new();
            let mut marked = false;
            let mut honored = false;
            for block in message.content() {
                if let ContentBlock::CachePoint(point) = block {
                    if !cache_managed {
                        if !attach_cache_control(&mut content, point.ttl(), 0) {
                            tracing::warn!("no preceding block accepts a cache point | skipped cache point");
                        }
                    } else if is_cache_target && !honored {
                        // A TTL written on the point is more specific than the configured one.
                        honored = true;
                        let ttl = non_empty(point.ttl()).or(ttl_str(&messages_cache));
                        marked = attach_cache_control(&mut content, ttl, 0);
                        if !marked {
                            tracing::warn!(
                                "msg_idx=<{message_index}> | nothing ahead of the placed cache point can carry one, falling back to automatic placement"
                            );
                        }
                    } else {
                        stripped_cache_points = stripped_cache_points.saturating_add(1);
                    }
                    continue;
                }
                let Some(formatted_block) = self.format_content_block(block)? else {
                    continue;
                };
                if block_type(&formatted_block) == "document"
                    && message.role() == Role::Assistant
                    && self.uses_current_api_version()
                {
                    let version = self
                        .api_version
                        .map(|version| version.value.as_str())
                        .unwrap_or_default();
                    tracing::warn!(
                        "msg_idx=<{message_index}>, api_version=<{version}> | document blocks are not accepted in assistant messages, skipping"
                    );
                    continue;
                }
                content.push(formatted_block);
            }
            // Placed after formatting so the cache point lands on a block that survived translation.
            if is_cache_target && !marked {
                if attach_cache_control(&mut content, ttl_str(&messages_cache), dynamic_trailing_blocks) {
                    tracing::debug!("msg_idx=<{message_index}> | added cache point to last user message");
                } else {
                    tracing::debug!(
                        "msg_idx=<{message_index}> | no cacheable content block, skipped cache point"
                    );
                }
            }
            formatted.push(json!({ "role": message.role().as_str(), "content": content }));
        }
        if stripped_cache_points > 0 {
            tracing::warn!(
                "count=<{stripped_cache_points}> | stripped extra cache points, cacheConfig keeps the first cache point in the last user message; unset cacheConfig to keep every cache point"
            );
        }
        Ok(request_messages(messages, formatted, self.temporary_messages()))
    }

    fn format_tools(&self, options: &FormatOptions<'_>) -> Vec<JsonValue> {
        let mut tools: Vec<JsonValue> = options
            .tool_specs
            .iter()
            .map(|spec| {
                let mut tool = json!({ "name": spec.name(), "description": spec.description() });
                if let (Some(object), Some(schema)) = (tool.as_object_mut(), spec.input_schema()) {
                    object.insert("input_schema".to_owned(), schema.clone());
                }
                tool
            })
            .collect();
        let params_tools = self
            .config
            .params
            .as_ref()
            .and_then(|params| params.get("tools"))
            .and_then(JsonValue::as_array);
        let server_tools: Vec<&JsonValue> = self
            .config
            .anthropic_tools
            .iter()
            .flatten()
            .chain(params_tools.into_iter().flatten())
            .collect();
        // Forcing a tool means this turn must call a function tool, so server tools are left out.
        match options.tool_choice {
            None | Some(ToolChoice::Auto) => tools.extend(server_tools.into_iter().cloned()),
            Some(choice) if !server_tools.is_empty() => {
                let names: Vec<&str> = server_tools.iter().map(|tool| server_tool_name(tool)).collect();
                tracing::warn!(
                    "tool_choice=<{}>, server_tools=<{}> | forced tool call, omitting server tools",
                    tool_choice_key(choice),
                    names.join(",")
                );
            }
            Some(_) => {}
        }
        // A cache_control on the last tool caches all of them, so one cache point suffices.
        let tools_cache = self.cache_section(|config| config.tools_ttl.as_ref());
        if tools_cache.enabled
            && let Some(object) = tools.last_mut().and_then(JsonValue::as_object_mut)
        {
            object.insert("cache_control".to_owned(), cache_control(ttl_str(&tools_cache)));
        }
        tools
    }

    /// Formats a Messages request (TS `_formatRequest`).
    pub(crate) fn format_request(
        &self,
        messages: &[Message],
        options: &FormatOptions<'_>,
    ) -> Result<JsonObject> {
        let compacted;
        let messages = if self.server_side_summary() {
            compacted = from_last_compaction(messages);
            compacted.as_slice()
        } else {
            messages
        };
        let mut request = JsonObject::new();
        request.insert("model".to_owned(), json!(self.model_id));
        request.insert("max_tokens".to_owned(), json!(self.max_tokens));
        request.insert(
            "messages".to_owned(),
            JsonValue::Array(self.format_messages(messages, options.dynamic_trailing_blocks)?),
        );
        request.insert("stream".to_owned(), json!(true));
        if let Some(system) = options
            .system_prompt
            .filter(|prompt| !matches!(prompt, SystemPrompt::Text(text) if text.is_empty()))
            .and_then(|prompt| self.format_system_prompt(prompt))
        {
            request.insert("system".to_owned(), system);
        }
        let tools = self.format_tools(options);
        if let (false, Some(choice)) = (tools.is_empty(), options.tool_choice) {
            let formatted = match choice {
                ToolChoice::Auto => json!({ "type": "auto" }),
                ToolChoice::Any => json!({ "type": "any" }),
                ToolChoice::Tool { name } => json!({ "type": "tool", "name": name }),
            };
            request.insert("tool_choice".to_owned(), formatted);
        }
        if let Some(stop_sequences) = &self.config.stop_sequences {
            request.insert("stop_sequences".to_owned(), json!(stop_sequences));
        }
        if let Some(temperature) = self.config.temperature {
            request.insert("temperature".to_owned(), json!(temperature));
        }
        if let Some(top_p) = self.config.top_p {
            request.insert("top_p".to_owned(), json!(top_p));
        }
        for (key, value) in self.config.params.iter().flatten() {
            request.insert(key.clone(), value.clone());
        }
        self.reject_removed_sampling_params(&request)?;
        if self.drops_stale_thinking
            && let Some(thinking) = thinking_with_drop_block(request.get("thinking"))
        {
            request.insert("thinking".to_owned(), thinking);
        }
        // params.tools is already merged into tools above; never let it bypass the forced-turn rule.
        if tools.is_empty() {
            request.remove("tools");
        } else {
            request.insert("tools".to_owned(), JsonValue::Array(tools));
        }
        Ok(request)
    }
}

fn tool_choice_key(choice: &ToolChoice) -> &'static str {
    match choice {
        ToolChoice::Auto => "auto",
        ToolChoice::Any => "any",
        ToolChoice::Tool { .. } => "tool",
    }
}

/// Beta that lets a request drop thinking blocks invalidated by an edit to the history before them.
pub(crate) const THINKING_BINDING_BETA: &str = "thinking-binding-controls-2026-08-01";
const THINKING_BINDING_ERROR: &str = "bound to a different conversation";

/// Whether the provider rejected a replayed thinking block because history before it changed.
pub(crate) fn is_thinking_binding_error(message: &str) -> bool {
    message.contains(THINKING_BINDING_ERROR)
}

/// The thinking config with stale blocks dropped, or `None` when it cannot or should not change.
/// A caller-set `block_binding` is left as written.
pub(crate) fn thinking_with_drop_block(thinking: Option<&JsonValue>) -> Option<JsonValue> {
    let thinking = thinking?.as_object()?;
    if thinking.contains_key("block_binding") {
        return None;
    }
    let mut dropped = thinking.clone();
    dropped.insert(
        "block_binding".to_owned(),
        json!({ "prefix_mismatch_behavior": "drop_block" }),
    );
    Some(JsonValue::Object(dropped))
}
