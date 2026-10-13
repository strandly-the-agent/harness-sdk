//! Converse request formatting (`_formatRequest` and friends in `models/bedrock.ts`).

use aws_sdk_bedrockruntime::types as bt;
use aws_smithy_types::{Blob, Document, Number};
use serde_json::{Map, Value};

use super::{
    model_error, resolve_cache_section, BedrockModelConfig, CacheSection, CacheStrategy, GuardrailStreamProcessingMode,
    GuardrailTraceMode, IncludeToolResultStatus, MODELS_INCLUDE_STATUS, MODELS_SUPPORTING_ANTHROPIC_CACHING,
};
use crate::error::{Error, Result};
use crate::models::defaults::MODEL_DEFAULTS;
use crate::models::model::StreamOptions;
use crate::types::{
    AudioSource, Citation, CitationLocation, CitationsBlock, ContentBlock, DocumentBlock, DocumentSource,
    GuardContentBlock, GuardImageFormat, GuardQualifier, ImageBlock, ImageSource, JsonSchema, JsonValue, Message,
    Role, SystemContentBlock, SystemPrompt, ToolChoice, ToolResultBlock, ToolResultContent, ToolResultStatus,
    ToolSpec, ToolUseBlock, VideoBlock, VideoSource,
};

/// Placeholder tool sent when the history references tools but the caller supplied no specs.
fn noop_tool_spec() -> ToolSpec {
    ToolSpec {
        name: "noop".to_owned(),
        description: "This is a fake tool that MUST be completely ignored.".to_owned(),
        input_schema: Some(serde_json::json!({ "type": "object", "properties": {} })),
        output_schema: None,
        annotations: None,
    }
}

/// Guardrail settings resolved for a request; `Converse` and `ConverseStream` take different shapes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedGuardrailConfig {
    /// Guardrail id.
    pub identifier: String,
    /// Guardrail version.
    pub version: String,
    /// Trace level.
    pub trace: GuardrailTraceMode,
    /// Stream processing mode, when set.
    pub stream_processing_mode: Option<GuardrailStreamProcessingMode>,
}

impl ResolvedGuardrailConfig {
    fn trace(&self) -> bt::GuardrailTrace {
        match self.trace {
            GuardrailTraceMode::Enabled => bt::GuardrailTrace::Enabled,
            GuardrailTraceMode::Disabled => bt::GuardrailTrace::Disabled,
            GuardrailTraceMode::EnabledFull => bt::GuardrailTrace::from("enabled_full"),
        }
    }

    pub(crate) fn to_converse(&self) -> Result<bt::GuardrailConfiguration> {
        Ok(bt::GuardrailConfiguration::builder()
            .guardrail_identifier(&self.identifier)
            .guardrail_version(&self.version)
            .trace(self.trace())
            .build())
    }

    pub(crate) fn to_stream(&self) -> Result<bt::GuardrailStreamConfiguration> {
        let mut builder = bt::GuardrailStreamConfiguration::builder()
            .guardrail_identifier(&self.identifier)
            .guardrail_version(&self.version)
            .trace(self.trace());
        if let Some(mode) = self.stream_processing_mode {
            builder = builder.stream_processing_mode(match mode {
                GuardrailStreamProcessingMode::Sync => bt::GuardrailStreamProcessingMode::Sync,
                GuardrailStreamProcessingMode::Async => bt::GuardrailStreamProcessingMode::Async,
            });
        }
        Ok(builder.build())
    }
}

/// A formatted Converse request, applied to either the streaming or non-streaming operation.
#[derive(Clone, Debug, PartialEq)]
pub struct FormattedRequest {
    /// Model id.
    pub model_id: String,
    /// Conversation messages.
    pub messages: Vec<bt::Message>,
    /// System blocks; `None` when no system prompt was given.
    pub system: Option<Vec<bt::SystemContentBlock>>,
    /// Tool configuration; `None` when there are no tools.
    pub tool_config: Option<bt::ToolConfiguration>,
    /// Inference configuration; `None` when no inference field is set.
    pub inference_config: Option<bt::InferenceConfiguration>,
    /// `additionalModelRequestFields`.
    pub additional_model_request_fields: Option<Document>,
    /// `additionalModelResponseFieldPaths`.
    pub additional_model_response_field_paths: Option<Vec<String>>,
    /// Guardrail settings.
    pub guardrail: Option<ResolvedGuardrailConfig>,
}

