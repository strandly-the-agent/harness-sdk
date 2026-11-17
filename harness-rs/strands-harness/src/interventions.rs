//! Resolves the harness's `interventions` option into SDK intervention handlers (TS
//! `interventions.ts`).
//!
//! The option accepts a preset name, a natural-language policy, a Cedar policy file, an SDK
//! handler, or a list of these. A handler passes through untouched, so anything the presets
//! don't cover stays reachable by constructing the SDK handler yourself.
//!
//! The string grammar is deterministic, with no content sniffing:
//!
//! - `off`, `ask` and `smart` are presets of [`HumanInTheLoop`],
//! - a path ending in `.cedar` loads a [`CedarAuthorization`] policy file,
//! - any other string is a natural-language risk policy: the LLM risk classifier's prompt.
//!
//! Inline Cedar policy text is not detected (it cannot be told from prose): pass a
//! [`CedarAuthorization`] handler for that.

use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use strands::interventions::InterventionHandler;
use strands::vended_interventions::cedar::{CedarAuthorization, CedarAuthorizationConfig};
use strands::vended_interventions::hitl::{
    Ask, Classifier, HumanInTheLoop, HumanInTheLoopConfig, LlmClassifierConfig,
};

use crate::error::{HarnessError, Result};
use crate::text::js_trim;

/// One entry of the `interventions` option (TS `InterventionValue`).
#[derive(Clone)]
#[non_exhaustive]
pub enum InterventionValue {
    /// A preset name (`off`, `ask`, `smart`), the path of a `.cedar` policy file, or a
    /// natural-language risk policy.
    Text(String),
    /// An SDK handler, used as it is.
    Handler(Arc<dyn InterventionHandler>),
}

impl InterventionValue {
    /// The entry for an SDK handler.
    pub fn handler(handler: impl InterventionHandler) -> Self {
        Self::Handler(Arc::new(handler))
    }
}

impl fmt::Debug for InterventionValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(text) => f.debug_tuple("Text").field(text).finish(),
            Self::Handler(handler) => f.debug_tuple("Handler").field(&handler.name()).finish(),
        }
    }
}

impl From<&str> for InterventionValue {
    fn from(text: &str) -> Self {
        Self::Text(text.to_owned())
    }
}

