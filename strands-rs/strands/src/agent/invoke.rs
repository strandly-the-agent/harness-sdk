//! Invocation input and options (TS `InvokeArgs`, `InvokeOptions` in `types/agent.ts`).

use schemars::JsonSchema;

use crate::agent::{InvocationContext, InvocationState, InvokeLimits};
use crate::cancel::CancelSignal;
use crate::error::{Result, TypeError};
use crate::tools::schema::schema_for;
use crate::types::{ContentBlock, InterruptResponse, InterruptResponseContent, JsonValue, Message, Role};

/// What to invoke an agent with (TS `InvokeArgs`).
///
/// Built through `From`: a prompt (`&str`, `String`), content blocks (one user message),
/// messages, or interrupt responses (to resume after an interrupt).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum InvokeArgs {
    /// A text prompt.
    Text(String),
    /// Content blocks of one user message.
    Content(Vec<ContentBlock>),
    /// Messages to append as they are.
    Messages(Vec<Message>),
    /// Answers to the interrupts the last invocation stopped on.
    InterruptResponses(Vec<InterruptResponse>),
}

impl InvokeArgs {
    /// No input: the agent continues from its history.
    pub fn empty() -> Self {
        Self::Messages(Vec::new())
    }

    pub(crate) fn interrupt_responses(&self) -> Vec<InterruptResponseContent> {
        match self {
            Self::InterruptResponses(responses) => responses
                .iter()
                .cloned()
                .map(InterruptResponseContent::new)
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The messages this input appends (TS `_normalizeInput`).
    pub(crate) fn to_messages(&self) -> Result<Vec<Message>> {
        let messages = match self {
            Self::Text(text) => vec![Message::new(Role::User, vec![ContentBlock::from(text.as_str())])],
            Self::Content(content) if content.is_empty() => Vec::new(),
            Self::Content(content) => vec![Message::new(Role::User, content.clone())],
            Self::Messages(messages) => messages.clone(),
            Self::InterruptResponses(_) => Vec::new(),
        };
        let blank = |message: &Message| {
            message.role == Role::User
                && message
                    .content
                    .iter()
                    .all(|block| matches!(block, ContentBlock::Text(text) if text.text.trim().is_empty()))
        };
        if messages.iter().any(blank) {
            return Err(TypeError::new(
                "Input prompt is blank. Provide non-blank text or at least one non-text content block.",
            )
            .into());
        }
        Ok(messages)
    }
}

impl From<&str> for InvokeArgs {
    fn from(text: &str) -> Self {
        Self::Text(text.to_owned())
    }
}

impl From<String> for InvokeArgs {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<&String> for InvokeArgs {
    fn from(text: &String) -> Self {
        Self::Text(text.clone())
    }
}

impl From<Vec<ContentBlock>> for InvokeArgs {
    fn from(content: Vec<ContentBlock>) -> Self {
        Self::Content(content)
    }
}

impl From<ContentBlock> for InvokeArgs {
    fn from(block: ContentBlock) -> Self {
        Self::Content(vec![block])
    }
}

impl From<Vec<Message>> for InvokeArgs {
    fn from(messages: Vec<Message>) -> Self {
        Self::Messages(messages)
    }
}

impl From<Message> for InvokeArgs {
    fn from(message: Message) -> Self {
        Self::Messages(vec![message])
    }
}

impl From<Vec<InterruptResponse>> for InvokeArgs {
    fn from(responses: Vec<InterruptResponse>) -> Self {
        Self::InterruptResponses(responses)
    }
}

impl From<InterruptResponse> for InvokeArgs {
    fn from(response: InterruptResponse) -> Self {
        Self::InterruptResponses(vec![response])
    }
}

/// Options for one invocation (TS `InvokeOptions`).
///
/// ```
/// use strands::InvokeOptions;
///
/// #[derive(serde::Deserialize, schemars::JsonSchema)]
/// struct Summary { title: String }
///
/// let options = InvokeOptions::default().structured_output_schema::<Summary>();
/// assert!(options.structured_output_schema.is_some());
/// ```
#[derive(Debug, Clone, Default, bon::Builder)]
pub struct InvokeOptions {
    /// A JSON schema the final answer must match; overrides the agent's.
    pub structured_output_schema: Option<JsonValue>,
    /// Values for this invocation's hooks and tools. A fresh state when not set.
    pub invocation_state: Option<InvocationState>,
    /// A caller-owned cancel signal, combined with the agent's own.
    pub cancel_signal: Option<CancelSignal>,
    /// The outer invocation's context, so this invocation's usage and limits nest in it.
    pub invocation_context: Option<InvocationContext>,
    /// Limits for this invocation.
    pub limits: Option<InvokeLimits>,
}

impl InvokeOptions {
    /// Sets the structured output schema to the JSON schema of `T`.
    #[must_use]
    pub fn structured_output_schema<T: JsonSchema>(mut self) -> Self {
        self.structured_output_schema = Some(schema_for::<T>());
        self
    }
}