/// Format a Converse request exactly as the TS provider does.
pub(crate) fn format_request(
    config: &BedrockModelConfig,
    messages: &[Message],
    options: &StreamOptions,
) -> Result<FormattedRequest> {
    let model_id = config
        .model_id
        .clone()
        .unwrap_or_else(|| MODEL_DEFAULTS.bedrock.model_id.to_owned());

    let formatted_messages = format_messages(config, messages, options.dynamic_trailing_blocks.unwrap_or(0))?;

    let mut system: Option<Vec<bt::SystemContentBlock>> = match &options.system_prompt {
        Some(SystemPrompt::Text(text)) => Some(vec![bt::SystemContentBlock::Text(text.clone())]),
        Some(SystemPrompt::Blocks(blocks)) if !blocks.is_empty() => Some(
            blocks
                .iter()
                .map(format_system_content_block)
                .collect::<Result<Vec<_>>>()?,
        ),
        Some(SystemPrompt::Blocks(_)) | None => None,
    };

    if should_cache_system(config, system.as_deref()) {
        system
            .as_mut()
            .expect("checked by should_cache_system")
            .push(bt::SystemContentBlock::CachePoint(cache_point(None)?));
    }

    let mut tool_specs: Vec<ToolSpec> = options.tool_specs.clone();
    if tool_specs.is_empty() {
        let has_tool_blocks = messages.iter().any(|msg| {
            msg.content
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolUse(_) | ContentBlock::ToolResult(_)))
        });
        if has_tool_blocks {
            tool_specs = vec![noop_tool_spec()];
        }
    }

    let mut tool_config = None;
    if !tool_specs.is_empty() {
        let strict_tools = config.strict_tools == Some(true);
        let mut tools: Vec<bt::Tool> = Vec::with_capacity(tool_specs.len() + 1);
        for spec in &tool_specs {
            let schema = match (&spec.input_schema, strict_tools) {
                (Some(schema), true) => Some(to_strict_input_schema(&spec.name, schema)),
                (schema, _) => schema.clone(),
            };
            let mut tool_spec = bt::ToolSpecification::builder()
                .name(&spec.name)
                .description(&spec.description)
                .set_input_schema(
                    schema
                        .map(|schema| Ok::<_, Error>(bt::ToolInputSchema::Json(json_to_document(&schema)?)))
                        .transpose()?,
                );
            if strict_tools {
                tool_spec = tool_spec.strict(true);
            }
            tools.push(bt::Tool::ToolSpec(
                tool_spec
                    .build()
                    .map_err(|e| model_error("invalid tool specification", e))?,
            ));
        }

        let tools_cache = if should_enable_caching(config) {
            let cache_config = config.cache_config.as_ref();
            resolve_cache_section(
                cache_config.and_then(|c| c.tools_ttl.as_ref()),
                &[cache_config.and_then(|c| c.ttl.as_deref())],
            )
        } else {
            Default::default()
        };
        if tools_cache.enabled {
            tools.push(bt::Tool::CachePoint(cache_point(tools_cache.ttl.as_deref())?));
        }

        let mut builder = bt::ToolConfiguration::builder().set_tools(Some(tools));
        if let Some(tool_choice) = &options.tool_choice {
            builder = builder.tool_choice(format_tool_choice(tool_choice)?);
        }
        tool_config = Some(
            builder
                .build()
                .map_err(|e| model_error("invalid tool configuration", e))?,
        );
    }

    if let Some(system) = system.as_mut() {
        apply_system_cache_ttl(config, system, tool_config.as_ref())?;
    }

    let inference_config = format_inference_config(config)?;

    let additional_model_request_fields = additional_request_fields(config, options)
        .map(|fields| json_to_document(&fields))
        .transpose()?;

    if config.additional_args.is_some() {
        crate::logging::warn_once(
            "additionalArgs is not supported by the Rust Bedrock provider | the typed Converse request cannot be overridden",
        );
    }

    let guardrail = config
        .guardrail_config
        .as_ref()
        .map(|guardrail| ResolvedGuardrailConfig {
            identifier: guardrail.guardrail_identifier.clone(),
            version: guardrail.guardrail_version.clone(),
            trace: guardrail.trace.unwrap_or_default(),
            stream_processing_mode: guardrail.stream_processing_mode,
        });

    Ok(FormattedRequest {
        model_id,
        messages: formatted_messages,
        system,
        tool_config,
        inference_config,
        additional_model_request_fields,
        additional_model_response_field_paths: config.additional_response_field_paths.clone(),
        guardrail,
    })
}

fn format_inference_config(config: &BedrockModelConfig) -> Result<Option<bt::InferenceConfiguration>> {
    if config.max_tokens.is_none()
        && config.temperature.is_none()
        && config.top_p.is_none()
        && config.stop_sequences.is_none()
    {
        return Ok(None);
    }
    Ok(Some(
        bt::InferenceConfiguration::builder()
            .set_max_tokens(config.max_tokens)
            .set_temperature(config.temperature)
            .set_top_p(config.top_p)
            .set_stop_sequences(config.stop_sequences.clone())
            .build(),
    ))
}

fn format_tool_choice(tool_choice: &ToolChoice) -> Result<bt::ToolChoice> {
    Ok(match tool_choice {
        ToolChoice::Auto {} => bt::ToolChoice::Auto(bt::AutoToolChoice::builder().build()),
        ToolChoice::Any {} => bt::ToolChoice::Any(bt::AnyToolChoice::builder().build()),
        ToolChoice::Tool { name } => bt::ToolChoice::Tool(
            bt::SpecificToolChoice::builder()
                .name(name)
                .build()
                .map_err(|e| model_error("invalid tool choice", e))?,
        ),
    })
}

/// `additionalRequestFields`, with `thinking` stripped when tool choice forces a tool call.
fn additional_request_fields(config: &BedrockModelConfig, options: &StreamOptions) -> Option<JsonValue> {
    let fields = config.additional_request_fields.clone()?;
    let has_thinking = fields.get("thinking").is_some();
    if !has_thinking {
        return Some(fields);
    }
    match &options.tool_choice {
        None | Some(ToolChoice::Auto {}) => Some(fields),
        Some(_) => {
            let mut object = fields.as_object().cloned().unwrap_or_default();
            object.remove("thinking");
            if object.is_empty() {
                None
            } else {
                Some(Value::Object(object))
            }
        }
    }
}

fn cache_point(ttl: Option<&str>) -> Result<bt::CachePointBlock> {
    let mut builder = bt::CachePointBlock::builder().r#type(bt::CachePointType::Default);
    if let Some(ttl) = ttl.filter(|ttl| !ttl.is_empty()) {
        builder = builder.ttl(bt::CacheTtl::from(ttl));
    }
    builder
        .build()
        .map_err(|e| model_error("invalid cache point block", e))
}

