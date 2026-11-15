//! The SDK side of the transport: Converse JSON shapes to and from `aws-sdk-bedrockruntime`
//! types, and SDK errors classified through their typed variants.

use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;
use aws_sdk_bedrockruntime::Client;
use aws_sdk_bedrockruntime::error::{DisplayErrorContext, ProvideErrorMetadata, SdkError};
use aws_sdk_bedrockruntime::types as bt;
use aws_smithy_types::{Blob, Document};
use serde_json::json;

use super::document::{document_to_json, json_to_document};
use super::transport::{BedrockError, BedrockErrorKind, EventStream, Operation, Response, Transport};
use crate::types::json::{JsonObject, JsonValue, to_js_string};
use crate::types::media::{decode_base64, encode_base64};

type WireResult<T> = std::result::Result<T, BedrockError>;

fn invalid(message: impl Into<String>) -> BedrockError {
    BedrockError::service("ValidationException", message)
}

fn built<T, E: std::fmt::Display>(result: std::result::Result<T, E>) -> WireResult<T> {
    result.map_err(|error| invalid(format!("invalid Bedrock request: {error}")))
}

fn field<'a>(value: &'a JsonValue, key: &str) -> Option<&'a JsonValue> {
    value.get(key).filter(|value| !value.is_null())
}

fn string(value: &JsonValue, key: &str) -> Option<String> {
    field(value, key)
        .and_then(JsonValue::as_str)
        .map(ToOwned::to_owned)
}

fn required_string(value: &JsonValue, key: &str, what: &str) -> WireResult<String> {
    string(value, key).ok_or_else(|| invalid(format!("{what}.{key} must be a string")))
}

fn int(value: &JsonValue, key: &str) -> Option<i32> {
    field(value, key)
        .and_then(JsonValue::as_i64)
        .and_then(|number| i32::try_from(number).ok())
}

#[expect(clippy::cast_possible_truncation, reason = "Bedrock takes 32-bit floats")]
fn float(value: &JsonValue, key: &str) -> Option<f32> {
    field(value, key)
        .and_then(JsonValue::as_f64)
        .map(|number| number as f32)
}

fn array<'a>(value: &'a JsonValue, key: &str) -> &'a [JsonValue] {
    field(value, key)
        .and_then(JsonValue::as_array)
        .map_or(&[], Vec::as_slice)
}

fn strings(value: &JsonValue, key: &str) -> Option<Vec<String>> {
    field(value, key).and_then(JsonValue::as_array).map(|items| {
        items
            .iter()
            .filter_map(JsonValue::as_str)
            .map(ToOwned::to_owned)
            .collect()
    })
}

fn blob(value: &JsonValue) -> Blob {
    Blob::new(value.as_str().map(decode_base64).unwrap_or_default())
}

fn single(value: &JsonValue) -> Option<(&str, &JsonValue)> {
    value
        .as_object()
        .and_then(|object| object.iter().next())
        .map(|(key, value)| (key.as_str(), value))
}

/// Converts JSON values to `Document`s, reusing the `Document` built for an identical value in the
/// previous request, so a `HashMap`-backed object keeps its iteration order (and the request its
/// cache prefix) from one call to the next.
#[derive(Debug, Default)]
struct DocumentCache {
    previous: HashMap<String, Document>,
    current: HashMap<String, Document>,
}

impl DocumentCache {
    fn get(&mut self, value: &JsonValue) -> Document {
        let Ok(key) = to_js_string(value) else {
            return json_to_document(value);
        };
        if let Some(document) = self.current.get(&key) {
            return document.clone();
        }
        let document = self
            .previous
            .remove(&key)
            .unwrap_or_else(|| json_to_document(value));
        self.current.insert(key, document.clone());
        document
    }

    fn finish(&mut self) {
        self.previous = std::mem::take(&mut self.current);
    }
}

/// The parts of a Converse request, shared by the `Converse` and `ConverseStream` builders.
struct SdkRequest {
    model_id: Option<String>,
    messages: Option<Vec<bt::Message>>,
    system: Option<Vec<bt::SystemContentBlock>>,
    tool_config: Option<bt::ToolConfiguration>,
    inference_config: Option<bt::InferenceConfiguration>,
    additional_model_request_fields: Option<Document>,
    additional_model_response_field_paths: Option<Vec<String>>,
    guardrail: Option<JsonValue>,
    output_config: Option<bt::OutputConfig>,
    prompt_variables: Option<HashMap<String, bt::PromptVariableValues>>,
    request_metadata: Option<HashMap<String, String>>,
    performance_config: Option<bt::PerformanceConfiguration>,
    service_tier: Option<bt::ServiceTier>,
}

const REQUEST_KEYS: [&str; 13] = [
    "modelId",
    "messages",
    "system",
    "toolConfig",
    "inferenceConfig",
    "additionalModelRequestFields",
    "additionalModelResponseFieldPaths",
    "guardrailConfig",
    "outputConfig",
    "promptVariables",
    "requestMetadata",
    "performanceConfig",
    "serviceTier",
];

struct Converter<'a> {
    documents: &'a mut DocumentCache,
}

