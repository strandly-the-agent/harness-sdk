//! Converse request formatting (TS `_formatRequest` and the helpers it calls).

use serde_json::json;

use super::BedrockModelConfig;
use super::thinking_binding::fields_with_drop_block;
use super::{GuardrailTrace, IncludeToolResultStatus};
use crate::error::{Error, Result};
use crate::logging::warn_once::warn_once;
use crate::models::model::{CacheSection, CacheStrategy, CacheTtl, ModelRequest, resolve_cache_section};
use crate::models::model_info::{ModelInfo, PromptCaching};
use crate::models::strict_schema::{ensure_strict_json_schema, find_unsupported_strict_keywords};
use crate::types::citations::{Citation, CitationLocation, CitationsBlock};
use crate::types::json::{JsonObject, JsonValue};
use crate::types::media::{
    AudioSource, DocumentBlock, DocumentSource, ImageBlock, ImageSource, S3Location, VideoBlock, VideoSource,
    encode_base64,
};
use crate::types::messages::{
    ContentBlock, GuardContentBlock, Message, ReasoningBlock, Role, SystemContentBlock, SystemPrompt,
    ToolResultBlock, ToolResultContent,
};
use crate::types::tools::{ToolChoice, ToolSpec};

/// Models Bedrock documents as accepting `status` in tool results (an allowlist).
const MODELS_INCLUDE_STATUS: [&str; 2] = ["amazon.nova", "anthropic.claude"];

/// Models that reject a reasoning text block replayed in history.
const MODELS_REJECT_REASONING_TEXT: [&str; 16] = [
    "amazon.nova-lite",
    "amazon.nova-pro",
    "google.gemma",
    "meta.llama",
    "mistral.devstral",
    "mistral.ministral",
    "mistral.mistral-large",
    "nvidia.nemotron-nano-9b",
    "nvidia.nemotron-super",
    "openai.gpt-5",
    "openai.gpt-6",
    "openai.gpt-daybreak",
    "qwen.qwen3-235b",
    "qwen.qwen3-coder",
    "writer.palmyra",
    "xai.grok",
];

/// What formatting a request reads from the model.
pub(crate) struct FormatContext {
    pub(crate) config: BedrockModelConfig,
    pub(crate) model_info: ModelInfo,
    pub(crate) resolved_max_tokens: Option<u64>,
    pub(crate) drop_stale_thinking: bool,
}

/// The tool the request carries when history has tool blocks but no tools were given (TS
/// `NOOP_TOOL_SPEC`); Bedrock requires a tool config whenever messages reference tools.
pub(crate) fn noop_tool_spec() -> ToolSpec {
    ToolSpec::new("noop", "This is a fake tool that MUST be completely ignored.")
        .with_input_schema(json!({ "type": "object", "properties": {} }))
}

fn is_cache_point(block: &JsonValue) -> bool {
    block.get("cachePoint").is_some()
}

fn is_non_pdf_document(block: &JsonValue) -> bool {
    block
        .get("document")
        .is_some_and(|document| document.get("format").and_then(JsonValue::as_str) != Some("pdf"))
}

fn cache_point(ttl: Option<&CacheTtl>) -> JsonValue {
    let mut point = json!({ "type": "default" });
    if let (Some(ttl), Some(point)) = (ttl, point.as_object_mut()) {
        point.insert("ttl".to_owned(), JsonValue::from(ttl.as_str()));
    }
    json!({ "cachePoint": point })
}

fn set_cache_point_ttl(block: &mut JsonValue, ttl: Option<&CacheTtl>) {
    let Some(point) = block.get_mut("cachePoint").and_then(JsonValue::as_object_mut) else {
        return;
    };
    point.remove("ttl");
    if let Some(ttl) = ttl {
        point.insert("ttl".to_owned(), JsonValue::from(ttl.as_str()));
    }
}

fn cache_point_ttl(block: &JsonValue) -> Option<&str> {
    block
        .get("cachePoint")
        .and_then(|point| point.get("ttl"))
        .and_then(JsonValue::as_str)
        .filter(|ttl| !ttl.is_empty())
}

impl FormatContext {
    fn model_id(&self) -> &str {
        self.config.effective_model_id()
    }