fn cache_strategy(config: &BedrockModelConfig) -> Option<CacheStrategy> {
    let model_id = config.model_id.as_deref().unwrap_or_default();
    MODELS_SUPPORTING_ANTHROPIC_CACHING
        .iter()
        .any(|pattern| model_id.contains(pattern))
        .then_some(CacheStrategy::Anthropic)
}

/// Whether prompt caching applies to this model/config (`_shouldEnableCaching`).
pub(crate) fn should_enable_caching(config: &BedrockModelConfig) -> bool {
    let Some(cache_config) = config.cache_config.as_ref() else {
        return false;
    };
    match cache_config.strategy.unwrap_or_default() {
        CacheStrategy::Anthropic => true,
        CacheStrategy::Auto => {
            if cache_strategy(config).is_none() {
                tracing::warn!(
                    "model_id=<{}> | cache_config is enabled but this model does not support automatic caching",
                    config.model_id.as_deref().unwrap_or_default()
                );
                return false;
            }
            true
        }
    }
}

fn should_cache_system(config: &BedrockModelConfig, system: Option<&[bt::SystemContentBlock]>) -> bool {
    if !should_enable_caching(config) {
        return false;
    }
    if matches!(
        config.cache_config.as_ref().and_then(|c| c.system_prompt_ttl.as_ref()),
        Some(CacheSection::Disabled)
    ) {
        return false;
    }
    match system {
        None | Some([]) => false,
        Some(blocks) => !blocks
            .iter()
            .any(|block| matches!(block, bt::SystemContentBlock::CachePoint(_))),
    }
}

/// Fill the resolved system TTL into system cache points that carry none of their own.
fn apply_system_cache_ttl(
    config: &BedrockModelConfig,
    system: &mut [bt::SystemContentBlock],
    tool_config: Option<&bt::ToolConfiguration>,
) -> Result<()> {
    if !should_enable_caching(config) {
        return Ok(());
    }
    let cache_config = config.cache_config.as_ref();
    let section = resolve_cache_section(
        cache_config.and_then(|c| c.system_prompt_ttl.as_ref()),
        &[cache_config.and_then(|c| c.ttl.as_deref())],
    );
    let mut ttl = section.ttl;

    let explicit_system_ttl = matches!(
        cache_config.and_then(|c| c.system_prompt_ttl.as_ref()),
        Some(CacheSection::Ttl(_))
    );
    if ttl.is_some() && !explicit_system_ttl {
        let tools_point_ttl = tool_config
            .and_then(|tools| tools.tools.iter().find_map(|tool| tool.as_cache_point().ok()))
            .map(|point| point.ttl().map(|ttl| ttl.as_str().to_owned()));
        if let Some(tools_ttl) = tools_point_ttl {
            if tools_ttl != ttl {
                ttl = None;
            }
        }
    }

    for block in system.iter_mut() {
        if let bt::SystemContentBlock::CachePoint(point) = block {
            if point.ttl().is_none() {
                *point = cache_point(ttl.as_deref())?;
            }
        }
    }
    Ok(())
}

/// Format messages and inject the conversation cache point (`_formatMessages`).
fn format_messages(
    config: &BedrockModelConfig,
    messages: &[Message],
    dynamic_trailing_blocks: usize,
) -> Result<Vec<bt::Message>> {
    let last_user_text_idx = if config
        .guardrail_config
        .as_ref()
        .and_then(|g| g.guard_latest_user_message)
        == Some(true)
    {
        find_last_user_text_message_index(messages)
    } else {
        None
    };

    let mut formatted: Vec<bt::Message> = Vec::with_capacity(messages.len());
    for (idx, message) in messages.iter().enumerate() {
        let apply_guard_blocks = Some(idx) == last_user_text_idx;
        let mut content = Vec::with_capacity(message.content.len());
        for block in &message.content {
            let formatted_block = format_content_block(config, block)?;
            let formatted_block = if apply_guard_blocks {
                apply_guard_block(formatted_block)?
            } else {
                formatted_block
            };
            if let Some(block) = formatted_block {
                content.push(block);
            }
        }
        if content.is_empty() {
            continue;
        }
        formatted.push(
            bt::Message::builder()
                .role(match message.role {
                    Role::User => bt::ConversationRole::User,
                    Role::Assistant => bt::ConversationRole::Assistant,
                })
                .set_content(Some(content))
                .build()
                .map_err(|e| model_error("invalid message", e))?,
        );
    }

    let messages_cache = if should_enable_caching(config) {
        let cache_config = config.cache_config.as_ref();
        resolve_cache_section(
            cache_config.and_then(|c| c.messages_ttl.as_ref()),
            &[cache_config.and_then(|c| c.ttl.as_deref())],
        )
    } else {
        Default::default()
    };
    if messages_cache.enabled {
        inject_cache_point(
            &mut formatted,
            messages_cache.ttl.as_deref(),
            dynamic_trailing_blocks,
        )?;
    }

    Ok(formatted)
}

fn find_last_user_text_message_index(messages: &[Message]) -> Option<usize> {
    messages.iter().rposition(|msg| {
        msg.role == Role::User
            && msg
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::Text(_) | ContentBlock::Image(_)))
    })
}

fn is_non_pdf_document(block: &bt::ContentBlock) -> bool {
    matches!(block, bt::ContentBlock::Document(doc) if doc.format() != &bt::DocumentFormat::Pdf)
}

