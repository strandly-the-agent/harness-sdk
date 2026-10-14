//! Converse request formatting (`_formatRequest` and friends in `models/bedrock.ts`).

use aws_sdk_bedrockruntime::types as bt;
use aws_smithy_types::{Blob, Document, Number};
use serde_json::{Map, Value};

use super::{
    model_error, resolve_cache_section, BedrockModelConfig, CacheSection, CacheStrategy,
    GuardrailStreamProcessingMode, GuardrailTraceMode, IncludeToolResultStatus, MODELS_INCLUDE_STATUS,
    MODELS_SUPPORTING_ANTHROPIC_CACHING,
};
use crate::error::{Error, Result};
use crate::models::defaults::MODEL_DEFAULTS;
use crate::models::model::StreamOptions;
use crate::types::{
    AudioSource, Citation, CitationLocation, CitationsBlock, ContentBlock, DocumentBlock, DocumentSource,
    GuardContentBlock, GuardImageFormat, GuardQualifier, ImageBlock, ImageSource, JsonSchema, JsonValue,
    Message, Role, SystemContentBlock, SystemPrompt, ToolChoice, ToolResultBlock, ToolResultContent,
    ToolResultStatus, ToolSpec, ToolUseBlock, VideoBlock, VideoSource,
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
        config
            .cache_config
            .as_ref()
            .and_then(|c| c.system_prompt_ttl.as_ref()),
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
        tracing::debug!(
            "msg_idx=<{msg_idx}>, block_idx=<{target_idx}> | relocated caller-placed cache point"
        );
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
            let should_include = MODELS_INCLUDE_STATUS
                .iter()
                .any(|pattern| model_id.contains(pattern));
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
        SystemContentBlock::GuardContent(guard) => {
            bt::SystemContentBlock::GuardContent(format_guard_content(guard)?)
        }
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
fn format_content_block(
    config: &BedrockModelConfig,
    block: &ContentBlock,
) -> Result<Option<bt::ContentBlock>> {
    Ok(match block {
        ContentBlock::Text(text) => Some(bt::ContentBlock::Text(text.clone())),
        ContentBlock::ToolUse(tool_use) => Some(bt::ContentBlock::ToolUse(format_tool_use(tool_use)?)),
        ContentBlock::ToolResult(result) => {
            Some(bt::ContentBlock::ToolResult(format_tool_result(config, result)?))
        }
        ContentBlock::Reasoning(reasoning) => {
            if reasoning.text.is_some() || reasoning.signature.is_some() {
                let mut builder =
                    bt::ReasoningTextBlock::builder().text(reasoning.text.clone().unwrap_or_default());
                if let Some(signature) = &reasoning.signature {
                    builder = builder.signature(signature);
                }
                Some(bt::ContentBlock::ReasoningContent(
                    bt::ReasoningContentBlock::ReasoningText(
                        builder
                            .build()
                            .map_err(|e| model_error("invalid reasoning block", e))?,
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
        ContentBlock::CachePoint(point) => {
            Some(bt::ContentBlock::CachePoint(cache_point(point.ttl.as_deref())?))
        }
        ContentBlock::Audio(audio) => {
            let source = match &audio.source {
                AudioSource::Bytes(bytes) => Some(bt::AudioSource::Bytes(Blob::new(bytes.clone()))),
                AudioSource::Location(location) => {
                    Some(bt::AudioSource::S3Location(format_s3_location(location)?))
                }
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
        ContentBlock::GuardContent(guard) => {
            Some(bt::ContentBlock::GuardContent(format_guard_content(guard)?))
        }
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
            ToolResultContent::Document(document) => {
                bt::ToolResultContentBlock::Document(format_document(document)?)
            }
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
    builder.build().map_err(|e| model_error("invalid s3 location", e))
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
        Value::Array(items) => {
            Document::Array(items.iter().map(json_to_document).collect::<Result<Vec<_>>>()?)
        }
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::models::bedrock::{
        BedrockGuardrailConfig, CacheConfig, CacheSection, CacheStrategy, GuardrailStreamProcessingMode,
        GuardrailTraceMode,
    };
    use crate::types::{CachePointBlock, ReasoningBlock, Role, ToolResultContent};

    fn claude() -> BedrockModelConfig {
        BedrockModelConfig::new("anthropic.claude-sonnet-4-20250514-v1:0")
    }

    fn tool_spec(name: &str) -> ToolSpec {
        ToolSpec {
            name: name.to_owned(),
            description: "a tool".to_owned(),
            input_schema: Some(json!({"type": "object", "properties": {"a": {"type": "string"}}})),
            output_schema: Some(json!({"type": "object"})),
            annotations: None,
        }
    }

    fn user(blocks: Vec<ContentBlock>) -> Message {
        Message::new(Role::User, blocks)
    }

    fn format(
        config: &BedrockModelConfig,
        messages: &[Message],
        options: &StreamOptions,
    ) -> FormattedRequest {
        format_request(config, messages, options).expect("format_request")
    }

    fn text_of(block: &bt::ContentBlock) -> &str {
        block.as_text().expect("text block")
    }

    fn msg_cache_ttls(request: &FormattedRequest, index: usize) -> Vec<Option<String>> {
        request.messages[index]
            .content()
            .iter()
            .filter_map(|block| block.as_cache_point().ok())
            .map(|point| point.ttl().map(|ttl| ttl.as_str().to_owned()))
            .collect()
    }

    fn system_cache_ttls(request: &FormattedRequest) -> Vec<Option<String>> {
        request
            .system
            .as_ref()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|block| block.as_cache_point().ok())
                    .map(|point| point.ttl().map(|ttl| ttl.as_str().to_owned()))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn with_cache(config: &mut BedrockModelConfig, cache: CacheConfig) {
        config.cache_config = Some(cache);
    }

    #[test]
    fn appends_a_cache_point_to_a_string_system_prompt_with_cache_config() {
        let mut config = claude();
        with_cache(&mut config, CacheConfig::default());
        let options = StreamOptions {
            system_prompt: Some(SystemPrompt::Text("sys".into())),
            ..Default::default()
        };
        let request = format(&config, &[user(vec![ContentBlock::text("hi")])], &options);
        assert_eq!(system_cache_ttls(&request), vec![None]);
    }

    #[test]
    fn skips_the_system_cache_point_when_the_system_prompt_is_absent() {
        let mut config = claude();
        with_cache(&mut config, CacheConfig::default());
        let request = format(
            &config,
            &[user(vec![ContentBlock::text("hi")])],
            &StreamOptions::default(),
        );
        assert!(request.system.is_none());
    }

    #[test]
    fn carries_cache_config_ttl_into_the_appended_system_cache_point() {
        let mut config = claude();
        with_cache(
            &mut config,
            CacheConfig {
                ttl: Some("5m".into()),
                ..Default::default()
            },
        );
        let options = StreamOptions {
            system_prompt: Some(SystemPrompt::Text("sys".into())),
            ..Default::default()
        };
        let request = format(&config, &[user(vec![ContentBlock::text("hi")])], &options);
        assert_eq!(system_cache_ttls(&request), vec![Some("5m".to_owned())]);
    }

    #[test]
    fn does_not_inject_a_system_cache_point_when_system_prompt_ttl_is_false() {
        let mut config = claude();
        with_cache(
            &mut config,
            CacheConfig {
                system_prompt_ttl: Some(CacheSection::Disabled),
                ..Default::default()
            },
        );
        let options = StreamOptions {
            system_prompt: Some(SystemPrompt::Text("sys".into())),
            ..Default::default()
        };
        let request = format(&config, &[user(vec![ContentBlock::text("hi")])], &options);
        assert!(system_cache_ttls(&request).is_empty());
    }

    #[test]
    fn handles_empty_array_system_prompt() {
        let options = StreamOptions {
            system_prompt: Some(SystemPrompt::Blocks(vec![])),
            ..Default::default()
        };
        let request = format(&claude(), &[user(vec![ContentBlock::text("hi")])], &options);
        assert!(request.system.is_none());
    }

    #[test]
    fn adds_cache_points_to_tools_and_messages_when_cache_config_enabled() {
        let mut config = claude();
        with_cache(&mut config, CacheConfig::default());
        let options = StreamOptions {
            tool_specs: vec![tool_spec("t1")],
            ..Default::default()
        };
        let request = format(&config, &[user(vec![ContentBlock::text("hi")])], &options);
        let tools = request.tool_config.as_ref().expect("toolConfig").tools();
        assert!(tools.last().expect("tool").as_cache_point().is_ok());
        assert_eq!(msg_cache_ttls(&request, 0), vec![None]);
    }

    #[test]
    fn propagates_cache_config_ttls_independently_to_tools_and_last_user_message() {
        let mut config = claude();
        with_cache(
            &mut config,
            CacheConfig {
                tools_ttl: Some(CacheSection::Ttl("1h".into())),
                messages_ttl: Some(CacheSection::Ttl("5m".into())),
                ..Default::default()
            },
        );
        let options = StreamOptions {
            tool_specs: vec![tool_spec("t1")],
            ..Default::default()
        };
        let request = format(&config, &[user(vec![ContentBlock::text("hi")])], &options);
        let tools = request.tool_config.as_ref().expect("toolConfig").tools();
        assert_eq!(
            tools
                .last()
                .expect("tool")
                .as_cache_point()
                .expect("cachePoint")
                .ttl()
                .map(|ttl| ttl.as_str()),
            Some("1h")
        );
        assert_eq!(msg_cache_ttls(&request, 0), vec![Some("5m".to_owned())]);
    }

    #[test]
    fn caches_only_the_tool_definitions_when_messages_ttl_is_false() {
        let mut config = claude();
        with_cache(
            &mut config,
            CacheConfig {
                messages_ttl: Some(CacheSection::Disabled),
                ..Default::default()
            },
        );
        let options = StreamOptions {
            tool_specs: vec![tool_spec("t1")],
            ..Default::default()
        };
        let request = format(&config, &[user(vec![ContentBlock::text("hi")])], &options);
        let tools = request.tool_config.as_ref().expect("toolConfig").tools();
        assert!(tools.last().expect("tool").as_cache_point().is_ok());
        assert!(msg_cache_ttls(&request, 0).is_empty());
    }

    #[test]
    fn treats_an_empty_ttl_as_unset_rather_than_shipping_it() {
        let mut config = claude();
        with_cache(
            &mut config,
            CacheConfig {
                ttl: Some(String::new()),
                ..Default::default()
            },
        );
        let request = format(
            &config,
            &[user(vec![ContentBlock::text("hi")])],
            &StreamOptions::default(),
        );
        assert_eq!(msg_cache_ttls(&request, 0), vec![None]);
    }

    #[test]
    fn falls_through_an_empty_per_section_ttl_to_the_shared_ttl() {
        let mut config = claude();
        with_cache(
            &mut config,
            CacheConfig {
                ttl: Some("5m".into()),
                messages_ttl: Some(CacheSection::Ttl(String::new())),
                ..Default::default()
            },
        );
        let request = format(
            &config,
            &[user(vec![ContentBlock::text("hi")])],
            &StreamOptions::default(),
        );
        assert_eq!(msg_cache_ttls(&request, 0), vec![Some("5m".to_owned())]);
    }

    #[test]
    fn ignores_cache_key_it_does_not_change_the_request_shape() {
        let mut with_key = claude();
        with_cache(
            &mut with_key,
            CacheConfig {
                cache_key: Some("k".into()),
                ..Default::default()
            },
        );
        let mut without = claude();
        with_cache(&mut without, CacheConfig::default());
        let messages = [user(vec![ContentBlock::text("hi")])];
        assert_eq!(
            format(&with_key, &messages, &StreamOptions::default()),
            format(&without, &messages, &StreamOptions::default())
        );
    }

    #[test]
    fn logs_warning_and_disables_caching_for_non_caching_models() {
        let mut config = BedrockModelConfig::new("amazon.nova-pro-v1:0");
        with_cache(&mut config, CacheConfig::default());
        let request = format(
            &config,
            &[user(vec![ContentBlock::text("hi")])],
            &StreamOptions::default(),
        );
        assert!(msg_cache_ttls(&request, 0).is_empty());
        assert!(!should_enable_caching(&config));
    }

    #[test]
    fn enables_caching_with_anthropic_strategy_for_application_inference_profiles() {
        let mut config =
            BedrockModelConfig::new("arn:aws:bedrock:us-west-2:1:application-inference-profile/abc");
        with_cache(
            &mut config,
            CacheConfig {
                strategy: Some(CacheStrategy::Anthropic),
                ..Default::default()
            },
        );
        assert!(should_enable_caching(&config));
        let request = format(
            &config,
            &[user(vec![ContentBlock::text("hi")])],
            &StreamOptions::default(),
        );
        assert_eq!(msg_cache_ttls(&request, 0), vec![None]);
    }

    #[test]
    fn preserves_ttl_on_user_supplied_cache_point_blocks_in_messages() {
        let message = user(vec![
            ContentBlock::text("hi"),
            ContentBlock::CachePoint(CachePointBlock {
                cache_type: Default::default(),
                ttl: Some("1h".into()),
            }),
        ]);
        let request = format(&claude(), &[message], &StreamOptions::default());
        assert_eq!(msg_cache_ttls(&request, 0), vec![Some("1h".to_owned())]);
    }

    #[test]
    fn omits_a_falsy_ttl_on_user_supplied_cache_point_blocks_in_messages() {
        for ttl in [None, Some(String::new())] {
            let message = user(vec![
                ContentBlock::text("hi"),
                ContentBlock::CachePoint(CachePointBlock {
                    cache_type: Default::default(),
                    ttl,
                }),
            ]);
            let request = format(&claude(), &[message], &StreamOptions::default());
            assert_eq!(msg_cache_ttls(&request, 0), vec![None]);
        }
    }

    #[test]
    fn forwards_arbitrary_ttl_strings_without_client_side_validation() {
        let mut config = claude();
        with_cache(
            &mut config,
            CacheConfig {
                messages_ttl: Some(CacheSection::Ttl("37 fortnights".into())),
                ..Default::default()
            },
        );
        let request = format(
            &config,
            &[user(vec![ContentBlock::text("hi")])],
            &StreamOptions::default(),
        );
        assert_eq!(
            msg_cache_ttls(&request, 0),
            vec![Some("37 fortnights".to_owned())]
        );
    }

    #[test]
    fn does_not_mutate_the_original_messages_array() {
        let messages = vec![user(vec![ContentBlock::text("hi")])];
        let snapshot = messages.clone();
        let mut config = claude();
        with_cache(&mut config, CacheConfig::default());
        let _ = format(&config, &messages, &StreamOptions::default());
        assert_eq!(messages, snapshot);
    }

    #[test]
    fn appends_the_cache_point_at_the_end_when_no_per_call_tail_is_marked() {
        let mut config = claude();
        with_cache(&mut config, CacheConfig::default());
        let message = user(vec![ContentBlock::text("a"), ContentBlock::text("b")]);
        let request = format(&config, &[message], &StreamOptions::default());
        let content = request.messages[0].content();
        assert!(content.last().expect("block").as_cache_point().is_ok());
    }

    #[test]
    fn places_the_cache_point_ahead_of_the_per_call_tail() {
        let mut config = claude();
        with_cache(&mut config, CacheConfig::default());
        let message = user(vec![ContentBlock::text("a"), ContentBlock::text("tail")]);
        let options = StreamOptions {
            dynamic_trailing_blocks: Some(1),
            ..Default::default()
        };
        let request = format(&config, &[message], &options);
        let content = request.messages[0].content();
        assert!(content[1].as_cache_point().is_ok());
        assert_eq!(text_of(&content[2]), "tail");
    }

    #[test]
    fn skips_the_cache_point_when_the_whole_message_is_per_call_content() {
        let mut config = claude();
        with_cache(&mut config, CacheConfig::default());
        let message = user(vec![ContentBlock::text("a")]);
        let options = StreamOptions {
            dynamic_trailing_blocks: Some(1),
            ..Default::default()
        };
        let request = format(&config, &[message], &options);
        assert!(msg_cache_ttls(&request, 0).is_empty());
    }

    #[test]
    fn emits_no_cache_point_for_a_per_call_tail_when_caching_is_not_configured() {
        let options = StreamOptions {
            dynamic_trailing_blocks: Some(1),
            ..Default::default()
        };
        let message = user(vec![ContentBlock::text("a"), ContentBlock::text("tail")]);
        let request = format(&claude(), &[message], &options);
        assert!(msg_cache_ttls(&request, 0).is_empty());
    }

    #[test]
    fn formats_tool_choice_variants() {
        for (choice, expect_tool) in [
            (ToolChoice::Auto {}, None),
            (ToolChoice::Any {}, None),
            (ToolChoice::Tool { name: "t1".into() }, Some("t1")),
        ] {
            let options = StreamOptions {
                tool_specs: vec![tool_spec("t1")],
                tool_choice: Some(choice.clone()),
                ..Default::default()
            };
            let request = format(&claude(), &[user(vec![ContentBlock::text("hi")])], &options);
            let formatted = request
                .tool_config
                .as_ref()
                .expect("toolConfig")
                .tool_choice()
                .expect("toolChoice");
            match (choice, expect_tool) {
                (ToolChoice::Auto {}, _) => assert!(formatted.as_auto().is_ok()),
                (ToolChoice::Any {}, _) => assert!(formatted.as_any().is_ok()),
                (ToolChoice::Tool { .. }, Some(name)) => {
                    assert_eq!(formatted.as_tool().expect("tool").name(), name);
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn forwards_additional_model_request_fields_and_response_field_paths() {
        let mut config = claude();
        config.additional_request_fields = Some(json!({"some_other_field": "value"}));
        config.additional_response_field_paths = Some(vec!["/stop_reason".into()]);
        let request = format(
            &config,
            &[user(vec![ContentBlock::text("hi")])],
            &StreamOptions::default(),
        );
        assert_eq!(
            document_to_json(request.additional_model_request_fields.as_ref().expect("fields")),
            json!({"some_other_field": "value"})
        );
        assert_eq!(
            request.additional_model_response_field_paths.as_deref(),
            Some(["/stop_reason".to_owned()].as_slice())
        );
    }

    #[test]
    fn strips_thinking_from_additional_request_fields_when_tool_choice_forces_tool_use() {
        for choice in [ToolChoice::Any {}, ToolChoice::Tool { name: "t1".into() }] {
            let mut config = claude();
            config.additional_request_fields = Some(json!({
                "thinking": {"type": "enabled", "budget_tokens": 1024},
                "some_other_field": "value"
            }));
            let options = StreamOptions {
                tool_specs: vec![tool_spec("t1")],
                tool_choice: Some(choice),
                ..Default::default()
            };
            let request = format(&config, &[user(vec![ContentBlock::text("hi")])], &options);
            assert_eq!(
                document_to_json(request.additional_model_request_fields.as_ref().expect("fields")),
                json!({"some_other_field": "value"})
            );
        }
    }

    #[test]
    fn preserves_thinking_when_tool_choice_is_auto_or_absent() {
        let fields = json!({"thinking": {"type": "enabled"}});
        for choice in [Some(ToolChoice::Auto {}), None] {
            let mut config = claude();
            config.additional_request_fields = Some(fields.clone());
            let options = StreamOptions {
                tool_specs: vec![tool_spec("t1")],
                tool_choice: choice,
                ..Default::default()
            };
            let request = format(&config, &[user(vec![ContentBlock::text("hi")])], &options);
            assert_eq!(
                document_to_json(request.additional_model_request_fields.as_ref().expect("fields")),
                fields
            );
        }
    }

    #[test]
    fn omits_additional_model_request_fields_when_thinking_is_the_only_field_and_tool_choice_forces_tool_use()
    {
        let mut config = claude();
        config.additional_request_fields = Some(json!({"thinking": {"type": "enabled"}}));
        let options = StreamOptions {
            tool_specs: vec![tool_spec("t1")],
            tool_choice: Some(ToolChoice::Any {}),
            ..Default::default()
        };
        let request = format(&config, &[user(vec![ContentBlock::text("hi")])], &options);
        assert!(request.additional_model_request_fields.is_none());
    }

    #[test]
    fn includes_guardrail_config_in_request_with_default_trace() {
        let mut config = claude();
        config.guardrail_config = Some(BedrockGuardrailConfig {
            guardrail_identifier: "gr-1".into(),
            guardrail_version: "1".into(),
            ..Default::default()
        });
        let request = format(
            &config,
            &[user(vec![ContentBlock::text("hi")])],
            &StreamOptions::default(),
        );
        let guardrail = request.guardrail.as_ref().expect("guardrail");
        assert_eq!(guardrail.identifier, "gr-1");
        assert_eq!(guardrail.version, "1");
        assert_eq!(guardrail.trace, GuardrailTraceMode::Enabled);
        let converse = guardrail.to_converse().expect("converse config");
        assert_eq!(converse.trace(), &bt::GuardrailTrace::Enabled);
    }

    #[test]
    fn includes_stream_processing_mode_when_specified() {
        let mut config = claude();
        config.guardrail_config = Some(BedrockGuardrailConfig {
            guardrail_identifier: "gr-1".into(),
            guardrail_version: "1".into(),
            trace: Some(GuardrailTraceMode::EnabledFull),
            stream_processing_mode: Some(GuardrailStreamProcessingMode::Async),
            ..Default::default()
        });
        let request = format(
            &config,
            &[user(vec![ContentBlock::text("hi")])],
            &StreamOptions::default(),
        );
        let guardrail = request.guardrail.as_ref().expect("guardrail");
        assert_eq!(guardrail.trace, GuardrailTraceMode::EnabledFull);
        let stream = guardrail.to_stream().expect("stream config");
        assert_eq!(
            stream.stream_processing_mode(),
            &bt::GuardrailStreamProcessingMode::Async
        );
    }

    #[test]
    fn does_not_include_guardrail_config_when_not_configured() {
        let request = format(
            &claude(),
            &[user(vec![ContentBlock::text("hi")])],
            &StreamOptions::default(),
        );
        assert!(request.guardrail.is_none());
    }

    #[test]
    fn wraps_latest_user_message_text_content_in_guard_content_when_enabled() {
        let mut config = claude();
        config.guardrail_config = Some(BedrockGuardrailConfig {
            guardrail_identifier: "gr-1".into(),
            guardrail_version: "1".into(),
            guard_latest_user_message: Some(true),
            ..Default::default()
        });
        let messages = [
            user(vec![ContentBlock::text("older")]),
            Message::new(Role::Assistant, vec![ContentBlock::text("reply")]),
            user(vec![ContentBlock::text("latest")]),
        ];
        let request = format(&config, &messages, &StreamOptions::default());
        assert!(request.messages[2].content()[0].as_guard_content().is_ok());
        assert_eq!(text_of(&request.messages[0].content()[0]), "older");
    }

    #[test]
    fn does_not_wrap_messages_when_guard_latest_user_message_is_false_or_undefined() {
        for guard in [Some(false), None] {
            let mut config = claude();
            config.guardrail_config = Some(BedrockGuardrailConfig {
                guardrail_identifier: "gr-1".into(),
                guardrail_version: "1".into(),
                guard_latest_user_message: guard,
                ..Default::default()
            });
            let request = format(
                &config,
                &[user(vec![ContentBlock::text("hi")])],
                &StreamOptions::default(),
            );
            assert_eq!(text_of(&request.messages[0].content()[0]), "hi");
        }
    }

    #[test]
    fn does_not_wrap_tool_result_messages_even_though_role_is_user() {
        let mut config = claude();
        config.guardrail_config = Some(BedrockGuardrailConfig {
            guardrail_identifier: "gr-1".into(),
            guardrail_version: "1".into(),
            guard_latest_user_message: Some(true),
            ..Default::default()
        });
        let message = user(vec![ContentBlock::ToolResult(ToolResultBlock::success(
            "tu-1",
            vec![ToolResultContent::text("done")],
        ))]);
        let request = format(&config, &[message], &StreamOptions::default());
        assert!(request.messages[0].content()[0].as_tool_result().is_ok());
    }

    #[test]
    fn injects_strict_and_closes_the_schema_when_strict_tools_is_true() {
        let mut config = claude();
        config.strict_tools = Some(true);
        let spec = tool_spec("t1");
        let original = spec.input_schema.clone();
        let options = StreamOptions {
            tool_specs: vec![spec.clone()],
            ..Default::default()
        };
        let request = format(&config, &[user(vec![ContentBlock::text("hi")])], &options);
        let formatted = request.tool_config.as_ref().expect("toolConfig").tools()[0]
            .as_tool_spec()
            .expect("spec");
        let schema = document_to_json(formatted.input_schema().expect("schema").as_json().expect("json"));
        assert_eq!(schema["additionalProperties"], json!(false));
        assert_eq!(spec.input_schema, original, "caller schema must not be mutated");
    }

    #[test]
    fn omits_strict_and_leaves_the_schema_unchanged_by_default() {
        for strict in [None, Some(false)] {
            let mut config = claude();
            config.strict_tools = strict;
            let options = StreamOptions {
                tool_specs: vec![tool_spec("t1")],
                ..Default::default()
            };
            let request = format(&config, &[user(vec![ContentBlock::text("hi")])], &options);
            let formatted = request.tool_config.as_ref().expect("toolConfig").tools()[0]
                .as_tool_spec()
                .expect("spec");
            let schema = document_to_json(formatted.input_schema().expect("schema").as_json().expect("json"));
            assert!(schema.get("additionalProperties").is_none());
            assert!(schema.get("strict").is_none());
        }
    }

    #[test]
    fn formats_the_request_to_bedrock_properly() {
        let options = StreamOptions {
            system_prompt: Some(SystemPrompt::Text("be nice".into())),
            tool_specs: vec![tool_spec("t1")],
            ..Default::default()
        };
        let mut config = claude();
        config.max_tokens = Some(128);
        config.temperature = Some(0.5);
        config.top_p = Some(0.9);
        config.stop_sequences = Some(vec!["STOP".into()]);

        let request = format(&config, &[user(vec![ContentBlock::text("hi")])], &options);

        assert_eq!(request.model_id, "anthropic.claude-sonnet-4-20250514-v1:0");
        assert_eq!(request.messages.len(), 1);
        assert_eq!(request.messages[0].role(), &bt::ConversationRole::User);
        assert_eq!(text_of(&request.messages[0].content()[0]), "hi");
        let system = request.system.as_ref().expect("system");
        assert_eq!(system[0].as_text().expect("text"), "be nice");
        let tool_config = request.tool_config.as_ref().expect("toolConfig");
        assert_eq!(tool_config.tools().len(), 1);
        let inference = request.inference_config.as_ref().expect("inferenceConfig");
        assert_eq!(inference.max_tokens(), Some(128));
        assert_eq!(inference.temperature(), Some(0.5));
        assert_eq!(inference.top_p(), Some(0.9));
        assert_eq!(inference.stop_sequences(), ["STOP"]);
        assert!(request.guardrail.is_none());
    }

    #[test]
    fn formats_tool_use_messages() {
        let message = Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse(ToolUseBlock::new(
                "calc",
                "tu-1",
                json!({"x": 1}),
            ))],
        );
        let request = format(&claude(), &[message], &StreamOptions::default());
        let tool_use = request.messages[0].content()[0].as_tool_use().expect("toolUse");
        assert_eq!(tool_use.tool_use_id(), "tu-1");
        assert_eq!(tool_use.name(), "calc");
        assert_eq!(document_to_json(tool_use.input()), json!({"x": 1}));
    }

    #[test]
    fn formats_tool_result_messages() {
        let message = user(vec![ContentBlock::ToolResult(ToolResultBlock::success(
            "tu-1",
            vec![
                ToolResultContent::text("done"),
                ToolResultContent::Json(json!({"n": 2})),
            ],
        ))]);
        let request = format(&claude(), &[message], &StreamOptions::default());
        let result = request.messages[0].content()[0]
            .as_tool_result()
            .expect("toolResult");
        assert_eq!(result.tool_use_id(), "tu-1");
        assert_eq!(result.status(), Some(&bt::ToolResultStatus::Success));
        assert_eq!(result.content().len(), 2);
    }

    #[test]
    fn injects_noop_tool_config_when_messages_have_tool_blocks_but_no_tool_specs() {
        let message = Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse(ToolUseBlock::new(
                "calc",
                "tu-1",
                json!({}),
            ))],
        );
        let request = format(&claude(), &[message], &StreamOptions::default());
        let tools = request.tool_config.as_ref().expect("toolConfig").tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools[0].as_tool_spec().expect("spec").name(),
            noop_tool_spec().name
        );
    }

    #[test]
    fn does_not_inject_noop_tool_config_when_messages_have_no_tool_blocks() {
        let request = format(
            &claude(),
            &[user(vec![ContentBlock::text("hi")])],
            &StreamOptions::default(),
        );
        assert!(request.tool_config.is_none());
    }

    #[test]
    fn does_not_inject_noop_tool_config_when_tool_specs_are_provided() {
        let options = StreamOptions {
            tool_specs: vec![tool_spec("real")],
            ..Default::default()
        };
        let message = Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse(ToolUseBlock::new(
                "real",
                "tu-1",
                json!({}),
            ))],
        );
        let request = format(&claude(), &[message], &options);
        let tools = request.tool_config.as_ref().expect("toolConfig").tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].as_tool_spec().expect("spec").name(), "real");
    }

    #[test]
    fn formats_tool_specs_without_output_schema_in_bedrock_requests() {
        let options = StreamOptions {
            tool_specs: vec![tool_spec("t1")],
            ..Default::default()
        };
        let request = format(&claude(), &[user(vec![ContentBlock::text("hi")])], &options);
        let spec = request.tool_config.as_ref().expect("toolConfig").tools()[0]
            .as_tool_spec()
            .expect("spec");
        let schema = document_to_json(spec.input_schema().expect("schema").as_json().expect("json"));
        assert!(schema.get("outputSchema").is_none());
        assert_eq!(schema["type"], "object");
    }

    #[test]
    fn formats_reasoning_messages_properly() {
        let message = Message::new(
            Role::Assistant,
            vec![ContentBlock::Reasoning(ReasoningBlock {
                text: Some("thinking".into()),
                signature: Some("sig".into()),
                redacted_content: None,
            })],
        );
        let request = format(&claude(), &[message], &StreamOptions::default());
        let reasoning = request.messages[0].content()[0]
            .as_reasoning_content()
            .expect("reasoningContent");
        let reasoning_text = reasoning.as_reasoning_text().expect("reasoningText");
        assert_eq!(reasoning_text.text(), "thinking");
        assert_eq!(reasoning_text.signature(), Some("sig"));
    }

    #[test]
    fn formats_a_signature_only_reasoning_block() {
        let message = Message::new(
            Role::Assistant,
            vec![ContentBlock::Reasoning(ReasoningBlock {
                text: None,
                signature: Some("sig".into()),
                redacted_content: None,
            })],
        );
        let request = format(&claude(), &[message], &StreamOptions::default());
        let reasoning_text = request.messages[0].content()[0]
            .as_reasoning_content()
            .expect("reasoningContent")
            .as_reasoning_text()
            .expect("reasoningText");
        assert_eq!(reasoning_text.text(), "");
        assert_eq!(reasoning_text.signature(), Some("sig"));
    }

    #[test]
    fn throws_for_a_reasoning_block_with_no_text_signature_or_redacted_content() {
        let message = Message::new(
            Role::Assistant,
            vec![ContentBlock::Reasoning(ReasoningBlock::default())],
        );
        let error = format_request(&claude(), &[message], &StreamOptions::default())
            .expect_err("reasoning block without content must fail");
        assert!(matches!(error, Error::Model { .. }), "got {error:?}");
    }

    fn auto_cache(messages_ttl: Option<CacheSection>) -> BedrockModelConfig {
        let mut config = claude();
        with_cache(
            &mut config,
            CacheConfig {
                strategy: Some(CacheStrategy::Auto),
                messages_ttl,
                ..Default::default()
            },
        );
        config
    }

    fn cp_block(ttl: Option<&str>) -> ContentBlock {
        ContentBlock::CachePoint(CachePointBlock {
            cache_type: Default::default(),
            ttl: ttl.map(str::to_owned),
        })
    }

    fn doc_block(name: &str, format: crate::types::DocumentFormat) -> ContentBlock {
        ContentBlock::Document(DocumentBlock {
            name: name.to_owned(),
            format,
            source: DocumentSource::Bytes(vec![1]),
            citations: None,
            context: None,
        })
    }

    fn img_block(format: crate::types::ImageFormat, source: ImageSource) -> ContentBlock {
        ContentBlock::Image(ImageBlock { format, source })
    }

    fn block_tag(block: &bt::ContentBlock) -> String {
        match block {
            bt::ContentBlock::Text(text) => format!("text:{text}"),
            bt::ContentBlock::CachePoint(point) => match point.ttl() {
                Some(ttl) => format!("cachePoint:{}", ttl.as_str()),
                None => "cachePoint".to_owned(),
            },
            bt::ContentBlock::Document(doc) => format!("document:{}", doc.name()),
            bt::ContentBlock::Image(image) => format!("image:{}", image.format().as_str()),
            bt::ContentBlock::ToolResult(_) => "toolResult".to_owned(),
            bt::ContentBlock::ReasoningContent(_) => "reasoning".to_owned(),
            bt::ContentBlock::GuardContent(guard) => match guard {
                bt::GuardrailConverseContentBlock::Text(text) => format!("guardText:{}", text.text()),
                bt::GuardrailConverseContentBlock::Image(image) => {
                    format!("guardImage:{}", image.format().as_str())
                }
                other => format!("guardOther:{other:?}"),
            },
            other => format!("other:{other:?}"),
        }
    }

    fn tags(request: &FormattedRequest, index: usize) -> Vec<String> {
        request.messages[index].content().iter().map(block_tag).collect()
    }

    #[test]
    fn replaces_a_leading_caller_placed_cache_point_with_automatic_placement() {
        let config = auto_cache(None);
        let messages = [user(vec![cp_block(None), ContentBlock::text("durable ask")])];
        let request = format(&config, &messages, &StreamOptions::default());
        assert_eq!(tags(&request, 0), ["text:durable ask", "cachePoint"]);
    }

    #[test]
    fn leaves_a_message_that_was_only_a_cache_point_empty() {
        let config = auto_cache(None);
        let request = format(&config, &[user(vec![cp_block(None)])], &StreamOptions::default());
        assert!(tags(&request, 0).is_empty());
    }

    #[test]
    fn honors_a_cache_point_the_caller_placed_in_the_last_user_message() {
        let config = auto_cache(None);
        let messages = [user(vec![
            ContentBlock::text("durable ask"),
            cp_block(None),
            ContentBlock::text("<context-status>rebuilt each call</context-status>"),
        ])];
        let request = format(&config, &messages, &StreamOptions::default());
        assert_eq!(
            tags(&request, 0),
            [
                "text:durable ask",
                "cachePoint",
                "text:<context-status>rebuilt each call</context-status>",
            ]
        );
    }

    #[test]
    fn keeps_only_the_first_of_three_cache_points_and_gives_it_the_configured_ttl() {
        let config = auto_cache(Some(CacheSection::Ttl("1h".into())));
        let messages = [user(vec![
            ContentBlock::text("durable ask"),
            cp_block(None),
            ContentBlock::text("per-call"),
            cp_block(None),
            ContentBlock::text("more per-call"),
            cp_block(None),
        ])];
        let request = format(&config, &messages, &StreamOptions::default());
        assert_eq!(
            tags(&request, 0),
            [
                "text:durable ask",
                "cachePoint:1h",
                "text:per-call",
                "text:more per-call",
            ]
        );
    }

    #[test]
    fn relocates_an_honored_cache_point_over_the_adjacent_document_run_only() {
        let config = auto_cache(None);
        let messages = [user(vec![
            ContentBlock::text("analyze these"),
            doc_block("earlier", crate::types::DocumentFormat::Md),
            ContentBlock::text("notes"),
            doc_block("adjacent", crate::types::DocumentFormat::Md),
            cp_block(None),
        ])];
        let request = format(&config, &messages, &StreamOptions::default());
        assert_eq!(
            tags(&request, 0),
            [
                "text:analyze these",
                "document:earlier",
                "text:notes",
                "cachePoint",
                "document:adjacent",
            ]
        );
    }

    #[test]
    fn still_strips_cache_points_in_earlier_messages() {
        let config = auto_cache(None);
        let messages = [
            user(vec![ContentBlock::text("old ask"), cp_block(None)]),
            Message::new(Role::Assistant, vec![ContentBlock::text("reply")]),
            user(vec![ContentBlock::text("new ask"), cp_block(None)]),
        ];
        let request = format(&config, &messages, &StreamOptions::default());
        assert_eq!(tags(&request, 0), ["text:old ask"]);
        assert_eq!(tags(&request, 1), ["text:reply"]);
        assert_eq!(tags(&request, 2), ["text:new ask", "cachePoint"]);
    }

    #[test]
    fn applies_the_configured_messages_ttl_to_an_honored_cache_point_without_one() {
        let config = auto_cache(Some(CacheSection::Ttl("1h".into())));
        let messages = [user(vec![
            ContentBlock::text("durable ask"),
            cp_block(None),
            ContentBlock::text("per-call"),
        ])];
        let request = format(&config, &messages, &StreamOptions::default());
        assert_eq!(
            tags(&request, 0),
            ["text:durable ask", "cachePoint:1h", "text:per-call"]
        );
    }

    #[test]
    fn does_not_forward_an_empty_configured_messages_ttl() {
        let config = auto_cache(Some(CacheSection::Ttl(String::new())));
        let messages = [user(vec![
            ContentBlock::text("durable ask"),
            cp_block(Some("1h")),
            ContentBlock::text("per-call"),
        ])];
        let request = format(&config, &messages, &StreamOptions::default());
        assert_eq!(
            tags(&request, 0),
            ["text:durable ask", "cachePoint", "text:per-call"]
        );
    }

    #[test]
    fn normalizes_the_ttl_of_a_relocated_cache_point() {
        let config = auto_cache(Some(CacheSection::Ttl("5m".into())));
        let messages = [user(vec![
            ContentBlock::text("analyze"),
            doc_block("d", crate::types::DocumentFormat::Md),
            cp_block(Some("1h")),
            ContentBlock::text("per-call"),
        ])];
        let request = format(&config, &messages, &StreamOptions::default());
        assert_eq!(
            tags(&request, 0),
            ["text:analyze", "cachePoint:5m", "document:d", "text:per-call"]
        );
    }

    #[test]
    fn normalizes_a_hand_placed_ttl_to_the_configured_one() {
        let config = auto_cache(Some(CacheSection::Ttl("1h".into())));
        let messages = [user(vec![
            ContentBlock::text("durable ask"),
            cp_block(Some("5m")),
            ContentBlock::text("per-call"),
        ])];
        let request = format(&config, &messages, &StreamOptions::default());
        assert_eq!(
            tags(&request, 0),
            ["text:durable ask", "cachePoint:1h", "text:per-call"]
        );
    }

    #[test]
    fn drops_a_hand_placed_ttl_when_no_messages_ttl_is_configured() {
        let config = auto_cache(None);
        let messages = [user(vec![
            ContentBlock::text("durable ask"),
            cp_block(Some("1h")),
            ContentBlock::text("per-call"),
        ])];
        let request = format(&config, &messages, &StreamOptions::default());
        assert_eq!(
            tags(&request, 0),
            ["text:durable ask", "cachePoint", "text:per-call"]
        );
    }

    #[test]
    fn applies_the_configured_messages_ttl_over_a_null_ttl_on_the_caller_point() {
        let config = auto_cache(Some(CacheSection::Ttl("1h".into())));
        let messages = [user(vec![ContentBlock::text("durable ask"), cp_block(None)])];
        let request = format(&config, &messages, &StreamOptions::default());
        assert_eq!(tags(&request, 0), ["text:durable ask", "cachePoint:1h"]);
    }

    #[test]
    fn relocates_an_honored_cache_point_ahead_of_a_directly_preceding_non_pdf_document() {
        let config = auto_cache(None);
        let messages = [user(vec![
            ContentBlock::text("analyze this"),
            doc_block("readme", crate::types::DocumentFormat::Md),
            cp_block(None),
            ContentBlock::text("per-call"),
        ])];
        let request = format(&config, &messages, &StreamOptions::default());
        assert_eq!(
            tags(&request, 0),
            [
                "text:analyze this",
                "cachePoint",
                "document:readme",
                "text:per-call",
            ]
        );
    }

    #[test]
    fn does_not_relocate_an_honored_cache_point_a_document_does_not_directly_precede() {
        let config = auto_cache(None);
        let messages = [user(vec![
            ContentBlock::text("analyze this"),
            doc_block("readme", crate::types::DocumentFormat::Md),
            ContentBlock::text("notes"),
            cp_block(None),
        ])];
        let request = format(&config, &messages, &StreamOptions::default());
        assert_eq!(
            tags(&request, 0),
            ["text:analyze this", "document:readme", "text:notes", "cachePoint",]
        );
    }

    #[test]
    fn leaves_an_honored_cache_point_after_a_pdf_document() {
        let config = auto_cache(None);
        let messages = [user(vec![
            ContentBlock::text("analyze this"),
            doc_block("report", crate::types::DocumentFormat::Pdf),
            cp_block(None),
        ])];
        let request = format(&config, &messages, &StreamOptions::default());
        assert_eq!(
            tags(&request, 0),
            ["text:analyze this", "document:report", "cachePoint"]
        );
    }

    #[test]
    fn drops_an_honored_cache_point_when_a_non_pdf_document_leads_the_message() {
        let config = auto_cache(None);
        let messages = [user(vec![
            doc_block("readme", crate::types::DocumentFormat::Md),
            cp_block(None),
        ])];
        let request = format(&config, &messages, &StreamOptions::default());
        assert_eq!(tags(&request, 0), ["document:readme"]);
    }

    fn tools_cache_ttls(request: &FormattedRequest) -> Vec<Option<String>> {
        request
            .tool_config
            .as_ref()
            .map(|tools| {
                tools
                    .tools()
                    .iter()
                    .filter_map(|tool| tool.as_cache_point().ok())
                    .map(|point| point.ttl().map(|ttl| ttl.as_str().to_owned()))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn with_tools_and_system(system: SystemPrompt) -> StreamOptions {
        StreamOptions {
            tool_specs: vec![tool_spec("t1")],
            system_prompt: Some(system),
            ..Default::default()
        }
    }

    fn system_with_cache_point(ttl: Option<&str>) -> SystemPrompt {
        SystemPrompt::Blocks(vec![
            SystemContentBlock::Text("be brief".into()),
            SystemContentBlock::CachePoint(CachePointBlock {
                cache_type: Default::default(),
                ttl: ttl.map(str::to_owned),
            }),
        ])
    }

    #[test]
    fn caches_every_section_when_no_strategy_is_given() {
        let mut config = claude();
        with_cache(&mut config, CacheConfig::default());
        let options = with_tools_and_system(SystemPrompt::Text("be brief".into()));
        let request = format(&config, &[user(vec![ContentBlock::text("hi")])], &options);
        assert_eq!(tools_cache_ttls(&request), vec![None]);
        assert_eq!(system_cache_ttls(&request), vec![None]);
        assert_eq!(msg_cache_ttls(&request, 0), vec![None]);
    }

    #[test]
    fn caches_only_the_conversation_when_tools_ttl_is_false() {
        let mut config = claude();
        with_cache(
            &mut config,
            CacheConfig {
                tools_ttl: Some(CacheSection::Disabled),
                ..Default::default()
            },
        );
        let options = with_tools_and_system(SystemPrompt::Text("be brief".into()));
        let request = format(&config, &[user(vec![ContentBlock::text("hi")])], &options);
        assert!(tools_cache_ttls(&request).is_empty());
        assert_eq!(msg_cache_ttls(&request, 0), vec![None]);
    }

    #[test]
    fn emits_nothing_when_every_section_is_disabled() {
        let mut config = claude();
        with_cache(
            &mut config,
            CacheConfig {
                tools_ttl: Some(CacheSection::Disabled),
                system_prompt_ttl: Some(CacheSection::Disabled),
                messages_ttl: Some(CacheSection::Disabled),
                ..Default::default()
            },
        );
        let options = with_tools_and_system(SystemPrompt::Text("be brief".into()));
        let request = format(&config, &[user(vec![ContentBlock::text("hi")])], &options);
        assert!(tools_cache_ttls(&request).is_empty());
        assert!(system_cache_ttls(&request).is_empty());
        assert!(msg_cache_ttls(&request, 0).is_empty());
    }

    #[test]
    fn propagates_only_tools_ttl_when_messages_ttl_is_not_set() {
        let mut config = claude();
        with_cache(
            &mut config,
            CacheConfig {
                tools_ttl: Some(CacheSection::Ttl("1h".into())),
                ..Default::default()
            },
        );
        let options = StreamOptions {
            tool_specs: vec![tool_spec("t1")],
            ..Default::default()
        };
        let request = format(&config, &[user(vec![ContentBlock::text("hi")])], &options);
        assert_eq!(tools_cache_ttls(&request), vec![Some("1h".to_owned())]);
        assert_eq!(msg_cache_ttls(&request, 0), vec![None]);
    }

    #[test]
    fn propagates_only_messages_ttl_when_tools_ttl_is_not_set() {
        let mut config = claude();
        with_cache(
            &mut config,
            CacheConfig {
                messages_ttl: Some(CacheSection::Ttl("5m".into())),
                ..Default::default()
            },
        );
        let options = StreamOptions {
            tool_specs: vec![tool_spec("t1")],
            ..Default::default()
        };
        let request = format(&config, &[user(vec![ContentBlock::text("hi")])], &options);
        assert_eq!(tools_cache_ttls(&request), vec![None]);
        assert_eq!(msg_cache_ttls(&request, 0), vec![Some("5m".to_owned())]);
    }

    #[test]
    fn applies_the_shared_ttl_to_a_caller_placed_system_cache_point() {
        let mut config = claude();
        with_cache(
            &mut config,
            CacheConfig {
                ttl: Some("1h".into()),
                ..Default::default()
            },
        );
        let options = StreamOptions {
            system_prompt: Some(system_with_cache_point(None)),
            ..Default::default()
        };
        let request = format(&config, &[user(vec![ContentBlock::text("hi")])], &options);
        assert_eq!(system_cache_ttls(&request), vec![Some("1h".to_owned())]);
    }

    #[test]
    fn leaves_a_system_cache_point_ttl_the_caller_wrote_as_written() {
        let mut config = claude();
        with_cache(
            &mut config,
            CacheConfig {
                ttl: Some("1h".into()),
                ..Default::default()
            },
        );
        let options = StreamOptions {
            system_prompt: Some(system_with_cache_point(Some("5m"))),
            ..Default::default()
        };
        let request = format(&config, &[user(vec![ContentBlock::text("hi")])], &options);
        assert_eq!(system_cache_ttls(&request), vec![Some("5m".to_owned())]);
    }

    #[test]
    fn leaves_a_system_cache_point_alone_when_no_ttl_is_configured() {
        let mut config = claude();
        with_cache(&mut config, CacheConfig::default());
        let options = StreamOptions {
            system_prompt: Some(system_with_cache_point(None)),
            ..Default::default()
        };
        let request = format(&config, &[user(vec![ContentBlock::text("hi")])], &options);
        assert_eq!(system_cache_ttls(&request), vec![None]);
    }

    #[test]
    fn treats_an_empty_shared_ttl_as_unconfigured_for_system_points() {
        let mut config = claude();
        with_cache(
            &mut config,
            CacheConfig {
                ttl: Some(String::new()),
                ..Default::default()
            },
        );
        let options = StreamOptions {
            system_prompt: Some(system_with_cache_point(Some(""))),
            ..Default::default()
        };
        let request = format(&config, &[user(vec![ContentBlock::text("hi")])], &options);
        assert_eq!(system_cache_ttls(&request), vec![None]);
    }

    #[test]
    fn leaves_a_system_cache_point_alone_when_the_model_does_not_support_caching() {
        let mut config = BedrockModelConfig::new("amazon.titan-text-v1");
        with_cache(
            &mut config,
            CacheConfig {
                ttl: Some("1h".into()),
                ..Default::default()
            },
        );
        let options = StreamOptions {
            system_prompt: Some(system_with_cache_point(None)),
            ..Default::default()
        };
        let request = format(&config, &[user(vec![ContentBlock::text("hi")])], &options);
        assert_eq!(system_cache_ttls(&request), vec![None]);
    }

    #[test]
    fn leaves_a_system_cache_point_alone_when_the_tools_checkpoint_ahead_of_it_is_shorter() {
        let mut config = claude();
        with_cache(
            &mut config,
            CacheConfig {
                ttl: Some("1h".into()),
                tools_ttl: Some(CacheSection::Ttl("5m".into())),
                ..Default::default()
            },
        );
        let options = with_tools_and_system(system_with_cache_point(None));
        let request = format(&config, &[user(vec![ContentBlock::text("hi")])], &options);
        assert_eq!(tools_cache_ttls(&request), vec![Some("5m".to_owned())]);
        assert_eq!(system_cache_ttls(&request), vec![None]);
    }

    #[test]
    fn applies_the_shared_ttl_when_the_tools_section_carries_it_too() {
        let mut config = claude();
        with_cache(
            &mut config,
            CacheConfig {
                ttl: Some("1h".into()),
                tools_ttl: Some(CacheSection::Ttl("1h".into())),
                ..Default::default()
            },
        );
        let options = with_tools_and_system(system_with_cache_point(None));
        let request = format(&config, &[user(vec![ContentBlock::text("hi")])], &options);
        assert_eq!(system_cache_ttls(&request), vec![Some("1h".to_owned())]);
    }

    #[test]
    fn applies_the_shared_ttl_when_the_tools_section_is_disabled() {
        let mut config = claude();
        with_cache(
            &mut config,
            CacheConfig {
                ttl: Some("1h".into()),
                tools_ttl: Some(CacheSection::Disabled),
                ..Default::default()
            },
        );
        let options = with_tools_and_system(system_with_cache_point(None));
        let request = format(&config, &[user(vec![ContentBlock::text("hi")])], &options);
        assert!(tools_cache_ttls(&request).is_empty());
        assert_eq!(system_cache_ttls(&request), vec![Some("1h".to_owned())]);
    }

    #[test]
    fn applies_the_shared_ttl_when_the_request_carries_no_tools_at_all() {
        let mut config = claude();
        with_cache(
            &mut config,
            CacheConfig {
                ttl: Some("1h".into()),
                tools_ttl: Some(CacheSection::Ttl("5m".into())),
                ..Default::default()
            },
        );
        let options = StreamOptions {
            system_prompt: Some(system_with_cache_point(None)),
            ..Default::default()
        };
        let request = format(&config, &[user(vec![ContentBlock::text("hi")])], &options);
        assert_eq!(system_cache_ttls(&request), vec![Some("1h".to_owned())]);
    }

    #[test]
    fn does_not_mutate_the_system_prompt_blocks_the_caller_owns() {
        let mut config = claude();
        with_cache(
            &mut config,
            CacheConfig {
                ttl: Some("1h".into()),
                ..Default::default()
            },
        );
        let options = StreamOptions {
            system_prompt: Some(system_with_cache_point(None)),
            ..Default::default()
        };
        let snapshot = options.system_prompt.clone();
        let _ = format(&config, &[user(vec![ContentBlock::text("hi")])], &options);
        assert_eq!(options.system_prompt, snapshot);
    }

    #[test]
    fn places_the_cache_point_ahead_of_every_block_of_a_multi_block_tail() {
        let config = auto_cache(None);
        let messages = [user(vec![
            ContentBlock::text("durable ask"),
            ContentBlock::text("injected"),
            ContentBlock::text("status"),
        ])];
        let options = StreamOptions {
            dynamic_trailing_blocks: Some(2),
            ..Default::default()
        };
        let request = format(&config, &messages, &options);
        assert_eq!(
            tags(&request, 0),
            ["text:durable ask", "cachePoint", "text:injected", "text:status",]
        );
    }

    #[test]
    fn steps_a_per_call_boundary_back_over_a_directly_preceding_non_pdf_document() {
        let config = auto_cache(None);
        let messages = [user(vec![
            ContentBlock::text("analyze this"),
            doc_block("readme", crate::types::DocumentFormat::Md),
            ContentBlock::text("per-call"),
        ])];
        let options = StreamOptions {
            dynamic_trailing_blocks: Some(1),
            ..Default::default()
        };
        let request = format(&config, &messages, &options);
        assert_eq!(
            tags(&request, 0),
            [
                "text:analyze this",
                "cachePoint",
                "document:readme",
                "text:per-call",
            ]
        );
    }

    #[test]
    fn applies_the_configured_messages_ttl_to_a_per_call_boundary() {
        let config = auto_cache(Some(CacheSection::Ttl("1h".into())));
        let messages = [user(vec![
            ContentBlock::text("durable ask"),
            ContentBlock::text("per-call"),
        ])];
        let options = StreamOptions {
            dynamic_trailing_blocks: Some(1),
            ..Default::default()
        };
        let request = format(&config, &messages, &options);
        assert_eq!(
            tags(&request, 0),
            ["text:durable ask", "cachePoint:1h", "text:per-call"]
        );
    }

    fn guarding() -> BedrockModelConfig {
        let mut config = claude();
        config.guardrail_config = Some(BedrockGuardrailConfig {
            guardrail_identifier: "gr-1".into(),
            guardrail_version: "1".into(),
            guard_latest_user_message: Some(true),
            ..Default::default()
        });
        config
    }

    #[test]
    fn wraps_latest_user_message_image_content_in_guard_content_when_enabled() {
        let messages = [user(vec![img_block(
            crate::types::ImageFormat::Png,
            ImageSource::Bytes(vec![1, 2, 3]),
        )])];
        let request = format(&guarding(), &messages, &StreamOptions::default());
        assert_eq!(tags(&request, 0), ["guardImage:png"]);
    }

    #[test]
    fn wraps_supported_image_formats_png_and_jpeg_with_bytes_source() {
        for (image_format, tag) in [
            (crate::types::ImageFormat::Png, "guardImage:png"),
            (crate::types::ImageFormat::Jpeg, "guardImage:jpeg"),
        ] {
            let messages = [user(vec![img_block(image_format, ImageSource::Bytes(vec![1]))])];
            let request = format(&guarding(), &messages, &StreamOptions::default());
            assert_eq!(tags(&request, 0), [tag]);
        }
    }

    #[test]
    fn skips_wrapping_images_with_unsupported_formats() {
        for (image_format, tag) in [
            (crate::types::ImageFormat::Gif, "image:gif"),
            (crate::types::ImageFormat::Webp, "image:webp"),
        ] {
            let messages = [user(vec![img_block(image_format, ImageSource::Bytes(vec![1]))])];
            let request = format(&guarding(), &messages, &StreamOptions::default());
            assert_eq!(tags(&request, 0), [tag]);
        }
    }

    #[test]
    fn skips_wrapping_images_with_a_non_bytes_source() {
        let sources = [
            ImageSource::Location(crate::types::S3Location {
                uri: "s3://bucket/key.png".into(),
                bucket_owner: None,
            }),
            ImageSource::Url("s3://bucket/key.png".into()),
        ];
        for source in sources {
            let messages = [user(vec![img_block(crate::types::ImageFormat::Png, source)])];
            let request = format(&guarding(), &messages, &StreamOptions::default());
            assert_eq!(tags(&request, 0), ["image:png"]);
        }
    }

    #[test]
    fn does_not_wrap_assistant_messages() {
        let messages = [
            user(vec![ContentBlock::text("ask")]),
            Message::new(Role::Assistant, vec![ContentBlock::text("reply")]),
        ];
        let request = format(&guarding(), &messages, &StreamOptions::default());
        assert_eq!(tags(&request, 0), ["guardText:ask"]);
        assert_eq!(tags(&request, 1), ["text:reply"]);
    }

    #[test]
    fn wraps_only_the_last_user_text_or_image_message_in_multi_turn_conversation() {
        let messages = [
            user(vec![ContentBlock::text("older")]),
            Message::new(Role::Assistant, vec![ContentBlock::text("reply")]),
            user(vec![ContentBlock::text("newer")]),
        ];
        let request = format(&guarding(), &messages, &StreamOptions::default());
        assert_eq!(tags(&request, 0), ["text:older"]);
        assert_eq!(tags(&request, 2), ["guardText:newer"]);
    }

    #[test]
    fn wraps_all_text_and_image_blocks_in_the_latest_user_message() {
        let messages = [user(vec![
            ContentBlock::text("first"),
            img_block(crate::types::ImageFormat::Png, ImageSource::Bytes(vec![1])),
            ContentBlock::text("second"),
        ])];
        let request = format(&guarding(), &messages, &StreamOptions::default());
        assert_eq!(
            tags(&request, 0),
            ["guardText:first", "guardImage:png", "guardText:second"]
        );
    }

    #[test]
    fn handles_no_user_messages_with_text_or_image_content_gracefully() {
        let messages = [user(vec![ContentBlock::ToolResult(ToolResultBlock::success(
            "t1",
            vec![ToolResultContent::Text("done".into())],
        ))])];
        let request = format(&guarding(), &messages, &StreamOptions::default());
        assert_eq!(tags(&request, 0), ["toolResult"]);
    }

    #[test]
    fn preserves_explicit_guard_content_block_in_messages_without_double_wrapping() {
        let messages = [user(vec![
            ContentBlock::GuardContent(GuardContentBlock {
                text: Some(crate::types::GuardContentText {
                    qualifiers: vec![],
                    text: "grounding".into(),
                }),
                image: None,
            }),
            ContentBlock::text("ask"),
        ])];
        let request = format(&guarding(), &messages, &StreamOptions::default());
        assert_eq!(tags(&request, 0), ["guardText:grounding", "guardText:ask"]);
    }

    #[test]
    fn does_not_wrap_reasoning_or_cache_point_blocks() {
        let messages = [user(vec![
            ContentBlock::text("ask"),
            ContentBlock::Reasoning(ReasoningBlock {
                text: Some("thinking".into()),
                signature: Some("sig".into()),
                redacted_content: None,
            }),
            cp_block(None),
        ])];
        let request = format(&guarding(), &messages, &StreamOptions::default());
        assert_eq!(tags(&request, 0), ["guardText:ask", "reasoning", "cachePoint"]);
    }
}