    /// `anthropic` when the model takes cache points (TS `_getCacheStrategy`).
    fn detected_cache_strategy(&self) -> Option<CacheStrategy> {
        let prompt_caching = self
            .model_info
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.prompt_caching.as_ref());
        match prompt_caching {
            // Unlisted ids keep the name test so caching doesn't turn off until the data catches up.
            None => {
                let model_id = self.model_id().to_lowercase();
                (model_id.contains("claude") || model_id.contains("anthropic"))
                    .then_some(CacheStrategy::Anthropic)
            }
            Some(PromptCaching::CachePoints) => Some(CacheStrategy::Anthropic),
            Some(_) => None,
        }
    }

    /// Whether cache points are placed (TS `_shouldEnableCaching`).
    pub(crate) fn caching_enabled(&self) -> bool {
        let Some(cache_config) = &self.config.cache_config else {
            return false;
        };
        let strategy = cache_config.strategy.unwrap_or(CacheStrategy::Auto);
        if strategy != CacheStrategy::Auto {
            return strategy == CacheStrategy::Anthropic;
        }
        if self.detected_cache_strategy().is_none() {
            warn_once(&format!(
                "model_id=<{}> | cache_config is enabled but this model does not support automatic caching",
                self.config.model_id.as_deref().unwrap_or_default()
            ));
            return false;
        }
        true
    }

    fn section(
        &self,
        pick: fn(&super::BedrockCacheConfig) -> Option<&CacheSection>,
    ) -> (bool, Option<CacheTtl>) {
        if !self.caching_enabled() {
            return (false, None);
        }
        let cache_config = self.config.cache_config.as_ref();
        let resolved = resolve_cache_section(
            cache_config.and_then(pick),
            &[cache_config.and_then(|c| c.ttl.as_ref())],
        );
        (resolved.enabled, resolved.ttl)
    }

    fn include_tool_result_status(&self) -> bool {
        match self
            .config
            .include_tool_result_status
            .unwrap_or(IncludeToolResultStatus::Auto)
        {
            IncludeToolResultStatus::Enabled(include) => include,
            IncludeToolResultStatus::Auto => {
                let model_id = self.config.model_id.as_deref().unwrap_or_default();
                let include = MODELS_INCLUDE_STATUS
                    .iter()
                    .any(|pattern| model_id.contains(pattern));
                tracing::debug!(
                    "model_id=<{model_id}>, include_tool_result_status=<{include}> | auto-detected includeToolResultStatus"
                );
                include
            }
        }
    }

    /// Whether the model rejects `block` replayed in history (TS `_rejectsReasoningBlock`).
    fn rejects_reasoning_block(&self, block: &ReasoningBlock) -> bool {
        let model_id = self.model_id().to_lowercase();
        if model_id.contains("deepseek") {
            return true;
        }
        if block.text().is_none() && block.signature().is_none() {
            return false;
        }
        let signed = block.signature().is_some_and(|signature| !signature.is_empty());
        if model_id.contains("anthropic") || model_id.contains("claude") {
            return !signed;
        }
        if model_id.starts_with("arn:") || !model_id.contains('.') {
            return false;
        }
        if MODELS_REJECT_REASONING_TEXT
            .iter()
            .any(|model| model_id.contains(model))
        {
            return true;
        }
        signed
    }

    /// The Converse request for `request` (TS `_formatRequest`); `tool_choice` replaces the
    /// request's own.
    pub(crate) fn format_request(
        &self,
        request: &ModelRequest,
        tool_choice: Option<&ToolChoice>,
    ) -> Result<JsonObject> {
        let mut out = JsonObject::new();
        out.insert("modelId".to_owned(), JsonValue::from(self.model_id()));
        let messages =
            self.format_messages(&request.messages, request.dynamic_trailing_blocks.unwrap_or(0))?;
        out.insert("messages".to_owned(), JsonValue::Array(messages));

        let mut system = self.format_system(request.system_prompt.as_ref())?;
        if let Some(system) = system.as_mut()
            && self.should_cache_system(system)
        {
            system.push(cache_point(None));
        }

        let tool_config = self.format_tool_config(request, tool_choice)?;
        if let Some(system) = system.as_mut() {
            self.apply_system_cache_ttl(system, tool_config.as_ref());
            out.insert("system".to_owned(), JsonValue::Array(std::mem::take(system)));
        }
        if let Some(tool_config) = tool_config {
            out.insert("toolConfig".to_owned(), tool_config);
        }

        let inference = self.inference_config();
        if !inference.is_empty() {
            out.insert("inferenceConfig".to_owned(), JsonValue::Object(inference));
        }
        if let Some(fields) = self.additional_request_fields(tool_choice) {
            out.insert("additionalModelRequestFields".to_owned(), fields);
        }
        if let Some(paths) = &self.config.additional_response_field_paths {
            out.insert("additionalModelResponseFieldPaths".to_owned(), json!(paths));
        }
        if let Some(JsonValue::Object(args)) = &self.config.additional_args {
            for (key, value) in args {
                out.insert(key.clone(), value.clone());
            }
        }
        if let Some(guardrail) = &self.config.guardrail_config {
            let mut config = JsonObject::new();
            config.insert(
                "guardrailIdentifier".to_owned(),
                json!(guardrail.guardrail_identifier),
            );
            config.insert("guardrailVersion".to_owned(), json!(guardrail.guardrail_version));
            config.insert(
                "trace".to_owned(),
                json!(guardrail.trace.unwrap_or(GuardrailTrace::Enabled).as_str()),
            );
            if let Some(mode) = guardrail.stream_processing_mode {
                config.insert("streamProcessingMode".to_owned(), json!(mode.as_str()));
            }
            out.insert("guardrailConfig".to_owned(), JsonValue::Object(config));
        }
        Ok(out)
    }

    fn format_system(&self, system_prompt: Option<&SystemPrompt>) -> Result<Option<Vec<JsonValue>>> {
        match system_prompt {
            Some(SystemPrompt::Text(text)) => Ok(Some(vec![json!({ "text": text })])),
            Some(SystemPrompt::Blocks(blocks)) if !blocks.is_empty() => blocks
                .iter()
                .map(format_system_block)
                .collect::<Result<Vec<_>>>()
                .map(Some),
            _ => Ok(None),
        }
    }

    fn should_cache_system(&self, system: &[JsonValue]) -> bool {
        if !self.caching_enabled() {
            return false;
        }
        let disabled = self
            .config
            .cache_config
            .as_ref()
            .is_some_and(|c| c.system_prompt_ttl == Some(CacheSection::Enabled(false)));
        !disabled && !system.is_empty() && !system.iter().any(is_cache_point)
    }

    /// Fills unset system cache point TTLs (TS `_applySystemCacheTTL`).
    fn apply_system_cache_ttl(&self, system: &mut [JsonValue], tool_config: Option<&JsonValue>) {
        if !self.caching_enabled() {
            return;
        }
        let cache_config = self.config.cache_config.as_ref();
        let section = cache_config.and_then(|c| c.system_prompt_ttl.as_ref());
        let mut ttl = resolve_cache_section(section, &[cache_config.and_then(|c| c.ttl.as_ref())]).ttl;
        if ttl.is_some() && !matches!(section, Some(CacheSection::Ttl(_))) {
            let tools_point = tool_config
                .and_then(|config| config.get("tools"))
                .and_then(JsonValue::as_array)
                .and_then(|tools| tools.iter().find(|tool| is_cache_point(tool)));
            if let Some(point) = tools_point
                && cache_point_ttl(point) != ttl.as_ref().map(CacheTtl::as_str)
            {
                ttl = None;
            }
        }
        for block in system.iter_mut().filter(|block| is_cache_point(block)) {
            if cache_point_ttl(block).is_none() {
                set_cache_point_ttl(block, ttl.as_ref());
            }
        }
    }

    fn format_tool_config(
        &self,
        request: &ModelRequest,
        tool_choice: Option<&ToolChoice>,
    ) -> Result<Option<JsonValue>> {
        let mut specs = request.tool_specs.clone();
        if specs.is_empty() {
            let has_tool_blocks = request.messages.iter().any(|message| {
                message
                    .content()
                    .iter()
                    .any(|block| matches!(block, ContentBlock::ToolUse(_) | ContentBlock::ToolResult(_)))
            });
            if has_tool_blocks {
                specs.push(noop_tool_spec());
            }
        }
        if specs.is_empty() {
            return Ok(None);
        }
        let strict = self.config.strict_tools == Some(true);
        let mut tools: Vec<JsonValue> = specs.iter().map(|spec| tool_spec_json(spec, strict)).collect();
        let (enabled, ttl) = self.section(|c| c.tools_ttl.as_ref());
        if enabled {
            tools.push(cache_point(ttl.as_ref()));
        }
        let mut config = JsonObject::new();
        config.insert("tools".to_owned(), JsonValue::Array(tools));
        if let Some(tool_choice) = tool_choice {
            config.insert(
                "toolChoice".to_owned(),
                serde_json::to_value(tool_choice).map_err(Error::other)?,
            );
        }
        Ok(Some(JsonValue::Object(config)))
    }

    fn inference_config(&self) -> JsonObject {
        let mut config = JsonObject::new();
        if let Some(max_tokens) = self.resolved_max_tokens {
            config.insert("maxTokens".to_owned(), json!(max_tokens));
        }
        if let Some(temperature) = self.config.temperature {
            config.insert("temperature".to_owned(), json!(temperature));
        }
        if let Some(top_p) = self.config.top_p {
            config.insert("topP".to_owned(), json!(top_p));
        }
        if let Some(stop_sequences) = &self.config.stop_sequences {
            config.insert("stopSequences".to_owned(), json!(stop_sequences));
        }
        config
    }

    /// The configured request fields; `thinking` is dropped when the tool choice forces a tool,
    /// which thinking models reject (TS `_getAdditionalRequestFields`).
    fn additional_request_fields(&self, tool_choice: Option<&ToolChoice>) -> Option<JsonValue> {
        let fields = self.config.additional_request_fields.as_ref()?;
        let Some(object) = fields
            .as_object()
            .filter(|object| object.contains_key("thinking"))
        else {
            return Some(fields.clone());
        };
        if matches!(tool_choice, None | Some(ToolChoice::Auto)) {
            if self.drop_stale_thinking {
                return Some(
                    fields_with_drop_block(Some(fields)).map_or_else(|| fields.clone(), JsonValue::Object),
                );
            }
            return Some(fields.clone());
        }
        let mut rest = object.clone();
        rest.shift_remove("thinking");
        (!rest.is_empty()).then_some(JsonValue::Object(rest))
    }

    /// TS `_formatMessages`.
    fn format_messages(
        &self,
        messages: &[Message],
        dynamic_trailing_blocks: usize,
    ) -> Result<Vec<JsonValue>> {
        let guard_index = self
            .config
            .guardrail_config
            .as_ref()
            .filter(|guardrail| guardrail.guard_latest_user_message == Some(true))
            .and_then(|_| last_user_text_message_index(messages));
        let mut dropped_reasoning = false;
        let mut formatted: Vec<(Role, Vec<JsonValue>)> = Vec::new();
        for (index, message) in messages.iter().enumerate() {
            let mut content = Vec::new();
            for block in message.content() {
                if let ContentBlock::Reasoning(reasoning) = block
                    && self.rejects_reasoning_block(reasoning)
                {
                    dropped_reasoning = true;
                    continue;
                }
                let Some(mut block) = self.format_content_block(block)? else {
                    continue;
                };
                if guard_index == Some(index) {
                    block = apply_guard_block(block);
                }
                content.push(block);
            }
            if !content.is_empty() {
                formatted.push((message.role(), content));
            }
        }
        if dropped_reasoning {
            tracing::debug!(
                "model_id=<{}> | dropped reasoningContent blocks the model does not accept",
                self.model_id()
            );
        }
        let (enabled, ttl) = self.section(|c| c.messages_ttl.as_ref());
        if enabled {
            inject_cache_point(&mut formatted, ttl.as_ref(), dynamic_trailing_blocks);
        }
        Ok(formatted
            .into_iter()
            .map(|(role, content)| json!({ "role": role.as_str(), "content": content }))
            .collect())
    }

    /// TS `_formatContentBlock`; `None` skips the block.
    fn format_content_block(&self, block: &ContentBlock) -> Result<Option<JsonValue>> {
        let formatted = match block {
            ContentBlock::Text(text) => json!({ "text": text.text() }),
            ContentBlock::ToolUse(tool_use) => json!({
                "toolUse": { "toolUseId": tool_use.tool_use_id(), "name": tool_use.name(), "input": tool_use.input() }
            }),
            ContentBlock::ToolResult(result) => self.format_tool_result(result)?,
            ContentBlock::Reasoning(reasoning) => format_reasoning(reasoning)?,
            ContentBlock::CachePoint(point) => {
                let mut cache_point = JsonObject::new();
                cache_point.insert("type".to_owned(), json!(point.cache_type().as_str()));
                if let Some(ttl) = point.ttl().filter(|ttl| !ttl.is_empty()) {
                    cache_point.insert("ttl".to_owned(), json!(ttl));
                }
                json!({ "cachePoint": cache_point })
            }
            ContentBlock::Audio(audio) => {
                json!({ "audio": { "format": audio.format().as_str(), "source": audio_source(audio.source()) } })
            }
            ContentBlock::Image(image) => json!({ "image": format_image(image) }),
            ContentBlock::Video(video) => json!({ "video": format_video(video) }),
            ContentBlock::Document(document) => json!({ "document": format_document(document) }),
            ContentBlock::Citations(citations) => format_citations(citations),
            ContentBlock::GuardContent(guard) => format_guard_content(guard),
            ContentBlock::Injected(_) => {
                warn_once("block_type=<injectedBlock> | bedrock does not send injected blocks | skipping it");
                return Ok(None);
            }
            ContentBlock::Summary(_) => {
                return Err(Error::other("content_type=<summaryBlock> | unsupported type"));
            }
            ContentBlock::Unknown(_) => return Ok(None),
        };
        Ok(Some(formatted))
    }

    fn format_tool_result(&self, result: &ToolResultBlock) -> Result<JsonValue> {
        let mut content = Vec::new();
        for item in result.content() {
            let formatted = match item {
                ToolResultContent::Text(text) => json!({ "text": text.text() }),
                ToolResultContent::Json(block) => json!({ "json": block.json() }),
                ToolResultContent::Image(image) => json!({ "image": format_image(image) }),
                ToolResultContent::Video(video) => json!({ "video": format_video(video) }),
                ToolResultContent::Document(document) => json!({ "document": format_document(document) }),
                _ => continue,
            };
            content.push(formatted);
        }
        let mut tool_result = JsonObject::new();
        tool_result.insert("toolUseId".to_owned(), json!(result.tool_use_id()));
        tool_result.insert("content".to_owned(), JsonValue::Array(content));
        if self.include_tool_result_status() {
            tool_result.insert("status".to_owned(), json!(result.status().as_str()));
        }
        Ok(json!({ "toolResult": tool_result }))
    }
}