/// Keep a caller-placed cache point, applying the configured TTL and the document rule.
fn honor_placed_cache_point(
    content: &mut Vec<bt::ContentBlock>,
    placed_idx: usize,
    msg_idx: usize,
    ttl: Option<&str>,
) -> Result<bool> {
    if !matches!(content.get(placed_idx), Some(bt::ContentBlock::CachePoint(_))) {
        return Ok(false);
    }

    if placed_idx == 0 {
        content.remove(placed_idx);
        tracing::warn!(
            "msg_idx=<{msg_idx}> | removed cache point with no content ahead of it, falling back to automatic placement"
        );
        return Ok(false);
    }

    content[placed_idx] = bt::ContentBlock::CachePoint(cache_point(ttl)?);

    let mut target_idx = placed_idx;
    while target_idx > 0 {
        match content.get(target_idx - 1) {
            Some(previous) if is_non_pdf_document(previous) => target_idx -= 1,
            _ => break,
        }
    }

    if target_idx != placed_idx {
        let placed = content.remove(placed_idx);
        if target_idx == 0 {
            tracing::warn!("msg_idx=<{msg_idx}> | dropped cache point ahead of a leading document");
            return Ok(false);
        }
        content.insert(target_idx, placed);
        tracing::debug!("msg_idx=<{msg_idx}>, block_idx=<{target_idx}> | relocated caller-placed cache point");
        return Ok(true);
    }

    tracing::debug!("msg_idx=<{msg_idx}>, block_idx=<{placed_idx}> | honored caller-placed cache point");
    Ok(true)
}

/// Ensure the last user message carries exactly one cache point (`_injectCachePoint`).
fn inject_cache_point(
    messages: &mut [bt::Message],
    ttl: Option<&str>,
    dynamic_trailing_blocks: usize,
) -> Result<()> {
    if messages.is_empty() {
        return Ok(());
    }

    let last_user_idx = messages
        .iter()
        .rposition(|msg| msg.role() == &bt::ConversationRole::User);

    for (msg_idx, msg) in messages.iter_mut().enumerate() {
        if Some(msg_idx) == last_user_idx {
            continue;
        }
        for block_idx in (0..msg.content.len()).rev() {
            if matches!(msg.content[block_idx], bt::ContentBlock::CachePoint(_)) {
                msg.content.remove(block_idx);
                tracing::warn!(
                    "msg_idx=<{msg_idx}>, block_idx=<{block_idx}> | stripped existing cache point (auto mode manages cache points)"
                );
            }
        }
    }

    let Some(last_user_idx) = last_user_idx else {
        return Ok(());
    };
    let content = &mut messages[last_user_idx].content;

    let placed_idxs: Vec<usize> = content
        .iter()
        .enumerate()
        .filter(|(_, block)| matches!(block, bt::ContentBlock::CachePoint(_)))
        .map(|(idx, _)| idx)
        .collect();

    if let Some(&first_placed) = placed_idxs.first() {
        for &extra_idx in placed_idxs[1..].iter().rev() {
            content.remove(extra_idx);
            tracing::warn!(
                "msg_idx=<{last_user_idx}>, block_idx=<{extra_idx}> | stripped existing cache point (auto mode manages cache points)"
            );
        }
        if honor_placed_cache_point(content, first_placed, last_user_idx, ttl)? {
            return Ok(());
        }
        if content.is_empty() {
            tracing::debug!("msg_idx=<{last_user_idx}> | no content left to cache");
            return Ok(());
        }
    }

    if dynamic_trailing_blocks > 0 {
        let boundary_idx = content.len().saturating_sub(dynamic_trailing_blocks);
        if boundary_idx == 0 {
            tracing::debug!("msg_idx=<{last_user_idx}> | skipped cache point, no durable prefix");
            return Ok(());
        }
        content.insert(boundary_idx, bt::ContentBlock::CachePoint(cache_point(ttl)?));
        honor_placed_cache_point(content, boundary_idx, last_user_idx, ttl)?;
        return Ok(());
    }

    let first_non_pdf_doc_idx = content.iter().position(is_non_pdf_document);
    match first_non_pdf_doc_idx {
        None => content.push(bt::ContentBlock::CachePoint(cache_point(ttl)?)),
        Some(0) => {
            tracing::debug!("msg_idx=<{last_user_idx}> | skipped cache point for leading non-PDF document");
            return Ok(());
        }
        Some(idx) => content.insert(idx, bt::ContentBlock::CachePoint(cache_point(ttl)?)),
    }

    tracing::debug!("msg_idx=<{last_user_idx}> | added cache point to last user message");
    Ok(())
}

/// Wrap a formatted text/image block in `guardContent` for guardrail evaluation.
fn apply_guard_block(block: Option<bt::ContentBlock>) -> Result<Option<bt::ContentBlock>> {
    let Some(block) = block else { return Ok(None) };

    match &block {
        bt::ContentBlock::Text(text) => Ok(Some(bt::ContentBlock::GuardContent(
            bt::GuardrailConverseContentBlock::Text(
                bt::GuardrailConverseTextBlock::builder()
                    .text(text)
                    .build()
                    .map_err(|e| model_error("invalid guard content", e))?,
            ),
        ))),
        bt::ContentBlock::Image(image) => {
            let format = image.format();
            if format != &bt::ImageFormat::Png && format != &bt::ImageFormat::Jpeg {
                tracing::warn!(
                    "image_format=<{}> | format not supported by bedrock guardrails | skipping guardContent wrap",
                    format.as_str()
                );
                return Ok(Some(block));
            }
            let Some(bt::ImageSource::Bytes(bytes)) = image.source() else {
                tracing::warn!(
                    "source_type=<non-bytes> | image source must be bytes for bedrock guardrails | skipping guardContent wrap"
                );
                return Ok(Some(block));
            };
            let guard_format = if format == &bt::ImageFormat::Png {
                bt::GuardrailConverseImageFormat::Png
            } else {
                bt::GuardrailConverseImageFormat::Jpeg
            };
            Ok(Some(bt::ContentBlock::GuardContent(
                bt::GuardrailConverseContentBlock::Image(
                    bt::GuardrailConverseImageBlock::builder()
                        .format(guard_format)
                        .source(bt::GuardrailConverseImageSource::Bytes(bytes.clone()))
                        .build()
                        .map_err(|e| model_error("invalid guard content", e))?,
                ),
            )))
        }
        _ => Ok(Some(block)),
    }
}