impl Converter<'_> {
    fn request(&mut self, request: &JsonObject) -> WireResult<SdkRequest> {
        if let Some(key) = request.keys().find(|key| !REQUEST_KEYS.contains(&key.as_str())) {
            return Err(invalid(format!(
                "key=<{key}> | additionalArgs key is not a Converse request field"
            )));
        }
        let value = JsonValue::Object(request.clone());
        let messages = array(&value, "messages")
            .iter()
            .map(|message| self.message(message))
            .collect::<WireResult<Vec<_>>>()?;
        let system = field(&value, "system")
            .map(|_| {
                array(&value, "system")
                    .iter()
                    .map(|block| self.system_block(block))
                    .collect::<WireResult<Vec<_>>>()
            })
            .transpose()?;
        let tool_config = field(&value, "toolConfig")
            .map(|config| self.tool_config(config))
            .transpose()?;
        let inference_config = field(&value, "inferenceConfig").map(|config| {
            bt::InferenceConfiguration::builder()
                .set_max_tokens(int(config, "maxTokens"))
                .set_temperature(float(config, "temperature"))
                .set_top_p(float(config, "topP"))
                .set_stop_sequences(strings(config, "stopSequences"))
                .build()
        });
        Ok(SdkRequest {
            model_id: string(&value, "modelId"),
            messages: Some(messages),
            system,
            tool_config,
            inference_config,
            additional_model_request_fields: field(&value, "additionalModelRequestFields")
                .map(|fields| self.documents.get(fields)),
            additional_model_response_field_paths: strings(&value, "additionalModelResponseFieldPaths"),
            guardrail: field(&value, "guardrailConfig").cloned(),
            output_config: field(&value, "outputConfig").map(output_config).transpose()?,
            prompt_variables: field(&value, "promptVariables").map(prompt_variables),
            request_metadata: field(&value, "requestMetadata").map(request_metadata),
            performance_config: field(&value, "performanceConfig")
                .and_then(|config| string(config, "latency"))
                .map(|latency| {
                    bt::PerformanceConfiguration::builder()
                        .latency(bt::PerformanceConfigLatency::from(latency.as_str()))
                        .build()
                }),
            service_tier: field(&value, "serviceTier")
                .and_then(|tier| string(tier, "type"))
                .map(|tier| {
                    built(
                        bt::ServiceTier::builder()
                            .r#type(bt::ServiceTierType::from(tier.as_str()))
                            .build(),
                    )
                })
                .transpose()?,
        })
    }

    fn message(&mut self, message: &JsonValue) -> WireResult<bt::Message> {
        let role = required_string(message, "role", "message")?;
        let content = array(message, "content")
            .iter()
            .map(|block| self.content_block(block))
            .collect::<WireResult<Vec<_>>>()?;
        built(
            bt::Message::builder()
                .role(bt::ConversationRole::from(role.as_str()))
                .set_content(Some(content))
                .build(),
        )
    }

    fn content_block(&mut self, block: &JsonValue) -> WireResult<bt::ContentBlock> {
        let Some((key, value)) = single(block) else {
            return Err(invalid("content block must be an object with one key"));
        };
        Ok(match key {
            "text" => bt::ContentBlock::Text(value.as_str().unwrap_or_default().to_owned()),
            "image" => bt::ContentBlock::Image(image(value)?),
            "document" => bt::ContentBlock::Document(document(value)?),
            "video" => bt::ContentBlock::Video(video(value)?),
            "audio" => bt::ContentBlock::Audio(audio(value)?),
            "toolUse" => bt::ContentBlock::ToolUse(built(
                bt::ToolUseBlock::builder()
                    .tool_use_id(required_string(value, "toolUseId", "toolUse")?)
                    .name(required_string(value, "name", "toolUse")?)
                    .input(
                        self.documents
                            .get(field(value, "input").unwrap_or(&JsonValue::Null)),
                    )
                    .build(),
            )?),
            "toolResult" => bt::ContentBlock::ToolResult(self.tool_result(value)?),
            "guardContent" => bt::ContentBlock::GuardContent(guard_content(value)?),
            "cachePoint" => bt::ContentBlock::CachePoint(cache_point(value)?),
            "reasoningContent" => bt::ContentBlock::ReasoningContent(reasoning(value)?),
            "citationsContent" => bt::ContentBlock::CitationsContent(citations_content(value)),
            other => return Err(invalid(format!("unsupported content block `{other}`"))),
        })
    }

    fn tool_result(&mut self, value: &JsonValue) -> WireResult<bt::ToolResultBlock> {
        let content = array(value, "content")
            .iter()
            .map(|item| {
                let Some((key, item)) = single(item) else {
                    return Err(invalid("tool result content must be an object with one key"));
                };
                Ok(match key {
                    "text" => bt::ToolResultContentBlock::Text(item.as_str().unwrap_or_default().to_owned()),
                    "json" => bt::ToolResultContentBlock::Json(self.documents.get(item)),
                    "image" => bt::ToolResultContentBlock::Image(image(item)?),
                    "video" => bt::ToolResultContentBlock::Video(video(item)?),
                    "document" => bt::ToolResultContentBlock::Document(document(item)?),
                    other => return Err(invalid(format!("unsupported tool result content `{other}`"))),
                })
            })
            .collect::<WireResult<Vec<_>>>()?;
        built(
            bt::ToolResultBlock::builder()
                .tool_use_id(required_string(value, "toolUseId", "toolResult")?)
                .set_content(Some(content))
                .set_status(string(value, "status").map(|status| bt::ToolResultStatus::from(status.as_str())))
                .build(),
        )
    }

    fn system_block(&mut self, block: &JsonValue) -> WireResult<bt::SystemContentBlock> {
        let Some((key, value)) = single(block) else {
            return Err(invalid("system block must be an object with one key"));
        };
        Ok(match key {
            "text" => bt::SystemContentBlock::Text(value.as_str().unwrap_or_default().to_owned()),
            "guardContent" => bt::SystemContentBlock::GuardContent(guard_content(value)?),
            "cachePoint" => bt::SystemContentBlock::CachePoint(cache_point(value)?),
            other => return Err(invalid(format!("unsupported system block `{other}`"))),
        })
    }

    fn tool_config(&mut self, config: &JsonValue) -> WireResult<bt::ToolConfiguration> {
        let tools = array(config, "tools")
            .iter()
            .map(|tool| match single(tool) {
                Some(("toolSpec", spec)) => {
                    let schema = field(spec, "inputSchema")
                        .and_then(|schema| field(schema, "json"))
                        .map(|schema| bt::ToolInputSchema::Json(self.documents.get(schema)));
                    Ok(bt::Tool::ToolSpec(built(
                        bt::ToolSpecification::builder()
                            .name(required_string(spec, "name", "toolSpec")?)
                            .set_description(string(spec, "description"))
                            .set_input_schema(schema)
                            .set_strict(field(spec, "strict").and_then(JsonValue::as_bool))
                            .build(),
                    )?))
                }
                Some(("cachePoint", point)) => Ok(bt::Tool::CachePoint(cache_point(point)?)),
                _ => Err(invalid("unsupported tool entry")),
            })
            .collect::<WireResult<Vec<_>>>()?;
        let tool_choice = field(config, "toolChoice")
            .map(|choice| match single(choice) {
                Some(("auto", _)) => Ok(bt::ToolChoice::Auto(bt::AutoToolChoice::builder().build())),
                Some(("any", _)) => Ok(bt::ToolChoice::Any(bt::AnyToolChoice::builder().build())),
                Some(("tool", tool)) => Ok(bt::ToolChoice::Tool(built(
                    bt::SpecificToolChoice::builder()
                        .name(required_string(tool, "name", "toolChoice.tool")?)
                        .build(),
                )?)),
                _ => Err(invalid("unsupported tool choice")),
            })
            .transpose()?;
        built(
            bt::ToolConfiguration::builder()
                .set_tools(Some(tools))
                .set_tool_choice(tool_choice)
                .build(),
        )
    }
}

fn s3_location(value: &JsonValue) -> WireResult<bt::S3Location> {
    built(
        bt::S3Location::builder()
            .uri(required_string(value, "uri", "s3Location")?)
            .set_bucket_owner(string(value, "bucketOwner"))
            .build(),
    )
}

fn image(value: &JsonValue) -> WireResult<bt::ImageBlock> {
    let source = match field(value, "source").and_then(single) {
        Some(("bytes", bytes)) => Some(bt::ImageSource::Bytes(blob(bytes))),
        Some(("s3Location", location)) => Some(bt::ImageSource::S3Location(s3_location(location)?)),
        _ => None,
    };
    built(
        bt::ImageBlock::builder()
            .format(bt::ImageFormat::from(
                required_string(value, "format", "image")?.as_str(),
            ))
            .set_source(source)
            .build(),
    )
}

fn video(value: &JsonValue) -> WireResult<bt::VideoBlock> {
    let source = match field(value, "source").and_then(single) {
        Some(("bytes", bytes)) => Some(bt::VideoSource::Bytes(blob(bytes))),
        Some(("s3Location", location)) => Some(bt::VideoSource::S3Location(s3_location(location)?)),
        _ => None,
    };
    built(
        bt::VideoBlock::builder()
            .format(bt::VideoFormat::from(
                required_string(value, "format", "video")?.as_str(),
            ))
            .set_source(source)
            .build(),
    )
}

fn audio(value: &JsonValue) -> WireResult<bt::AudioBlock> {
    let source = match field(value, "source").and_then(single) {
        Some(("bytes", bytes)) => Some(bt::AudioSource::Bytes(blob(bytes))),
        Some(("s3Location", location)) => Some(bt::AudioSource::S3Location(s3_location(location)?)),
        _ => None,
    };
    built(
        bt::AudioBlock::builder()
            .format(bt::AudioFormat::from(
                required_string(value, "format", "audio")?.as_str(),
            ))
            .set_source(source)
            .build(),
    )
}