fn tool_spec_json(spec: &ToolSpec, strict: bool) -> JsonValue {
    let schema = match spec.input_schema() {
        Some(schema) if strict => Some(to_strict_input_schema(spec.name(), schema)),
        schema => schema.cloned(),
    };
    let mut tool_spec = JsonObject::new();
    tool_spec.insert("name".to_owned(), json!(spec.name()));
    tool_spec.insert("description".to_owned(), json!(spec.description()));
    let mut input_schema = JsonObject::new();
    if let Some(schema) = schema {
        input_schema.insert("json".to_owned(), schema);
    }
    tool_spec.insert("inputSchema".to_owned(), JsonValue::Object(input_schema));
    if strict {
        tool_spec.insert("strict".to_owned(), JsonValue::Bool(true));
    }
    json!({ "toolSpec": tool_spec })
}

fn to_strict_input_schema(tool_name: &str, input_schema: &JsonValue) -> JsonValue {
    let strict = ensure_strict_json_schema(input_schema);
    let unsupported = find_unsupported_strict_keywords(&strict);
    if !unsupported.is_empty() {
        warn_once(&format!(
            "tool=<{tool_name}>, keywords=<{}> | tool schema uses keywords bedrock strict mode rejects, request may fail",
            unsupported.join(",")
        ));
    }
    strict
}