fn should_include_tool_result_status(config: &BedrockModelConfig) -> bool {
    match config.include_tool_result_status.unwrap_or_default() {
        IncludeToolResultStatus::Always => true,
        IncludeToolResultStatus::Never => false,
        IncludeToolResultStatus::Auto => {
            let model_id = config.model_id.as_deref().unwrap_or_default();
            let should_include = MODELS_INCLUDE_STATUS.iter().any(|pattern| model_id.contains(pattern));
            tracing::debug!(
                "model_id=<{model_id}>, include_tool_result_status=<{should_include}> | auto-detected includeToolResultStatus"
            );
            should_include
        }
    }
}

fn format_system_content_block(block: &SystemContentBlock) -> Result<bt::SystemContentBlock> {
    Ok(match block {
        SystemContentBlock::Text(text) => bt::SystemContentBlock::Text(text.clone()),
        SystemContentBlock::CachePoint(point) => {
            bt::SystemContentBlock::CachePoint(cache_point(point.ttl.as_deref())?)
        }
        SystemContentBlock::GuardContent(guard) => bt::SystemContentBlock::GuardContent(format_guard_content(guard)?),
    })
}

fn format_guard_content(guard: &GuardContentBlock) -> Result<bt::GuardrailConverseContentBlock> {
    if let Some(text) = &guard.text {
        let qualifiers: Vec<bt::GuardrailConverseContentQualifier> = text
            .qualifiers
            .iter()
            .map(|qualifier| match qualifier {
                GuardQualifier::GroundingSource => bt::GuardrailConverseContentQualifier::GroundingSource,
                GuardQualifier::Query => bt::GuardrailConverseContentQualifier::Query,
                GuardQualifier::GuardContent => bt::GuardrailConverseContentQualifier::GuardContent,
            })
            .collect();
        return Ok(bt::GuardrailConverseContentBlock::Text(
            bt::GuardrailConverseTextBlock::builder()
                .text(&text.text)
                .set_qualifiers(Some(qualifiers))
                .build()
                .map_err(|e| model_error("invalid guard content", e))?,
        ));
    }
    if let Some(image) = &guard.image {
        return Ok(bt::GuardrailConverseContentBlock::Image(
            bt::GuardrailConverseImageBlock::builder()
                .format(match image.format {
                    GuardImageFormat::Png => bt::GuardrailConverseImageFormat::Png,
                    GuardImageFormat::Jpeg => bt::GuardrailConverseImageFormat::Jpeg,
                })
                .source(bt::GuardrailConverseImageSource::Bytes(Blob::new(
                    image.source.bytes.clone(),
                )))
                .build()
                .map_err(|e| model_error("invalid guard content", e))?,
        ));
    }
    Err(Error::model("guardContent must have either text or image"))
}

/// SDK content block → Converse content block; `None` means the block is dropped.
fn format_content_block(config: &BedrockModelConfig, block: &ContentBlock) -> Result<Option<bt::ContentBlock>> {
    Ok(match block {
        ContentBlock::Text(text) => Some(bt::ContentBlock::Text(text.clone())),
        ContentBlock::ToolUse(tool_use) => Some(bt::ContentBlock::ToolUse(format_tool_use(tool_use)?)),
        ContentBlock::ToolResult(result) => Some(bt::ContentBlock::ToolResult(format_tool_result(config, result)?)),
        ContentBlock::Reasoning(reasoning) => {
            if reasoning.text.is_some() || reasoning.signature.is_some() {
                let mut builder = bt::ReasoningTextBlock::builder().text(reasoning.text.clone().unwrap_or_default());
                if let Some(signature) = &reasoning.signature {
                    builder = builder.signature(signature);
                }
                Some(bt::ContentBlock::ReasoningContent(
                    bt::ReasoningContentBlock::ReasoningText(
                        builder.build().map_err(|e| model_error("invalid reasoning block", e))?,
                    ),
                ))
            } else if let Some(redacted) = reasoning.redacted_content.as_ref().filter(|r| !r.is_empty()) {
                Some(bt::ContentBlock::ReasoningContent(
                    bt::ReasoningContentBlock::RedactedContent(Blob::new(redacted.clone())),
                ))
            } else {
                return Err(Error::model(
                    "reasoning content requires one of 'text', 'signature', or 'redactedContent' to be set",
                ));
            }
        }
        ContentBlock::CachePoint(point) => Some(bt::ContentBlock::CachePoint(cache_point(point.ttl.as_deref())?)),
        ContentBlock::Audio(audio) => {
            let source = match &audio.source {
                AudioSource::Bytes(bytes) => Some(bt::AudioSource::Bytes(Blob::new(bytes.clone()))),
                AudioSource::Location(location) => Some(bt::AudioSource::S3Location(format_s3_location(location)?)),
            };
            Some(bt::ContentBlock::Audio(
                bt::AudioBlock::builder()
                    .format(bt::AudioFormat::from(audio_format_str(&audio.format)))
                    .set_source(source)
                    .build()
                    .map_err(|e| model_error("invalid audio block", e))?,
            ))
        }
        ContentBlock::Image(image) => Some(bt::ContentBlock::Image(format_image(image)?)),
        ContentBlock::Video(video) => Some(bt::ContentBlock::Video(format_video(video)?)),
        ContentBlock::Document(document) => Some(bt::ContentBlock::Document(format_document(document)?)),
        ContentBlock::Citations(citations) => {
            Some(bt::ContentBlock::CitationsContent(format_citations(citations)?))
        }
        ContentBlock::GuardContent(guard) => Some(bt::ContentBlock::GuardContent(format_guard_content(guard)?)),
    })
}