fn document(value: &JsonValue) -> WireResult<bt::DocumentBlock> {
    let source = match field(value, "source").and_then(single) {
        Some(("bytes", bytes)) => Some(bt::DocumentSource::Bytes(blob(bytes))),
        Some(("text", text)) => Some(bt::DocumentSource::Text(
            text.as_str().unwrap_or_default().to_owned(),
        )),
        Some(("content", blocks)) => Some(bt::DocumentSource::Content(
            blocks
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .map(|block| bt::DocumentContentBlock::Text(string(block, "text").unwrap_or_default()))
                .collect(),
        )),
        Some(("s3Location", location)) => Some(bt::DocumentSource::S3Location(s3_location(location)?)),
        _ => None,
    };
    let citations = field(value, "citations")
        .map(|citations| {
            built(
                bt::CitationsConfig::builder()
                    .enabled(
                        field(citations, "enabled")
                            .and_then(JsonValue::as_bool)
                            .unwrap_or(false),
                    )
                    .build(),
            )
        })
        .transpose()?;
    built(
        bt::DocumentBlock::builder()
            .set_format(string(value, "format").map(|format| bt::DocumentFormat::from(format.as_str())))
            .name(required_string(value, "name", "document")?)
            .set_source(source)
            .set_context(string(value, "context"))
            .set_citations(citations)
            .build(),
    )
}

fn guard_content(value: &JsonValue) -> WireResult<bt::GuardrailConverseContentBlock> {
    if let Some(text) = field(value, "text") {
        let qualifiers = strings(text, "qualifiers").map(|qualifiers| {
            qualifiers
                .iter()
                .map(|qualifier| bt::GuardrailConverseContentQualifier::from(qualifier.as_str()))
                .collect()
        });
        return Ok(bt::GuardrailConverseContentBlock::Text(built(
            bt::GuardrailConverseTextBlock::builder()
                .text(required_string(text, "text", "guardContent.text")?)
                .set_qualifiers(qualifiers)
                .build(),
        )?));
    }
    if let Some(image) = field(value, "image") {
        let bytes = field(image, "source").and_then(|source| field(source, "bytes"));
        return Ok(bt::GuardrailConverseContentBlock::Image(built(
            bt::GuardrailConverseImageBlock::builder()
                .format(bt::GuardrailConverseImageFormat::from(
                    required_string(image, "format", "guardContent.image")?.as_str(),
                ))
                .set_source(bytes.map(|bytes| bt::GuardrailConverseImageSource::Bytes(blob(bytes))))
                .build(),
        )?));
    }
    Err(invalid("guardContent must have either text or image"))
}

fn cache_point(value: &JsonValue) -> WireResult<bt::CachePointBlock> {
    built(
        bt::CachePointBlock::builder()
            .r#type(bt::CachePointType::from(
                string(value, "type").as_deref().unwrap_or("default"),
            ))
            .set_ttl(string(value, "ttl").map(|ttl| bt::CacheTtl::from(ttl.as_str())))
            .build(),
    )
}

fn reasoning(value: &JsonValue) -> WireResult<bt::ReasoningContentBlock> {
    if let Some(text) = field(value, "reasoningText") {
        return Ok(bt::ReasoningContentBlock::ReasoningText(built(
            bt::ReasoningTextBlock::builder()
                .text(string(text, "text").unwrap_or_default())
                .set_signature(string(text, "signature"))
                .build(),
        )?));
    }
    match field(value, "redactedContent") {
        Some(redacted) => Ok(bt::ReasoningContentBlock::RedactedContent(blob(redacted))),
        None => Err(invalid(
            "reasoningContent must have reasoningText or redactedContent",
        )),
    }
}

fn citation_location(value: &JsonValue) -> Option<bt::CitationLocation> {
    let (key, loc) = single(value)?;
    Some(match key {
        "documentChar" => bt::CitationLocation::DocumentChar(
            bt::DocumentCharLocation::builder()
                .set_document_index(int(loc, "documentIndex"))
                .set_start(int(loc, "start"))
                .set_end(int(loc, "end"))
                .build(),
        ),
        "documentPage" => bt::CitationLocation::DocumentPage(
            bt::DocumentPageLocation::builder()
                .set_document_index(int(loc, "documentIndex"))
                .set_start(int(loc, "start"))
                .set_end(int(loc, "end"))
                .build(),
        ),
        "documentChunk" => bt::CitationLocation::DocumentChunk(
            bt::DocumentChunkLocation::builder()
                .set_document_index(int(loc, "documentIndex"))
                .set_start(int(loc, "start"))
                .set_end(int(loc, "end"))
                .build(),
        ),
        "searchResultLocation" => bt::CitationLocation::SearchResultLocation(
            bt::SearchResultLocation::builder()
                .set_search_result_index(int(loc, "searchResultIndex"))
                .set_start(int(loc, "start"))
                .set_end(int(loc, "end"))
                .build(),
        ),
        "web" => bt::CitationLocation::Web(
            bt::WebLocation::builder()
                .set_url(string(loc, "url"))
                .set_domain(string(loc, "domain"))
                .build(),
        ),
        _ => return None,
    })
}

fn citations_content(value: &JsonValue) -> bt::CitationsContentBlock {
    let citations = array(value, "citations")
        .iter()
        .map(|citation| {
            bt::Citation::builder()
                .set_title(string(citation, "title"))
                .set_source(string(citation, "source"))
                .set_source_content(Some(
                    array(citation, "sourceContent")
                        .iter()
                        .map(|content| {
                            bt::CitationSourceContent::Text(string(content, "text").unwrap_or_default())
                        })
                        .collect(),
                ))
                .set_location(field(citation, "location").and_then(citation_location))
                .build()
        })
        .collect();
    let content = array(value, "content")
        .iter()
        .map(|content| bt::CitationGeneratedContent::Text(string(content, "text").unwrap_or_default()))
        .collect();
    bt::CitationsContentBlock::builder()
        .set_citations(Some(citations))
        .set_content(Some(content))
        .build()
}

fn output_config(value: &JsonValue) -> WireResult<bt::OutputConfig> {
    let text_format = field(value, "textFormat")
        .map(|format| {
            let structure = field(format, "structure")
                .and_then(|structure| field(structure, "jsonSchema"))
                .map(|schema| {
                    built(
                        bt::JsonSchemaDefinition::builder()
                            .schema(string(schema, "schema").unwrap_or_default())
                            .set_name(string(schema, "name"))
                            .set_description(string(schema, "description"))
                            .build(),
                    )
                    .map(bt::OutputFormatStructure::JsonSchema)
                })
                .transpose()?;
            built(
                bt::OutputFormat::builder()
                    .r#type(bt::OutputFormatType::from(
                        string(format, "type").as_deref().unwrap_or("json_schema"),
                    ))
                    .set_structure(structure)
                    .build(),
            )
        })
        .transpose()?;
    Ok(bt::OutputConfig::builder().set_text_format(text_format).build())
}