fn format_system_block(block: &SystemContentBlock) -> Result<JsonValue> {
    Ok(match block {
        SystemContentBlock::Text(text) => json!({ "text": text.text() }),
        SystemContentBlock::CachePoint(point) => {
            let mut cache_point = JsonObject::new();
            cache_point.insert("type".to_owned(), json!(point.cache_type().as_str()));
            if let Some(ttl) = point.ttl().filter(|ttl| !ttl.is_empty()) {
                cache_point.insert("ttl".to_owned(), json!(ttl));
            }
            json!({ "cachePoint": cache_point })
        }
        SystemContentBlock::GuardContent(guard) => format_guard_content(guard),
    })
}

fn format_reasoning(block: &ReasoningBlock) -> Result<JsonValue> {
    if block.text().is_some() || block.signature().is_some() {
        let mut reasoning_text = JsonObject::new();
        reasoning_text.insert("text".to_owned(), json!(block.text().unwrap_or_default()));
        if let Some(signature) = block.signature() {
            reasoning_text.insert("signature".to_owned(), json!(signature));
        }
        return Ok(json!({ "reasoningContent": { "reasoningText": reasoning_text } }));
    }
    if let Some(redacted) = block.redacted_content() {
        return Ok(json!({ "reasoningContent": { "redactedContent": encode_base64(redacted) } }));
    }
    Err(Error::other(
        "reasoning content requires one of 'text', 'signature', or 'redactedContent' to be set",
    ))
}