fn format_tool_use(tool_use: &ToolUseBlock) -> Result<bt::ToolUseBlock> {
    bt::ToolUseBlock::builder()
        .tool_use_id(&tool_use.tool_use_id)
        .name(&tool_use.name)
        .input(json_to_document(&tool_use.input)?)
        .build()
        .map_err(|e| model_error("invalid tool use block", e))
}

fn format_tool_result(config: &BedrockModelConfig, result: &ToolResultBlock) -> Result<bt::ToolResultBlock> {
    let mut content = Vec::with_capacity(result.content.len());
    for entry in &result.content {
        content.push(match entry {
            ToolResultContent::Text(text) => bt::ToolResultContentBlock::Text(text.clone()),
            ToolResultContent::Json(json) => bt::ToolResultContentBlock::Json(json_to_document(json)?),
            ToolResultContent::Image(image) => bt::ToolResultContentBlock::Image(format_image(image)?),
            ToolResultContent::Video(video) => bt::ToolResultContentBlock::Video(format_video(video)?),
            ToolResultContent::Document(document) => bt::ToolResultContentBlock::Document(format_document(document)?),
        });
    }
    let mut builder = bt::ToolResultBlock::builder()
        .tool_use_id(&result.tool_use_id)
        .set_content(Some(content));
    if should_include_tool_result_status(config) {
        builder = builder.status(match result.status {
            ToolResultStatus::Success => bt::ToolResultStatus::Success,
            ToolResultStatus::Error => bt::ToolResultStatus::Error,
        });
    }
    builder
        .build()
        .map_err(|e| model_error("invalid tool result block", e))
}

fn format_image(image: &ImageBlock) -> Result<bt::ImageBlock> {
    let source = match &image.source {
        ImageSource::Bytes(bytes) => Some(bt::ImageSource::Bytes(Blob::new(bytes.clone()))),
        ImageSource::Location(location) => Some(bt::ImageSource::S3Location(format_s3_location(location)?)),
        ImageSource::Url(url) => {
            if url.starts_with("s3://") {
                Some(bt::ImageSource::S3Location(
                    bt::S3Location::builder()
                        .uri(url)
                        .build()
                        .map_err(|e| model_error("invalid s3 location", e))?,
                ))
            } else {
                tracing::warn!("source_type=<imageSourceUrl> | not supported by bedrock | skipping");
                None
            }
        }
    };
    bt::ImageBlock::builder()
        .format(bt::ImageFormat::from(image_format_str(&image.format)))
        .set_source(source)
        .build()
        .map_err(|e| model_error("invalid image block", e))
}

fn format_video(video: &VideoBlock) -> Result<bt::VideoBlock> {
    let source = match &video.source {
        VideoSource::Bytes(bytes) => Some(bt::VideoSource::Bytes(Blob::new(bytes.clone()))),
        VideoSource::Location(location) => Some(bt::VideoSource::S3Location(format_s3_location(location)?)),
    };
    bt::VideoBlock::builder()
        .format(bt::VideoFormat::from(video_format_str(&video.format)))
        .set_source(source)
        .build()
        .map_err(|e| model_error("invalid video block", e))
}

fn format_document(document: &DocumentBlock) -> Result<bt::DocumentBlock> {
    let source = match &document.source {
        DocumentSource::Bytes(bytes) => bt::DocumentSource::Bytes(Blob::new(bytes.clone())),
        DocumentSource::Text(text) => bt::DocumentSource::Bytes(Blob::new(text.as_bytes().to_vec())),
        DocumentSource::Content(content) => bt::DocumentSource::Content(
            content
                .iter()
                .map(|entry| bt::DocumentContentBlock::Text(entry.text.clone()))
                .collect(),
        ),
        DocumentSource::Location(location) => bt::DocumentSource::S3Location(format_s3_location(location)?),
    };
    let mut builder = bt::DocumentBlock::builder()
        .name(&document.name)
        .format(bt::DocumentFormat::from(document_format_str(&document.format)))
        .source(source);
    if let Some(citations) = &document.citations {
        builder = builder.citations(
            bt::CitationsConfig::builder()
                .enabled(citations.enabled)
                .build()
                .map_err(|e| model_error("invalid citations config", e))?,
        );
    }
    if let Some(context) = document.context.as_deref().filter(|c| !c.is_empty()) {
        builder = builder.context(context);
    }
    builder
        .build()
        .map_err(|e| model_error("invalid document block", e))
}

fn format_s3_location(location: &crate::types::S3Location) -> Result<bt::S3Location> {
    let mut builder = bt::S3Location::builder().uri(&location.uri);
    if let Some(owner) = location.bucket_owner.as_deref().filter(|o| !o.is_empty()) {
        builder = builder.bucket_owner(owner);
    }
    builder
        .build()
        .map_err(|e| model_error("invalid s3 location", e))
}

fn format_citations(citations: &CitationsBlock) -> Result<bt::CitationsContentBlock> {
    let mapped: Vec<bt::Citation> = citations
        .citations
        .iter()
        .map(format_citation)
        .collect::<Result<Vec<_>>>()?;
    let content: Vec<bt::CitationGeneratedContent> = citations
        .content
        .iter()
        .map(|entry| bt::CitationGeneratedContent::Text(entry.text.clone()))
        .collect();
    Ok(bt::CitationsContentBlock::builder()
        .set_citations(Some(mapped))
        .set_content(Some(content))
        .build())
}

