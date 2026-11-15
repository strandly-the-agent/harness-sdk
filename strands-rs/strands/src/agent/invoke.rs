//! What an invocation takes: input and options (TS `InvokeArgs` and `InvokeOptions` in
//! `types/agent.ts`).

use schemars::JsonSchema;
use serde::de::DeserializeOwned;

use crate::agent::invocation_context::{InvocationContext, InvokeLimits};
use crate::agent::invocation_state::InvocationState;
use crate::cancel::CancelSignal;
use crate::interrupt::InterruptResponse;
use crate::tools::structured_output::StructuredOutputSchema;
use crate::types::messages::{ContentBlock, Message};

/// The input of an invocation (TS `InvokeArgs`).
///
/// Build it from a prompt (`&str` or `String`), content blocks, messages, or the answers to an
/// interrupted invocation. Empty content or no messages continue from the current history.
///
/// ```
/// use strands::{ContentBlock, InvokeArgs};
///
/// let prompt = InvokeArgs::from("Hello");
/// let blocks = InvokeArgs::from(vec![ContentBlock::from("Hello")]);
/// assert_ne!(prompt, InvokeArgs::from(Vec::<ContentBlock>::new()));
/// # let _ = blocks;
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum InvokeArgs {
    /// A user prompt.
    Text(String),
    /// The content of one user message.
    ContentBlocks(Vec<ContentBlock>),
    /// Messages to append as they are.
    Messages(Vec<Message>),
    /// Answers to the interrupts of an interrupted invocation.
    InterruptResponses(Vec<InterruptResponse>),
}

impl Default for InvokeArgs {
    fn default() -> Self {
        Self::ContentBlocks(Vec::new())
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
        Self::ContentBlocks(content)
    }
}

impl From<ContentBlock> for InvokeArgs {
    fn from(content: ContentBlock) -> Self {
        Self::ContentBlocks(vec![content])
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

/// Options of one invocation (TS `InvokeOptions`).
///
/// ```
/// use strands::InvokeOptions;
/// use strands::agent::InvokeLimits;
///
/// /// The weather in a city.
/// #[derive(serde::Deserialize, schemars::JsonSchema)]
/// struct Weather {
///     city: String,
///     temperature: f64,
/// }
///
/// let options = InvokeOptions {
///     limits: Some(InvokeLimits::default()),
///     ..InvokeOptions::default()
/// }
/// .structured_output_schema::<Weather>();
/// assert!(options.structured_output_schema.is_some());
/// ```
#[derive(Debug, Clone, Default, bon::Builder)]
pub struct InvokeOptions {
    /// The type the result's structured output must have, for this call only; overrides
    /// [`AgentConfig::structured_output_schema`](crate::AgentConfig::structured_output_schema).
    pub structured_output_schema: Option<StructuredOutputSchema>,
    /// Values hooks and tools read and write during this call, returned on
    /// [`AgentResult::invocation_state`](crate::AgentResult::invocation_state). A new empty state
    /// when unset.
    pub invocation_state: Option<InvocationState>,
    /// A signal that cancels this call, alongside [`Agent::cancel_handle`](crate::Agent::cancel_handle).
    pub cancel_signal: Option<CancelSignal>,
    /// Runs this call as a nested call in that request, sharing its limits and usage; `limits`
    /// set alongside it only add stricter limits.
    pub invocation_context: Option<InvocationContext>,
    /// Limits for the request this call starts.
    pub limits: Option<InvokeLimits>,
}

impl InvokeOptions {
    /// Asks for structured output of type `T` (TS `structuredOutputSchema`).
    #[must_use]
    pub fn structured_output_schema<T: JsonSchema + DeserializeOwned + 'static>(mut self) -> Self {
        self.structured_output_schema = Some(StructuredOutputSchema::of::<T>());
        self
    }
}