fn format_guard_content(guard: &GuardContentBlock) -> JsonValue {
    match (guard.text(), guard.image()) {
        (Some(text), _) => {
            let qualifiers: Vec<&str> = text
                .qualifiers()
                .iter()
                .map(crate::types::messages::GuardQualifier::as_str)
                .collect();
            json!({ "guardContent": { "text": { "text": text.text(), "qualifiers": qualifiers } } })
        }
        (None, Some(image)) => json!({
            "guardContent": {
                "image": { "format": image.format().as_str(), "source": { "bytes": encode_base64(image.source().bytes()) } }
            }
        }),
        (None, None) => json!({ "guardContent": {} }),
    }
}

fn s3_location(location: &S3Location) -> JsonValue {
    let mut out = JsonObject::new();
    out.insert("uri".to_owned(), json!(location.uri()));
    if let Some(owner) = location.bucket_owner().filter(|owner| !owner.is_empty()) {
        out.insert("bucketOwner".to_owned(), json!(owner));
    }
    json!({ "s3Location": out })
}

fn audio_source(source: &AudioSource) -> JsonValue {
    match source {
        AudioSource::Bytes(bytes) => json!({ "bytes": encode_base64(bytes) }),
        AudioSource::Location(location) => s3_location(location),
    }
}

fn video_source(source: &VideoSource) -> JsonValue {
    match source {
        VideoSource::Bytes(bytes) => json!({ "bytes": encode_base64(bytes) }),
        VideoSource::Location(location) => s3_location(location),
    }
}