fn format_citation(citation: &Citation) -> Result<bt::Citation> {
    Ok(bt::Citation::builder()
        .location(format_citation_location(&citation.location)?)
        .set_source_content(Some(
            citation
                .source_content
                .iter()
                .map(|entry| bt::CitationSourceContent::Text(entry.text.clone()))
                .collect(),
        ))
        .source(&citation.source)
        .title(&citation.title)
        .build())
}

fn format_citation_location(location: &CitationLocation) -> Result<bt::CitationLocation> {
    Ok(match location {
        CitationLocation::DocumentChar {
            document_index,
            start,
            end,
        } => bt::CitationLocation::DocumentChar(
            bt::DocumentCharLocation::builder()
                .document_index(*document_index as i32)
                .start(*start as i32)
                .end(*end as i32)
                .build(),
        ),
        CitationLocation::DocumentPage {
            document_index,
            start,
            end,
        } => bt::CitationLocation::DocumentPage(
            bt::DocumentPageLocation::builder()
                .document_index(*document_index as i32)
                .start(*start as i32)
                .end(*end as i32)
                .build(),
        ),
        CitationLocation::DocumentChunk {
            document_index,
            start,
            end,
        } => bt::CitationLocation::DocumentChunk(
            bt::DocumentChunkLocation::builder()
                .document_index(*document_index as i32)
                .start(*start as i32)
                .end(*end as i32)
                .build(),
        ),
        CitationLocation::SearchResult {
            search_result_index,
            start,
            end,
        } => bt::CitationLocation::SearchResultLocation(
            bt::SearchResultLocation::builder()
                .search_result_index(*search_result_index as i32)
                .start(*start as i32)
                .end(*end as i32)
                .build(),
        ),
        CitationLocation::Web { url, domain } => {
            let mut builder = bt::WebLocation::builder().url(url);
            if let Some(domain) = domain.as_deref().filter(|d| !d.is_empty()) {
                builder = builder.domain(domain);
            }
            bt::CitationLocation::Web(builder.build())
        }
    })
}

fn image_format_str(format: &crate::types::ImageFormat) -> &'static str {
    use crate::types::ImageFormat::*;
    match format {
        Png => "png",
        Jpg => "jpg",
        Jpeg => "jpeg",
        Gif => "gif",
        Webp => "webp",
    }
}

fn video_format_str(format: &crate::types::VideoFormat) -> &'static str {
    use crate::types::VideoFormat::*;
    match format {
        Mkv => "mkv",
        Mov => "mov",
        Mp4 => "mp4",
        Webm => "webm",
        Flv => "flv",
        Mpeg => "mpeg",
        Mpg => "mpg",
        Wmv => "wmv",
        ThreeGp => "three_gp",
    }
}

fn document_format_str(format: &crate::types::DocumentFormat) -> &'static str {
    use crate::types::DocumentFormat::*;
    match format {
        Pdf => "pdf",
        Csv => "csv",
        Doc => "doc",
        Docx => "docx",
        Xls => "xls",
        Xlsx => "xlsx",
        Html => "html",
        Txt => "txt",
        Md => "md",
        Json => "json",
        Xml => "xml",
    }
}

fn audio_format_str(format: &crate::types::AudioFormat) -> &'static str {
    use crate::types::AudioFormat::*;
    match format {
        Mp3 => "mp3",
        Opus => "opus",
        Wav => "wav",
        Aac => "aac",
        Flac => "flac",
        Mp4 => "mp4",
        Ogg => "ogg",
        Mkv => "mkv",
        Mka => "mka",
        XAac => "x-aac",
        M4a => "m4a",
        Mpeg => "mpeg",
        Mpga => "mpga",
        Pcm => "pcm",
        Webm => "webm",
    }
}

/// `serde_json::Value` → smithy `Document`.
pub(crate) fn json_to_document(value: &Value) -> Result<Document> {
    Ok(match value {
        Value::Null => Document::Null,
        Value::Bool(b) => Document::Bool(*b),
        Value::Number(n) => {
            let number = if let Some(u) = n.as_u64() {
                Number::PosInt(u)
            } else if let Some(i) = n.as_i64() {
                Number::NegInt(i)
            } else if let Some(f) = n.as_f64() {
                Number::Float(f)
            } else {
                return Err(Error::model(format!("unsupported JSON number: {n}")));
            };
            Document::Number(number)
        }
        Value::String(s) => Document::String(s.clone()),
        Value::Array(items) => Document::Array(items.iter().map(json_to_document).collect::<Result<Vec<_>>>()?),
        Value::Object(entries) => Document::Object(
            entries
                .iter()
                .map(|(key, value)| Ok((key.clone(), json_to_document(value)?)))
                .collect::<Result<_>>()?,
        ),
    })
}