impl From<String> for InterventionValue {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<Arc<dyn InterventionHandler>> for InterventionValue {
    fn from(handler: Arc<dyn InterventionHandler>) -> Self {
        Self::Handler(handler)
    }
}

/// The harness's `interventions` option (TS `InterventionsOption`): one value or a list.
///
/// ```
/// use strands_harness::InterventionsOption;
///
/// let ask: InterventionsOption = "ask".into();
/// let layered: InterventionsOption = ["policies/agent.cedar", "smart"].into();
/// assert_eq!(ask.values().len(), 1);
/// assert_eq!(layered.values().len(), 2);
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum InterventionsOption {
    /// A single value.
    One(InterventionValue),
    /// A list of values, resolved in order.
    Many(Vec<InterventionValue>),
}

impl InterventionsOption {
    /// The values, in order.
    pub fn values(&self) -> &[InterventionValue] {
        match self {
            Self::One(value) => std::slice::from_ref(value),
            Self::Many(values) => values,
        }
    }
}

impl From<InterventionValue> for InterventionsOption {
    fn from(value: InterventionValue) -> Self {
        Self::One(value)
    }
}

impl From<&str> for InterventionsOption {
    fn from(text: &str) -> Self {
        Self::One(text.into())
    }
}

impl From<String> for InterventionsOption {
    fn from(text: String) -> Self {
        Self::One(text.into())
    }
}

impl From<Arc<dyn InterventionHandler>> for InterventionsOption {
    fn from(handler: Arc<dyn InterventionHandler>) -> Self {
        Self::One(handler.into())
    }
}

impl<V: Into<InterventionValue>> From<Vec<V>> for InterventionsOption {
    fn from(values: Vec<V>) -> Self {
        Self::Many(values.into_iter().map(Into::into).collect())
    }
}

impl<V: Into<InterventionValue>, const N: usize> From<[V; N]> for InterventionsOption {
    fn from(values: [V; N]) -> Self {
        Self::Many(values.into_iter().map(Into::into).collect())
    }
}

/// What a string value of the option means.
#[derive(Debug)]
enum Parsed {
    Off,
    HumanInTheLoop(HumanInTheLoopConfig),
    CedarFile(String),
}

fn parse(text: &str, ask: Option<&Ask>) -> Parsed {
    let ask = ask.cloned();
    let trimmed = js_trim(text);
    match trimmed {
        "off" => Parsed::Off,
        "ask" => Parsed::HumanInTheLoop(HumanInTheLoopConfig {
            ask,
            ..HumanInTheLoopConfig::default()
        }),
        "smart" => Parsed::HumanInTheLoop(HumanInTheLoopConfig {
            classifier: Some(Classifier::Enabled(true)),
            ask,
            ..HumanInTheLoopConfig::default()
        }),
        // The `.cedar` suffix only: the SDK loader treats any other string as inline policy, and
        // checking whether a file exists would misroute prose that matched a file name.
        _ if trimmed.ends_with(".cedar") => Parsed::CedarFile(trimmed.to_owned()),
        // A natural-language policy: the LLM risk classifier judges each call against this
        // prompt and escalates a flagged one for approval.
        _ => Parsed::HumanInTheLoop(HumanInTheLoopConfig {
            classifier: Some(Classifier::Llm(LlmClassifierConfig {
                system_prompt: Some(text.to_owned()),
                ..LlmClassifierConfig::default()
            })),
            ask,
            ..HumanInTheLoopConfig::default()
        }),
    }
}

fn resolve_one(value: &InterventionValue, ask: Option<&Ask>) -> Result<Option<Arc<dyn InterventionHandler>>> {
    let text = match value {
        InterventionValue::Handler(handler) => return Ok(Some(Arc::clone(handler))),
        InterventionValue::Text(text) => text,
    };
    Ok(match parse(text, ask) {
        Parsed::Off => None,
        Parsed::HumanInTheLoop(config) => Some(Arc::new(HumanInTheLoop::new(config))),
        Parsed::CedarFile(policies) => Some(Arc::new(CedarAuthorization::new(CedarAuthorizationConfig {
            policies,
            ..CedarAuthorizationConfig::default()
        })?)),
    })
}

/// The SDK registers at most one handler per name, so a second one would silently win or be
/// dropped. Different kinds have different names and coexist; two of the same kind collide.
fn check_handler_collisions(handlers: &[Arc<dyn InterventionHandler>]) -> Result<()> {
    let mut seen = HashSet::new();
    for handler in handlers {
        if !seen.insert(handler.name()) {
            return Err(HarnessError::InterventionNameCollision {
                name: handler.name().to_owned(),
            });
        }
    }
    Ok(())
}

/// Turns the `interventions` option into SDK handlers (TS `resolveInterventions`). `None` and
/// `off` yield no handler. With no `ask`, a preset interrupts the agent on a gated tool call.
///
/// Not async, unlike TS: Cedar is compiled in rather than imported on demand. A `.cedar` path is
/// read by the SDK's Cedar loader, which reports a missing or invalid file as
/// [`HarnessError::Sdk`].
pub(crate) fn resolve_interventions(
    value: Option<&InterventionsOption>,
    ask: Option<&Ask>,
) -> Result<Vec<Arc<dyn InterventionHandler>>> {
    let mut handlers = Vec::new();
    for value in value.map(InterventionsOption::values).unwrap_or_default() {
        handlers.extend(resolve_one(value, ask)?);
    }
    check_handler_collisions(&handlers)?;
    Ok(handlers)
}

#[cfg(test)]
mod tests;