fn image_source(source: &ImageSource) -> Option<JsonValue> {
    match source {
        ImageSource::Bytes(bytes) => Some(json!({ "bytes": encode_base64(bytes) })),
        ImageSource::Location(location) => Some(s3_location(location)),
        ImageSource::Url(url) if url.starts_with("s3://") => Some(json!({ "s3Location": { "uri": url } })),
        ImageSource::Url(_) => {
            tracing::warn!("source_type=<imageSourceUrl> | not supported by bedrock | skipping");
            None
        }
    }
}

fn format_image(image: &ImageBlock) -> JsonValue {
    let mut out = JsonObject::new();
    out.insert("format".to_owned(), json!(image.format().as_str()));
    if let Some(source) = image_source(image.source()) {
        out.insert("source".to_owned(), source);
    }
    JsonValue::Object(out)
}

fn format_video(video: &VideoBlock) -> JsonValue {
    let format = match video.format().as_str() {
        "3gp" => "three_gp",
        other => other,
    };
    json!({ "format": format, "source": video_source(video.source()) })
}

fn format_document(document: &DocumentBlock) -> JsonValue {
    let source = match document.source() {
        DocumentSource::Bytes(bytes) => json!({ "bytes": encode_base64(bytes) }),
        DocumentSource::Text(text) => json!({ "bytes": encode_base64(text.as_bytes()) }),
        DocumentSource::Content(blocks) => {
            json!({ "content": blocks.iter().map(|block| json!({ "text": block.text() })).collect::<Vec<_>>() })
        }
        DocumentSource::Location(location) => s3_location(location),
    };
    let mut out = JsonObject::new();
    out.insert("name".to_owned(), json!(document.name()));
    out.insert("format".to_owned(), json!(document.format().as_str()));
    out.insert("source".to_owned(), source);
    if let Some(citations) = document.citations() {
        out.insert("citations".to_owned(), json!({ "enabled": citations.enabled() }));
    }
    if let Some(context) = document.context().filter(|context| !context.is_empty()) {
        out.insert("context".to_owned(), json!(context));
    }
    JsonValue::Object(out)
}