fn prompt_variables(value: &JsonValue) -> HashMap<String, bt::PromptVariableValues> {
    value
        .as_object()
        .map(|variables| {
            variables
                .iter()
                .map(|(name, variable)| {
                    let text = string(variable, "text").unwrap_or_default();
                    (name.clone(), bt::PromptVariableValues::Text(text))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn request_metadata(value: &JsonValue) -> HashMap<String, String> {
    value
        .as_object()
        .map(|metadata| {
            metadata
                .iter()
                .filter_map(|(key, value)| value.as_str().map(|value| (key.clone(), value.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

macro_rules! apply_request {
    ($builder:expr, $request:expr) => {{
        let request = $request;
        $builder
            .set_model_id(request.model_id)
            .set_messages(request.messages)
            .set_system(request.system)
            .set_tool_config(request.tool_config)
            .set_inference_config(request.inference_config)
            .set_additional_model_request_fields(request.additional_model_request_fields)
            .set_additional_model_response_field_paths(request.additional_model_response_field_paths)
            .set_output_config(request.output_config)
            .set_prompt_variables(request.prompt_variables)
            .set_request_metadata(request.request_metadata)
            .set_performance_config(request.performance_config)
            .set_service_tier(request.service_tier)
    }};
}

fn guardrail_trace(guardrail: &JsonValue) -> Option<bt::GuardrailTrace> {
    string(guardrail, "trace").map(|trace| bt::GuardrailTrace::from(trace.as_str()))
}

fn stream_guardrail(guardrail: Option<&JsonValue>) -> Option<bt::GuardrailStreamConfiguration> {
    let guardrail = guardrail?;
    Some(
        bt::GuardrailStreamConfiguration::builder()
            .set_guardrail_identifier(string(guardrail, "guardrailIdentifier"))
            .set_guardrail_version(string(guardrail, "guardrailVersion"))
            .set_trace(guardrail_trace(guardrail))
            .set_stream_processing_mode(
                string(guardrail, "streamProcessingMode")
                    .map(|mode| bt::GuardrailStreamProcessingMode::from(mode.as_str())),
            )
            .build(),
    )
}

fn converse_guardrail(guardrail: Option<&JsonValue>) -> Option<bt::GuardrailConfiguration> {
    let guardrail = guardrail?;
    Some(
        bt::GuardrailConfiguration::builder()
            .set_guardrail_identifier(string(guardrail, "guardrailIdentifier"))
            .set_guardrail_version(string(guardrail, "guardrailVersion"))
            .set_trace(guardrail_trace(guardrail))
            .build(),
    )
}

/// The exception name and message of a modeled error variant; `None` for an unmodeled one.
trait TypedError {
    fn typed(&self) -> Option<(&'static str, Option<&str>)>;
}

macro_rules! typed_error {
    ($error:ty { $($variant:ident),+ $(,)? }) => {
        impl TypedError for $error {
            fn typed(&self) -> Option<(&'static str, Option<&str>)> {
                match self {
                    $(Self::$variant(inner) => Some((stringify!($variant), inner.message())),)+
                    _ => None,
                }
            }
        }
    };
}

typed_error!(
    aws_sdk_bedrockruntime::operation::converse_stream::ConverseStreamError {
        AccessDeniedException,
        InternalServerException,
        ModelErrorException,
        ModelNotReadyException,
        ModelTimeoutException,
        ResourceNotFoundException,
        ServiceUnavailableException,
        ThrottlingException,
        ValidationException,
        ModelStreamErrorException,
    }
);

typed_error!(aws_sdk_bedrockruntime::operation::converse::ConverseError {
    AccessDeniedException,
    InternalServerException,
    ModelErrorException,
    ModelNotReadyException,
    ModelTimeoutException,
    ResourceNotFoundException,
    ServiceUnavailableException,
    ThrottlingException,
    ValidationException,
});

typed_error!(
    aws_sdk_bedrockruntime::operation::count_tokens::CountTokensError {
        AccessDeniedException,
        InternalServerException,
        ResourceNotFoundException,
        ServiceUnavailableException,
        ThrottlingException,
        ValidationException,
    }
);

typed_error!(bt::error::ConverseStreamOutputError {
    InternalServerException,
    ModelStreamErrorException,
    ValidationException,
    ThrottlingException,
    ServiceUnavailableException,
});

/// Classifies an SDK failure: the exception through its typed variant (throttling becomes
/// [`BedrockErrorKind::Throttled`]), transport failures as network errors, everything else as a
/// service error.
fn classify<E, R>(error: SdkError<E, R>) -> BedrockError
where
    E: TypedError + ProvideErrorMetadata + std::error::Error + Send + Sync + 'static,
    R: Debug + Send + Sync + 'static,
{
    let context = DisplayErrorContext(&error).to_string();
    let (kind, name, message) = match &error {
        SdkError::ServiceError(service) => {
            let err = service.err();
            let (name, message) = match err.typed() {
                Some((name, message)) => (name.to_owned(), message.or_else(|| err.message())),
                None => (err.code().unwrap_or("Error").to_owned(), err.message()),
            };
            let kind = if name == "ThrottlingException" {
                BedrockErrorKind::Throttled
            } else {
                BedrockErrorKind::Service
            };
            (
                kind,
                name,
                message.map_or_else(|| context.clone(), ToOwned::to_owned),
            )
        }
        SdkError::DispatchFailure(_) | SdkError::TimeoutError(_) => {
            (BedrockErrorKind::Network, "NetworkError".to_owned(), context)
        }
        _ => (BedrockErrorKind::Service, "Error".to_owned(), context),
    };
    BedrockError {
        kind,
        name,
        message,
        source: Some(Box::new(error)),
    }
}

fn usage_json(usage: &bt::TokenUsage) -> JsonValue {
    let mut out = JsonObject::new();
    out.insert("inputTokens".to_owned(), json!(usage.input_tokens()));
    out.insert("outputTokens".to_owned(), json!(usage.output_tokens()));
    out.insert("totalTokens".to_owned(), json!(usage.total_tokens()));
    if let Some(tokens) = usage.cache_read_input_tokens() {
        out.insert("cacheReadInputTokens".to_owned(), json!(tokens));
    }
    if let Some(tokens) = usage.cache_write_input_tokens() {
        out.insert("cacheWriteInputTokens".to_owned(), json!(tokens));
    }
    JsonValue::Object(out)
}

fn insert_opt(out: &mut JsonObject, key: &str, value: Option<JsonValue>) {
    if let Some(value) = value {
        out.insert(key.to_owned(), value);
    }
}

fn assessment_json(assessment: &bt::GuardrailAssessment) -> JsonValue {
    let mut out = JsonObject::new();
    insert_opt(
        &mut out,
        "topicPolicy",
        assessment.topic_policy().map(|policy| {
            json!({ "topics": policy.topics().iter().map(|topic| json!({
                "name": topic.name(),
                "type": topic.r#type().as_str(),
                "action": topic.action().as_str(),
                "detected": topic.detected(),
            })).collect::<Vec<_>>() })
        }),
    );
    insert_opt(
        &mut out,
        "contentPolicy",
        assessment.content_policy().map(|policy| {
            json!({ "filters": policy.filters().iter().map(|filter| json!({
                "type": filter.r#type().as_str(),
                "confidence": filter.confidence().as_str(),
                "filterStrength": filter.filter_strength().map(bt::GuardrailContentFilterStrength::as_str),
                "action": filter.action().as_str(),
                "detected": filter.detected(),
            })).collect::<Vec<_>>() })
        }),
    );
    insert_opt(
        &mut out,
        "wordPolicy",
        assessment.word_policy().map(|policy| {
            json!({
                "customWords": policy.custom_words().iter().map(|word| json!({
                    "match": word.r#match(), "action": word.action().as_str(), "detected": word.detected(),
                })).collect::<Vec<_>>(),
                "managedWordLists": policy.managed_word_lists().iter().map(|word| json!({
                    "match": word.r#match(),
                    "type": word.r#type().as_str(),
                    "action": word.action().as_str(),
                    "detected": word.detected(),
                })).collect::<Vec<_>>(),
            })
        }),
    );
    insert_opt(
        &mut out,
        "sensitiveInformationPolicy",
        assessment.sensitive_information_policy().map(|policy| {
            json!({
                "piiEntities": policy.pii_entities().iter().map(|entity| json!({
                    "match": entity.r#match(),
                    "type": entity.r#type().as_str(),
                    "action": entity.action().as_str(),
                    "detected": entity.detected(),
                })).collect::<Vec<_>>(),
                "regexes": policy.regexes().iter().map(|regex| json!({
                    "name": regex.name(),
                    "match": regex.r#match(),
                    "regex": regex.regex(),
                    "action": regex.action().as_str(),
                    "detected": regex.detected(),
                })).collect::<Vec<_>>(),
            })
        }),
    );
    insert_opt(
        &mut out,
        "contextualGroundingPolicy",
        assessment.contextual_grounding_policy().map(|policy| {
            json!({ "filters": policy.filters().iter().map(|filter| json!({
                "type": filter.r#type().as_str(),
                "threshold": filter.threshold(),
                "score": filter.score(),
                "action": filter.action().as_str(),
                "detected": filter.detected(),
            })).collect::<Vec<_>>() })
        }),
    );
    insert_opt(
        &mut out,
        "invocationMetrics",
        assessment
            .invocation_metrics()
            .map(|metrics| json!({ "guardrailProcessingLatency": metrics.guardrail_processing_latency() })),
    );
    insert_opt(
        &mut out,
        "appliedGuardrailDetails",
        assessment.applied_guardrail_details().map(|details| {
            json!({
                "guardrailId": details.guardrail_id(),
                "guardrailVersion": details.guardrail_version(),
                "guardrailArn": details.guardrail_arn(),
            })
        }),
    );
    strip_nulls(JsonValue::Object(out))
}

fn strip_nulls(value: JsonValue) -> JsonValue {
    match value {
        JsonValue::Object(map) => JsonValue::Object(
            map.into_iter()
                .filter(|(_, value)| !value.is_null())
                .map(|(key, value)| (key, strip_nulls(value)))
                .collect(),
        ),
        JsonValue::Array(items) => JsonValue::Array(items.into_iter().map(strip_nulls).collect()),
        other => other,
    }
}

fn sorted<V>(map: &HashMap<String, V>) -> Vec<(&String, &V)> {
    let mut entries: Vec<_> = map.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    entries
}

/// The JSON form of a guardrail trace assessment. Automated reasoning findings and the usage and
/// coverage parts of the invocation metrics are left out.
pub(crate) fn guardrail_trace_json(trace: &bt::GuardrailTraceAssessment) -> JsonValue {
    let mut out = JsonObject::new();
    if trace.model_output.is_some() {
        out.insert("modelOutput".to_owned(), json!(trace.model_output()));
    }
    if let Some(input) = trace.input_assessment() {
        out.insert(
            "inputAssessment".to_owned(),
            JsonValue::Object(
                sorted(input)
                    .into_iter()
                    .map(|(id, assessment)| (id.clone(), assessment_json(assessment)))
                    .collect(),
            ),
        );
    }
    if let Some(outputs) = trace.output_assessments() {
        out.insert(
            "outputAssessments".to_owned(),
            JsonValue::Object(
                sorted(outputs)
                    .into_iter()
                    .map(|(id, assessments)| {
                        (
                            id.clone(),
                            JsonValue::Array(assessments.iter().map(assessment_json).collect()),
                        )
                    })
                    .collect(),
            ),
        );
    }
    if let Some(reason) = trace.action_reason() {
        out.insert("actionReason".to_owned(), json!(reason));
    }
    JsonValue::Object(out)
}

fn trace_json(
    guardrail: Option<&bt::GuardrailTraceAssessment>,
    router: Option<&bt::PromptRouterTrace>,
) -> JsonValue {
    let mut out = JsonObject::new();
    insert_opt(&mut out, "guardrail", guardrail.map(guardrail_trace_json));
    insert_opt(
        &mut out,
        "promptRouter",
        router.map(|router| strip_nulls(json!({ "invokedModelId": router.invoked_model_id() }))),
    );
    JsonValue::Object(out)
}

fn citation_location_json(location: &bt::CitationLocation) -> JsonValue {
    let triple = |index_key: &str, index: Option<i32>, start: Option<i32>, end: Option<i32>| {
        strip_nulls(json!({ index_key: index, "start": start, "end": end }))
    };
    match location {
        bt::CitationLocation::DocumentChar(loc) => {
            json!({ "documentChar": triple("documentIndex", loc.document_index(), loc.start(), loc.end()) })
        }
        bt::CitationLocation::DocumentPage(loc) => {
            json!({ "documentPage": triple("documentIndex", loc.document_index(), loc.start(), loc.end()) })
        }
        bt::CitationLocation::DocumentChunk(loc) => {
            json!({ "documentChunk": triple("documentIndex", loc.document_index(), loc.start(), loc.end()) })
        }
        bt::CitationLocation::SearchResultLocation(loc) => json!({
            "searchResultLocation": triple("searchResultIndex", loc.search_result_index(), loc.start(), loc.end())
        }),
        bt::CitationLocation::Web(loc) => {
            json!({ "web": strip_nulls(json!({ "url": loc.url(), "domain": loc.domain() })) })
        }
        _ => json!({ "unknown": {} }),
    }
}

fn citation_json(
    title: Option<&str>,
    source: Option<&str>,
    source_content: &[JsonValue],
    location: Option<&bt::CitationLocation>,
) -> JsonValue {
    strip_nulls(json!({
        "title": title,
        "source": source,
        "sourceContent": source_content,
        "location": location.map(citation_location_json),
    }))
}

fn stream_event_json(event: bt::ConverseStreamOutput) -> JsonValue {
    match event {
        bt::ConverseStreamOutput::MessageStart(start) => {
            json!({ "messageStart": { "role": start.role().as_str() } })
        }
        bt::ConverseStreamOutput::ContentBlockStart(start) => {
            let start_json = match start.start() {
                Some(bt::ContentBlockStart::ToolUse(tool_use)) => json!({
                    "toolUse": { "toolUseId": tool_use.tool_use_id(), "name": tool_use.name() }
                }),
                Some(bt::ContentBlockStart::Image(_)) => json!({ "image": {} }),
                Some(bt::ContentBlockStart::ToolResult(_)) => json!({ "toolResult": {} }),
                _ => json!({}),
            };
            json!({ "contentBlockStart": { "start": start_json, "contentBlockIndex": start.content_block_index() } })
        }
        bt::ConverseStreamOutput::ContentBlockDelta(delta) => {
            let delta_json = match delta.delta() {
                Some(bt::ContentBlockDelta::Text(text)) => json!({ "text": text }),
                Some(bt::ContentBlockDelta::ToolUse(tool_use)) => {
                    json!({ "toolUse": { "input": tool_use.input() } })
                }
                Some(bt::ContentBlockDelta::ReasoningContent(reasoning)) => {
                    json!({ "reasoningContent": reasoning_delta_json(reasoning) })
                }
                Some(bt::ContentBlockDelta::Citation(citation)) => json!({
                    "citation": citation_json(
                        citation.title(),
                        citation.source(),
                        &citation
                            .source_content()
                            .iter()
                            .map(|content| json!({ "text": content.text() }))
                            .collect::<Vec<_>>(),
                        citation.location(),
                    )
                }),
                Some(bt::ContentBlockDelta::Image(_)) => json!({ "image": {} }),
                Some(bt::ContentBlockDelta::ToolResult(_)) => json!({ "toolResult": [] }),
                _ => json!({ "unknown": {} }),
            };
            json!({ "contentBlockDelta": { "delta": delta_json, "contentBlockIndex": delta.content_block_index() } })
        }
        bt::ConverseStreamOutput::ContentBlockStop(stop) => {
            json!({ "contentBlockStop": { "contentBlockIndex": stop.content_block_index() } })
        }
        bt::ConverseStreamOutput::MessageStop(stop) => {
            let mut out = JsonObject::new();
            out.insert("stopReason".to_owned(), json!(stop.stop_reason().as_str()));
            insert_opt(
                &mut out,
                "additionalModelResponseFields",
                stop.additional_model_response_fields().map(document_to_json),
            );
            json!({ "messageStop": out })
        }
        bt::ConverseStreamOutput::Metadata(metadata) => {
            let mut out = JsonObject::new();
            insert_opt(&mut out, "usage", metadata.usage().map(usage_json));
            insert_opt(
                &mut out,
                "metrics",
                metadata
                    .metrics()
                    .map(|metrics| json!({ "latencyMs": metrics.latency_ms() })),
            );
            insert_opt(
                &mut out,
                "trace",
                metadata
                    .trace()
                    .map(|trace| trace_json(trace.guardrail(), trace.prompt_router())),
            );
            json!({ "metadata": out })
        }
        _ => json!({ "unknown": {} }),
    }
}

fn reasoning_delta_json(reasoning: &bt::ReasoningContentBlockDelta) -> JsonValue {
    match reasoning {
        bt::ReasoningContentBlockDelta::Text(text) => json!({ "text": text }),
        bt::ReasoningContentBlockDelta::Signature(signature) => json!({ "signature": signature }),
        bt::ReasoningContentBlockDelta::RedactedContent(bytes) => {
            json!({ "redactedContent": encode_base64(bytes.as_ref()) })
        }
        _ => json!({}),
    }
}

fn output_block_json(block: &bt::ContentBlock) -> JsonValue {
    match block {
        bt::ContentBlock::Text(text) => json!({ "text": text }),
        bt::ContentBlock::ToolUse(tool_use) => json!({
            "toolUse": { "toolUseId": tool_use.tool_use_id(), "name": tool_use.name(), "input": document_to_json(tool_use.input()) }
        }),
        bt::ContentBlock::ReasoningContent(bt::ReasoningContentBlock::ReasoningText(text)) => json!({
            "reasoningContent": { "reasoningText": strip_nulls(json!({ "text": text.text(), "signature": text.signature() })) }
        }),
        bt::ContentBlock::ReasoningContent(bt::ReasoningContentBlock::RedactedContent(bytes)) => {
            json!({ "reasoningContent": { "redactedContent": encode_base64(bytes.as_ref()) } })
        }
        bt::ContentBlock::CitationsContent(citations) => json!({
            "citationsContent": {
                "citations": citations.citations().iter().map(|citation| citation_json(
                    citation.title(),
                    citation.source(),
                    &citation.source_content().iter().map(|content| match content {
                        bt::CitationSourceContent::Text(text) => json!({ "text": text }),
                        _ => json!({}),
                    }).collect::<Vec<_>>(),
                    citation.location(),
                )).collect::<Vec<_>>(),
                "content": citations.content().iter().map(|content| match content {
                    bt::CitationGeneratedContent::Text(text) => json!({ "text": text }),
                    _ => json!({}),
                }).collect::<Vec<_>>(),
            }
        }),
        bt::ContentBlock::Image(_) => json!({ "image": {} }),
        bt::ContentBlock::Document(_) => json!({ "document": {} }),
        bt::ContentBlock::Video(_) => json!({ "video": {} }),
        bt::ContentBlock::Audio(_) => json!({ "audio": {} }),
        bt::ContentBlock::ToolResult(_) => json!({ "toolResult": {} }),
        bt::ContentBlock::GuardContent(_) => json!({ "guardContent": {} }),
        bt::ContentBlock::CachePoint(_) => json!({ "cachePoint": {} }),
        bt::ContentBlock::SearchResult(_) => json!({ "searchResult": {} }),
        _ => json!({ "unknown": {} }),
    }
}

fn converse_output_json(output: &aws_sdk_bedrockruntime::operation::converse::ConverseOutput) -> JsonObject {
    let mut out = JsonObject::new();
    if let Some(bt::ConverseOutput::Message(message)) = output.output() {
        out.insert(
            "output".to_owned(),
            json!({ "message": {
                "role": message.role().as_str(),
                "content": message.content().iter().map(output_block_json).collect::<Vec<_>>(),
            } }),
        );
    }
    out.insert("stopReason".to_owned(), json!(output.stop_reason().as_str()));
    insert_opt(&mut out, "usage", output.usage().map(usage_json));
    insert_opt(
        &mut out,
        "metrics",
        output
            .metrics()
            .map(|metrics| json!({ "latencyMs": metrics.latency_ms() })),
    );
    insert_opt(
        &mut out,
        "additionalModelResponseFields",
        output.additional_model_response_fields().map(document_to_json),
    );
    insert_opt(
        &mut out,
        "trace",
        output
            .trace()
            .map(|trace| trace_json(trace.guardrail(), trace.prompt_router())),
    );
    out
}

/// The [`Transport`] over an `aws-sdk-bedrockruntime` client.
#[derive(Debug)]
pub(crate) struct SdkTransport {
    client: Client,
    documents: Mutex<DocumentCache>,
}

impl SdkTransport {
    pub(crate) fn new(client: Client) -> Self {
        Self {
            client,
            documents: Mutex::new(DocumentCache::default()),
        }
    }

    fn convert(&self, request: &JsonObject) -> WireResult<SdkRequest> {
        let mut documents = self.documents.lock().unwrap_or_else(PoisonError::into_inner);
        let converted = Converter {
            documents: &mut documents,
        }
        .request(request);
        documents.finish();
        converted
    }
}

#[async_trait]
impl Transport for SdkTransport {
    async fn send(&self, operation: Operation, request: JsonObject) -> WireResult<Response> {
        let mut converted = self.convert(&request)?;
        let guardrail = converted.guardrail.take();
        match operation {
            Operation::ConverseStream => {
                let output = apply_request!(self.client.converse_stream(), converted)
                    .set_guardrail_config(stream_guardrail(guardrail.as_ref()))
                    .send()
                    .await
                    .map_err(classify)?;
                let events: EventStream = Box::pin(futures::stream::unfold(
                    Some(output.stream),
                    |receiver| async move {
                        let mut receiver = receiver?;
                        match receiver.recv().await {
                            Ok(Some(event)) => Some((Ok(stream_event_json(event)), Some(receiver))),
                            Ok(None) => None,
                            Err(error) => Some((Err(classify(error)), None)),
                        }
                    },
                ));
                Ok(Response::Stream(events))
            }
            Operation::Converse => {
                let output = apply_request!(self.client.converse(), converted)
                    .set_guardrail_config(converse_guardrail(guardrail.as_ref()))
                    .send()
                    .await
                    .map_err(classify)?;
                Ok(Response::Converse(converse_output_json(&output)))
            }
        }
    }

    async fn count_tokens(&self, model_id: &str, input: JsonObject) -> WireResult<Option<u64>> {
        let converted = self.convert(&input)?;
        let request = bt::ConverseTokensRequest::builder()
            .set_messages(converted.messages)
            .set_system(converted.system)
            .set_tool_config(converted.tool_config)
            .build();
        let output = self
            .client
            .count_tokens()
            .model_id(model_id)
            .input(bt::CountTokensInput::Converse(request))
            .send()
            .await
            .map_err(classify)?;
        Ok(u64::try_from(output.input_tokens()).ok())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

    use aws_sdk_bedrockruntime::config::http::HttpResponse;
    use aws_sdk_bedrockruntime::error::ConnectorError;
    use aws_sdk_bedrockruntime::operation::converse::ConverseOutput as SdkConverseOutput;
    use aws_sdk_bedrockruntime::operation::converse_stream::ConverseStreamError;
    use aws_smithy_types::body::SdkBody;
    use aws_smithy_types::event_stream::{Message as EventMessage, RawMessage};

    use super::*;
    use crate::models::bedrock::parse::has_blocked_guardrail_policy;

    fn response() -> HttpResponse {
        HttpResponse::new(400.try_into().unwrap(), SdkBody::empty())
    }

    fn convert(request: &JsonValue) -> WireResult<SdkRequest> {
        let mut documents = DocumentCache::default();
        Converter {
            documents: &mut documents,
        }
        .request(request.as_object().unwrap())
    }

    #[test]
    fn classifies_a_throttled_send_through_the_typed_variant() {
        let error = SdkError::service_error(
            ConverseStreamError::ThrottlingException(
                bt::error::ThrottlingException::builder()
                    .message("Too many tokens")
                    .build(),
            ),
            response(),
        );
        let classified = classify(error);
        assert_eq!(classified.kind, BedrockErrorKind::Throttled);
        assert_eq!(classified.name, "ThrottlingException");
        assert_eq!(classified.message, "Too many tokens");
        assert!(matches!(classified.into_error(), crate::Error::ModelThrottled(_)));
    }

    #[test]
    fn classifies_a_validation_exception_as_a_service_error_with_its_code() {
        let error = SdkError::service_error(
            ConverseStreamError::ValidationException(
                bt::error::ValidationException::builder()
                    .message("Input is too long for requested model")
                    .build(),
            ),
            response(),
        );
        let classified = classify(error);
        assert_eq!(classified.kind, BedrockErrorKind::Service);
        assert_eq!(classified.name, "ValidationException");
        assert_eq!(classified.message, "Input is too long for requested model");
    }

    #[test]
    fn classifies_transport_failures_as_network_errors() {
        let timeout: SdkError<ConverseStreamError, HttpResponse> = SdkError::timeout_error("timed out");
        assert_eq!(classify(timeout).kind, BedrockErrorKind::Network);
        let dispatch: SdkError<ConverseStreamError, HttpResponse> =
            SdkError::dispatch_failure(ConnectorError::io("connection refused".into()));
        let classified = classify(dispatch);
        assert_eq!(classified.kind, BedrockErrorKind::Network);
        assert!(matches!(classified.into_error(), crate::Error::ModelNetwork(_)));
    }

    #[test]
    fn classifies_stream_errors_from_recv_through_the_typed_variants() {
        let raw = || RawMessage::Decoded(EventMessage::new(&b""[..]));
        let throttled = SdkError::service_error(
            bt::error::ConverseStreamOutputError::ThrottlingException(
                bt::error::ThrottlingException::builder()
                    .message("Rate exceeded")
                    .build(),
            ),
            raw(),
        );
        let classified = classify(throttled);
        assert_eq!(classified.kind, BedrockErrorKind::Throttled);
        let stream_error = SdkError::service_error(
            bt::error::ConverseStreamOutputError::ModelStreamErrorException(
                bt::error::ModelStreamErrorException::builder()
                    .message("stream broke")
                    .build(),
            ),
            raw(),
        );
        let classified = classify(stream_error);
        assert_eq!(
            (
                classified.kind,
                classified.name.as_str(),
                classified.message.as_str()
            ),
            (
                BedrockErrorKind::Service,
                "ModelStreamErrorException",
                "stream broke"
            )
        );
    }

    #[test]
    fn converts_every_request_block_to_sdk_types() {
        let request = json!({
            "modelId": "m",
            "messages": [
                { "role": "user", "content": [
                    { "text": "hi" },
                    { "image": { "format": "png", "source": { "bytes": "AQID" } } },
                    { "document": { "name": "d", "format": "md", "source": { "bytes": "AQ==" }, "citations": { "enabled": true }, "context": "c" } },
                    { "video": { "format": "three_gp", "source": { "s3Location": { "uri": "s3://b/v", "bucketOwner": "1" } } } },
                    { "audio": { "format": "mp3", "source": { "bytes": "AQ==" } } },
                    { "guardContent": { "text": { "text": "g", "qualifiers": ["query"] } } },
                    { "guardContent": { "image": { "format": "png", "source": { "bytes": "AQ==" } } } },
                    { "toolResult": { "toolUseId": "t", "content": [{ "text": "x" }, { "json": { "a": 1 } }], "status": "error" } },
                    { "cachePoint": { "type": "default", "ttl": "1h" } },
                ] },
                { "role": "assistant", "content": [
                    { "toolUse": { "toolUseId": "t", "name": "n", "input": { "b": [1, 2] } } },
                    { "reasoningContent": { "reasoningText": { "text": "r", "signature": "s" } } },
                    { "reasoningContent": { "redactedContent": "AQ==" } },
                    { "citationsContent": { "citations": [{ "location": { "web": { "url": "u" } }, "sourceContent": [{ "text": "s" }], "title": "t" }], "content": [{ "text": "c" }] } },
                ] },
            ],
            "system": [{ "text": "sys" }, { "cachePoint": { "type": "default" } }],
            "toolConfig": {
                "tools": [{ "toolSpec": { "name": "n", "description": "d", "inputSchema": { "json": { "type": "object" } }, "strict": true } }, { "cachePoint": { "type": "default" } }],
                "toolChoice": { "tool": { "name": "n" } },
            },
            "inferenceConfig": { "maxTokens": 10, "temperature": 0.5, "topP": 0.9, "stopSequences": ["x"] },
            "additionalModelRequestFields": { "thinking": { "type": "adaptive" } },
            "additionalModelResponseFieldPaths": ["/a"],
            "guardrailConfig": { "guardrailIdentifier": "g", "guardrailVersion": "1", "trace": "enabled", "streamProcessingMode": "sync" },
            "outputConfig": { "textFormat": { "type": "json_schema", "structure": { "jsonSchema": { "schema": "{}", "name": "s" } } } },
            "requestMetadata": { "k": "v" },
            "performanceConfig": { "latency": "optimized" },
            "serviceTier": { "type": "priority" },
        });
        let converted = convert(&request).unwrap();
        let messages = converted.messages.unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content().len(), 9);
        assert_eq!(messages[1].content().len(), 4);
        assert!(
            matches!(&messages[0].content()[1], bt::ContentBlock::Image(image) if image.format() == &bt::ImageFormat::Png)
        );
        assert!(
            matches!(&messages[1].content()[0], bt::ContentBlock::ToolUse(tool) if document_to_json(tool.input()) == json!({ "b": [1, 2] }))
        );
        assert_eq!(converted.system.unwrap().len(), 2);
        let tool_config = converted.tool_config.unwrap();
        assert_eq!(tool_config.tools().len(), 2);
        assert!(matches!(tool_config.tool_choice(), Some(bt::ToolChoice::Tool(tool)) if tool.name() == "n"));
        let inference = converted.inference_config.unwrap();
        assert_eq!((inference.max_tokens(), inference.top_p()), (Some(10), Some(0.9)));
        assert!(stream_guardrail(converted.guardrail.as_ref()).is_some());
        assert!(converted.output_config.is_some());
        assert_eq!(converted.request_metadata.unwrap()["k"], "v");
        assert!(converted.performance_config.is_some() && converted.service_tier.is_some());
    }

    #[test]
    fn rejects_an_additional_args_key_that_is_not_a_request_field() {
        let error = convert(&json!({ "modelId": "m", "messages": [], "MyExtraArg": "x" }))
            .err()
            .unwrap();
        assert_eq!(
            error.message,
            "key=<MyExtraArg> | additionalArgs key is not a Converse request field"
        );
    }

    #[test]
    fn reuses_the_document_built_for_an_identical_value_in_the_next_request() {
        let schema = json!({ "type": "object", "properties": { "z": {}, "a": {}, "m": {}, "b": {}, "y": {}, "c": {} } });
        let mut cache = DocumentCache::default();
        let first = cache.get(&schema);
        cache.finish();
        let second = cache.get(&schema);
        let (Document::Object(first), Document::Object(second)) = (first, second) else {
            panic!("objects");
        };
        let keys = |doc: &HashMap<String, Document>| match doc.get("properties") {
            Some(Document::Object(properties)) => properties.keys().cloned().collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        assert_eq!(keys(&first), keys(&second));
    }

    #[test]
    fn maps_sdk_stream_events_to_their_json_shapes() {
        let start = bt::ConverseStreamOutput::ContentBlockStart(
            bt::ContentBlockStartEvent::builder()
                .content_block_index(0)
                .start(bt::ContentBlockStart::ToolUse(
                    bt::ToolUseBlockStart::builder()
                        .tool_use_id("t")
                        .name("n")
                        .build()
                        .unwrap(),
                ))
                .build()
                .unwrap(),
        );
        assert_eq!(
            stream_event_json(start),
            json!({ "contentBlockStart": { "start": { "toolUse": { "toolUseId": "t", "name": "n" } }, "contentBlockIndex": 0 } })
        );
        let delta = bt::ConverseStreamOutput::ContentBlockDelta(
            bt::ContentBlockDeltaEvent::builder()
                .content_block_index(1)
                .delta(bt::ContentBlockDelta::ReasoningContent(
                    bt::ReasoningContentBlockDelta::RedactedContent(Blob::new(vec![1, 2, 3])),
                ))
                .build()
                .unwrap(),
        );
        assert_eq!(
            stream_event_json(delta),
            json!({ "contentBlockDelta": { "delta": { "reasoningContent": { "redactedContent": "AQID" } }, "contentBlockIndex": 1 } })
        );
        let stop = bt::ConverseStreamOutput::MessageStop(
            bt::MessageStopEvent::builder()
                .stop_reason(bt::StopReason::ToolUse)
                .additional_model_response_fields(json_to_document(&json!({ "x": 1 })))
                .build()
                .unwrap(),
        );
        assert_eq!(
            stream_event_json(stop),
            json!({ "messageStop": { "stopReason": "tool_use", "additionalModelResponseFields": { "x": 1 } } })
        );
        let metadata = bt::ConverseStreamOutput::Metadata(
            bt::ConverseStreamMetadataEvent::builder()
                .usage(
                    bt::TokenUsage::builder()
                        .input_tokens(10)
                        .output_tokens(5)
                        .total_tokens(15)
                        .cache_read_input_tokens(8)
                        .build()
                        .unwrap(),
                )
                .metrics(
                    bt::ConverseStreamMetrics::builder()
                        .latency_ms(100)
                        .build()
                        .unwrap(),
                )
                .build(),
        );
        assert_eq!(
            stream_event_json(metadata),
            json!({ "metadata": {
                "usage": { "inputTokens": 10, "outputTokens": 5, "totalTokens": 15, "cacheReadInputTokens": 8 },
                "metrics": { "latencyMs": 100 },
            } })
        );
    }

    #[test]
    fn maps_a_guardrail_trace_so_blocked_policies_are_detected() {
        let topic = bt::GuardrailTopic::builder()
            .name("Harmful")
            .r#type(bt::GuardrailTopicType::Deny)
            .action(bt::GuardrailTopicPolicyAction::Blocked)
            .detected(true)
            .build()
            .unwrap();
        let assessment = bt::GuardrailAssessment::builder()
            .topic_policy(
                bt::GuardrailTopicPolicyAssessment::builder()
                    .topics(topic)
                    .build()
                    .unwrap(),
            )
            .build();
        let trace = bt::GuardrailTraceAssessment::builder()
            .model_output("blocked text")
            .input_assessment("g1", assessment)
            .build();
        let json = guardrail_trace_json(&trace);
        assert_eq!(
            json,
            json!({
                "modelOutput": ["blocked text"],
                "inputAssessment": { "g1": { "topicPolicy": { "topics": [
                    { "name": "Harmful", "type": "DENY", "action": "BLOCKED", "detected": true }
                ] } } },
            })
        );
        assert!(has_blocked_guardrail_policy(&json));
    }

    #[test]
    fn maps_a_converse_response_to_its_json_shape() {
        let message = bt::Message::builder()
            .role(bt::ConversationRole::Assistant)
            .content(bt::ContentBlock::Text("Hello".into()))
            .content(bt::ContentBlock::ToolUse(
                bt::ToolUseBlock::builder()
                    .tool_use_id("t")
                    .name("n")
                    .input(json_to_document(&json!({ "a": 1 })))
                    .build()
                    .unwrap(),
            ))
            .build()
            .unwrap();
        let output = SdkConverseOutput::builder()
            .output(bt::ConverseOutput::Message(message))
            .stop_reason(bt::StopReason::EndTurn)
            .usage(
                bt::TokenUsage::builder()
                    .input_tokens(1)
                    .output_tokens(2)
                    .total_tokens(3)
                    .build()
                    .unwrap(),
            )
            .metrics(bt::ConverseMetrics::builder().latency_ms(7).build().unwrap())
            .build()
            .unwrap();
        assert_eq!(
            JsonValue::Object(converse_output_json(&output)),
            json!({
                "output": { "message": { "role": "assistant", "content": [
                    { "text": "Hello" },
                    { "toolUse": { "toolUseId": "t", "name": "n", "input": { "a": 1 } } },
                ] } },
                "stopReason": "end_turn",
                "usage": { "inputTokens": 1, "outputTokens": 2, "totalTokens": 3 },
                "metrics": { "latencyMs": 7 },
            })
        );
    }
}
