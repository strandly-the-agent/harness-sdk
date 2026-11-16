//! The model-backed extractor (TS `memory/extraction/model-extractor.ts`).

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;

use super::types::{ExtractionResult, Extractor, ExtractorContext};
use crate::JsonValue;
use crate::error::{Error, Result};
use crate::models::model::{AggregatedEvent, Model, ModelRequest};
use crate::models::model_proxy::ModelProxy;
use crate::telemetry::tracer::ModelSpanEnd;
use crate::telemetry::tracer::memory::MemoryTracer;
use crate::types::messages::{ContentBlock, Message, Role, SystemPrompt, TextBlock};

const DEFAULT_SYSTEM_PROMPT: &str = "You extract durable facts worth remembering across future conversations from a transcript.\n\n\
Each entry is one discrete, self-contained fact (a preference, decision, or stable detail about the user or task). Do not include transient chit-chat, questions, or anything already obvious.";

const OUTPUT_FORMAT: &str = "Return ONLY a JSON array of objects, each: {\"content\": string}. If there is nothing worth remembering, return [].";

/// Options for [`ModelExtractor`] (TS `ModelExtractorOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Clone, Default, bon::Builder)]
#[builder(state_mod(vis = "pub"))]
pub struct ModelExtractorOptions {
    /// The extracting model; order: this model > `agent.aux_model` > `agent.model`.
    pub model: Option<Arc<dyn Model>>,
    /// Replaces the default framing; the output-format contract is always appended.
    #[builder(into)]
    pub system_prompt: Option<String>,
}

opaque_builder_debug!(ModelExtractorOptionsBuilder, model_extractor_options_builder);

impl fmt::Debug for ModelExtractorOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelExtractorOptions")
            .field("model", &self.model.as_ref().map(|_| ".."))
            .field("system_prompt", &self.system_prompt)
            .finish()
    }
}

/// Extracts facts from a transcript with one model call (TS `ModelExtractor`).
#[derive(Clone)]
pub struct ModelExtractor {
    model: Option<Arc<dyn Model>>,
    system_prompt: String,
}

impl Default for ModelExtractor {
    fn default() -> Self {
        Self::new(ModelExtractorOptions::default())
    }
}

impl fmt::Debug for ModelExtractor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelExtractor")
            .field("model", &self.model.as_ref().map(|_| ".."))
            .field("system_prompt", &self.system_prompt)
            .finish()
    }
}

impl ModelExtractor {
    /// An extractor with `options`.
    pub fn new(options: ModelExtractorOptions) -> Self {
        let framing = options.system_prompt.as_deref().unwrap_or(DEFAULT_SYSTEM_PROMPT);
        Self {
            model: options.model,
            system_prompt: format!("{framing}\n\n{OUTPUT_FORMAT}"),
        }
    }
}

#[async_trait]
impl Extractor for ModelExtractor {
    async fn extract(
        &self,
        messages: &[Message],
        context: &ExtractorContext,
    ) -> Result<Vec<ExtractionResult>> {
        let Some(model) = self.model.as_ref().or(context.default_model.as_ref()) else {
            return Err(Error::other(
                "ModelExtractor: no model configured and no default model available",
            ));
        };
        if messages.is_empty() {
            return Ok(Vec::new());
        }
        // One user turn, so the system prompt governs extraction instead of the model continuing
        // the raw roles.
        let transcript = messages.iter().map(render_message).collect::<Vec<_>>().join("\n");
        let prompt_messages = vec![Message::new(
            Role::User,
            vec![
                TextBlock::new(format!(
                    "Extract facts from the following transcript:\n\n{transcript}"
                ))
                .into(),
            ],
        )];
        let system_prompt = SystemPrompt::from(self.system_prompt.as_str());
        let mut tracer = context.tracer.as_ref().map(MemoryTracer::model_tracer);
        if let Some(tracer) = tracer.as_mut() {
            tracer.start_model(
                &prompt_messages,
                model.model_id().as_deref(),
                Some(&system_prompt),
            );
        }
        let request = ModelRequest::builder()
            .messages(prompt_messages)
            .system_prompt(system_prompt)
            .build();
        let mut stream =
            ModelProxy::new(&**model).stream_aggregated(request, context.invocation_context.clone());
        let mut result = None;
        while let Some(item) = stream.next().await {
            match item {
                Ok(AggregatedEvent::Done(done)) => {
                    result = Some(done);
                    break;
                }
                Ok(_) => {}
                Err(error) => {
                    if let Some(tracer) = tracer.as_mut() {
                        tracer.end_model(&ModelSpanEnd::failed(&error));
                    }
                    return Err(error);
                }
            }
        }
        let Some(result) = result else {
            let error = Error::other("ModelExtractor: model returned no response");
            if let Some(tracer) = tracer.as_mut() {
                tracer.end_model(&ModelSpanEnd::failed(&error));
            }
            return Err(error);
        };
        if let Some(tracer) = tracer.as_mut() {
            tracer.end_model(&ModelSpanEnd {
                output: Some(result.message()),
                stop_reason: Some(result.stop_reason().as_str()),
                usage: result.metadata().and_then(|metadata| metadata.usage()),
                metrics: result.metadata().and_then(|metadata| metadata.metrics()),
                error: None,
            });
        }
        let reply = result.into_message();
        let text: String = reply.content().iter().map(block_text).collect();
        Ok(parse_entries(
            text.trim(),
            model.model_id().as_deref().unwrap_or_default(),
        ))
    }
}

fn block_text(block: &ContentBlock) -> &str {
    match block {
        ContentBlock::Text(text) => text.text(),
        _ => "",
    }
}

fn render_message(message: &Message) -> String {
    let text = message
        .content()
        .iter()
        .map(block_text)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    format!("{}: {text}", message.role().as_str())
}

fn parse_entries(text: &str, model_name: &str) -> Vec<ExtractionResult> {
    let Some(json) = extract_json_array(text) else {
        tracing::warn!("model=<{model_name}> | ModelExtractor: no JSON array in model output, skipping");
        return Vec::new();
    };
    let parsed: JsonValue = match serde_json::from_str(json) {
        Ok(parsed) => parsed,
        Err(error) => {
            tracing::warn!("model=<{model_name}>, error=<{error}> | ModelExtractor: failed to parse output");
            return Vec::new();
        }
    };
    let JsonValue::Array(items) = parsed else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|item| {
            let content = item.get("content")?.as_str()?.trim().to_owned();
            if content.is_empty() {
                return None;
            }
            let metadata = item.get("metadata").and_then(JsonValue::as_object).cloned();
            Some(ExtractionResult { content, metadata })
        })
        .collect()
}

fn extract_json_array(text: &str) -> Option<&str> {
    let start = text.find('[')?;
    let end = text.rfind(']')?;
    text.get(start..=end)
}

#[cfg(test)]
mod tests;