/// Smithy `Document` → `serde_json::Value`.
pub(crate) fn document_to_json(document: &Document) -> Value {
    match document {
        Document::Null => Value::Null,
        Document::Bool(b) => Value::Bool(*b),
        Document::Number(Number::PosInt(u)) => Value::from(*u),
        Document::Number(Number::NegInt(i)) => Value::from(*i),
        Document::Number(Number::Float(f)) => serde_json::Number::from_f64(*f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Document::String(s) => Value::String(s.clone()),
        Document::Array(items) => Value::Array(items.iter().map(document_to_json).collect()),
        Document::Object(entries) => {
            let mut map = Map::with_capacity(entries.len());
            let mut keys: Vec<&String> = entries.keys().collect();
            keys.sort();
            for key in keys {
                map.insert(key.clone(), document_to_json(&entries[key]));
            }
            Value::Object(map)
        }
    }
}

/// Keywords Bedrock strict mode rejects.
const UNSUPPORTED_STRICT_KEYWORDS: &[&str] = &[
    "exclusiveMaximum",
    "exclusiveMinimum",
    "maxLength",
    "maximum",
    "minLength",
    "minimum",
    "multipleOf",
];

/// Strict-mode copy of a tool's input schema, warning once per tool on rejected keywords.
fn to_strict_input_schema(tool_name: &str, input_schema: &JsonSchema) -> JsonSchema {
    let mut schema = input_schema.clone();
    let root = schema.clone();
    apply_strict(&mut schema, &root, &mut Vec::new());
    let mut unsupported = Vec::new();
    collect_unsupported_keywords(&schema, &mut unsupported);
    unsupported.sort();
    unsupported.dedup();
    if !unsupported.is_empty() {
        crate::logging::warn_once(&format!(
            "tool=<{tool_name}>, keywords=<{}> | tool schema uses keywords bedrock strict mode rejects, request may fail",
            unsupported.join(",")
        ));
    }
    schema
}

const SCHEMA_MAP_KEYWORDS: &[&str] = &["$defs", "definitions", "properties"];
const SCHEMA_LIST_KEYWORDS: &[&str] = &["anyOf", "allOf", "oneOf"];
const SCHEMA_VALUE_KEYWORDS: &[&str] = &["items", "additionalProperties"];

fn child_keys(schema: &Map<String, Value>) -> Vec<Vec<String>> {
    let mut paths = Vec::new();
    for keyword in SCHEMA_MAP_KEYWORDS {
        if let Some(Value::Object(map)) = schema.get(*keyword) {
            for key in map.keys() {
                paths.push(vec![(*keyword).to_owned(), key.clone()]);
            }
        }
    }
    for keyword in SCHEMA_LIST_KEYWORDS {
        if let Some(Value::Array(items)) = schema.get(*keyword) {
            for idx in 0..items.len() {
                paths.push(vec![(*keyword).to_owned(), idx.to_string()]);
            }
        }
    }
    for keyword in SCHEMA_VALUE_KEYWORDS {
        if matches!(schema.get(*keyword), Some(Value::Object(_))) {
            paths.push(vec![(*keyword).to_owned()]);
        }
    }
    paths
}

fn child_mut<'a>(schema: &'a mut Map<String, Value>, path: &[String]) -> Option<&'a mut Value> {
    match path {
        [keyword] => schema.get_mut(keyword),
        [keyword, key] => match schema.get_mut(keyword)? {
            Value::Object(map) => map.get_mut(key),
            Value::Array(items) => key.parse::<usize>().ok().and_then(|idx| items.get_mut(idx)),
            _ => None,
        },
        _ => None,
    }
}

fn is_object_type(schema: &Map<String, Value>) -> bool {
    match schema.get("type") {
        Some(Value::String(t)) => t == "object",
        Some(Value::Array(types)) => types.iter().any(|t| t == "object"),
        _ => false,
    }
}

fn apply_strict(schema: &mut Value, root: &Value, inlining: &mut Vec<String>) {
    let Value::Object(map) = schema else { return };

    if is_object_type(map) && !map.contains_key("additionalProperties") && !map.contains_key("$ref") {
        map.insert("additionalProperties".to_owned(), Value::Bool(false));
    }

    for path in child_keys(map) {
        if let Some(child) = child_mut(map, &path) {
            apply_strict(child, root, inlining);
        }
    }

    let Some(Value::String(reference)) = map.get("$ref").cloned() else {
        return;
    };
    if map.len() <= 1 {
        return;
    }
    if inlining.iter().any(|r| r == &reference) {
        crate::logging::warn_once(&format!(
            "ref=<{reference}> | recursive $ref cannot be inlined, leaving it unresolved"
        ));
        return;
    }
    let Some(Value::Object(resolved)) = resolve_ref(root, &reference) else {
        return;
    };
    let mut merged = resolved;
    for (key, value) in map.iter() {
        merged.insert(key.clone(), value.clone());
    }
    merged.remove("$ref");
    *map = merged;
    inlining.push(reference);
    apply_strict(schema, root, inlining);
    inlining.pop();
}

fn resolve_ref(root: &Value, reference: &str) -> Option<Value> {
    let Some(path) = reference.strip_prefix("#/") else {
        tracing::warn!("ref=<{reference}> | unexpected $ref format, skipping resolution");
        return None;
    };
    let mut current = root;
    for key in path.split('/') {
        match current.get(key) {
            Some(next) => current = next,
            None => {
                tracing::warn!("ref=<{reference}> | failed to resolve $ref path");
                return None;
            }
        }
    }
    if !current.is_object() {
        tracing::warn!("ref=<{reference}> | resolved to non-dict value");
        return None;
    }
    Some(current.clone())
}

fn collect_unsupported_keywords(schema: &Value, found: &mut Vec<String>) {
    let Value::Object(map) = schema else { return };
    for keyword in UNSUPPORTED_STRICT_KEYWORDS {
        if map.contains_key(*keyword) {
            found.push((*keyword).to_owned());
        }
    }
    if let Some(value) = map.get("additionalProperties") {
        if value != &Value::Bool(false) {
            found.push("additionalProperties".to_owned());
        }
    }
    for path in child_keys(map) {
        let child = match path.as_slice() {
            [keyword] => map.get(keyword),
            [keyword, key] => match map.get(keyword) {
                Some(Value::Object(inner)) => inner.get(key),
                Some(Value::Array(items)) => key.parse::<usize>().ok().and_then(|idx| items.get(idx)),
                _ => None,
            },
            _ => None,
        };
        if let Some(child) = child {
            collect_unsupported_keywords(child, found);
        }
    }
}