fn format_citations(block: &CitationsBlock) -> JsonValue {
    let citations: Vec<JsonValue> = block.citations().iter().map(citation_to_bedrock).collect();
    let content: Vec<JsonValue> = block
        .content()
        .iter()
        .map(|content| json!({ "text": content.text() }))
        .collect();
    json!({ "citationsContent": { "citations": citations, "content": content } })
}

fn citation_to_bedrock(citation: &Citation) -> JsonValue {
    let mut out = JsonObject::new();
    out.insert(
        "location".to_owned(),
        citation_location_to_bedrock(citation.location()),
    );
    out.insert(
        "sourceContent".to_owned(),
        JsonValue::Array(
            citation
                .source_content()
                .iter()
                .map(|content| json!({ "text": content.text() }))
                .collect(),
        ),
    );
    // Bedrock rejects `source` on document locations and requires it on search results; an
    // empty source marks one Bedrock never returned.
    if !citation.source().is_empty() {
        out.insert("source".to_owned(), json!(citation.source()));
    }
    out.insert("title".to_owned(), json!(citation.title()));
    JsonValue::Object(out)
}

fn citation_location_to_bedrock(location: &CitationLocation) -> JsonValue {
    match location {
        CitationLocation::DocumentChar {
            document_index,
            start,
            end,
        } => json!({ "documentChar": { "documentIndex": document_index, "start": start, "end": end } }),
        CitationLocation::DocumentPage {
            document_index,
            start,
            end,
        } => json!({ "documentPage": { "documentIndex": document_index, "start": start, "end": end } }),
        CitationLocation::DocumentChunk {
            document_index,
            start,
            end,
        } => json!({ "documentChunk": { "documentIndex": document_index, "start": start, "end": end } }),
        CitationLocation::SearchResult {
            search_result_index,
            start,
            end,
        } => json!({
            "searchResultLocation": { "searchResultIndex": search_result_index, "start": start, "end": end }
        }),
        CitationLocation::Web { url, domain } => {
            let mut web = JsonObject::new();
            web.insert("url".to_owned(), json!(url));
            if let Some(domain) = domain.as_deref().filter(|domain| !domain.is_empty()) {
                web.insert("domain".to_owned(), json!(domain));
            }
            json!({ "web": web })
        }
        CitationLocation::Other(other) => JsonValue::Object(other.as_object().clone()),
    }
}

/// Wraps text and supported images in `guardContent` (TS `_applyGuardBlocks`).
fn apply_guard_block(block: JsonValue) -> JsonValue {
    if let Some(text) = block.get("text") {
        return json!({ "guardContent": { "text": { "text": text } } });
    }
    let Some(image) = block.get("image") else {
        return block;
    };
    let (Some(format), Some(source)) = (
        image.get("format").and_then(JsonValue::as_str),
        image.get("source"),
    ) else {
        return block;
    };
    if format != "png" && format != "jpeg" {
        tracing::warn!(
            "image_format=<{format}> | format not supported by bedrock guardrails | skipping guardContent wrap"
        );
        return block;
    }
    if source.get("bytes").is_none() {
        tracing::warn!(
            "source_type=<non-bytes> | image source must be bytes for bedrock guardrails | skipping guardContent wrap"
        );
        return block;
    }
    json!({ "guardContent": { "image": { "format": format, "source": source } } })
}

/// The last user message with text or image content (TS `_findLastUserTextMessageIndex`).
fn last_user_text_message_index(messages: &[Message]) -> Option<usize> {
    messages.iter().rposition(|message| {
        message.role() == Role::User
            && message
                .content()
                .iter()
                .any(|block| matches!(block, ContentBlock::Text(_) | ContentBlock::Image(_)))
    })
}

/// Keeps exactly one cache point in the last user message and strips the rest (TS
/// `_injectCachePoint`).
fn inject_cache_point(
    messages: &mut [(Role, Vec<JsonValue>)],
    ttl: Option<&CacheTtl>,
    dynamic_trailing_blocks: usize,
) {
    let last_user = messages.iter().rposition(|(role, _)| *role == Role::User);
    for (msg_idx, (_, content)) in messages.iter_mut().enumerate() {
        if Some(msg_idx) == last_user {
            continue;
        }
        for block_idx in (0..content.len()).rev() {
            if content.get(block_idx).is_some_and(is_cache_point) {
                content.remove(block_idx);
                tracing::warn!(
                    "msg_idx=<{msg_idx}>, block_idx=<{block_idx}> | stripped existing cache point (auto mode manages cache points)"
                );
            }
        }
    }
    let Some(msg_idx) = last_user else {
        return;
    };
    let Some((_, content)) = messages.get_mut(msg_idx) else {
        return;
    };
    let placed: Vec<usize> = content
        .iter()
        .enumerate()
        .filter(|(_, block)| is_cache_point(block))
        .map(|(idx, _)| idx)
        .collect();
    if let Some(&first) = placed.first() {
        for &extra in placed.iter().skip(1).rev() {
            content.remove(extra);
            tracing::warn!(
                "msg_idx=<{msg_idx}>, block_idx=<{extra}> | stripped existing cache point (auto mode manages cache points)"
            );
        }
        if honor_placed_cache_point(content, first, msg_idx, ttl) {
            return;
        }
        if content.is_empty() {
            tracing::debug!("msg_idx=<{msg_idx}> | no content left to cache");
            return;
        }
    }
    if dynamic_trailing_blocks > 0 {
        let boundary = content.len().saturating_sub(dynamic_trailing_blocks);
        if boundary == 0 {
            tracing::debug!("msg_idx=<{msg_idx}> | skipped cache point, no durable prefix");
            return;
        }
        content.insert(boundary, cache_point(ttl));
        honor_placed_cache_point(content, boundary, msg_idx, ttl);
        return;
    }
    match content.iter().position(is_non_pdf_document) {
        None => content.push(cache_point(ttl)),
        Some(0) => {
            tracing::debug!("msg_idx=<{msg_idx}> | skipped cache point for leading non-PDF document");
            return;
        }
        Some(idx) => content.insert(idx, cache_point(ttl)),
    }
    tracing::debug!("msg_idx=<{msg_idx}> | added cache point to last user message");
}

/// Keeps a caller-placed cache point where it sits, with the configured TTL, stepping back over
/// directly preceding non-PDF documents; false when it was removed (TS `_honorPlacedCachePoint`).
fn honor_placed_cache_point(
    content: &mut Vec<JsonValue>,
    placed_idx: usize,
    msg_idx: usize,
    ttl: Option<&CacheTtl>,
) -> bool {
    if !content.get(placed_idx).is_some_and(is_cache_point) {
        return false;
    }
    if placed_idx == 0 {
        content.remove(0);
        tracing::warn!(
            "msg_idx=<{msg_idx}> | removed cache point with no content ahead of it, falling back to automatic placement"
        );
        return false;
    }
    if let Some(placed) = content.get_mut(placed_idx) {
        set_cache_point_ttl(placed, ttl);
    }
    let mut target = placed_idx;
    while target > 0 && content.get(target - 1).is_some_and(is_non_pdf_document) {
        target -= 1;
    }
    if target != placed_idx {
        let placed = content.remove(placed_idx);
        if target == 0 {
            tracing::warn!("msg_idx=<{msg_idx}> | dropped cache point ahead of a leading document");
            return false;
        }
        content.insert(target, placed);
        tracing::debug!("msg_idx=<{msg_idx}>, block_idx=<{target}> | relocated caller-placed cache point");
        return true;
    }
    tracing::debug!("msg_idx=<{msg_idx}>, block_idx=<{placed_idx}> | honored caller-placed cache point");
    true
}
