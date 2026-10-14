//! Intervention sugar (`harness-ts/src/interventions.ts`).
//!
//! The `interventions` option accepts preset keywords, an SDK handler instance, or a list of
//! these, and coerces them into the [`InterventionHandler`]s that
//! [`strands_agents::agent::AgentBuilder::interventions`] expects.
//!
//! The string grammar is TS's, and deterministic — no content sniffing:
//! `off` yields no handler, `ask` maps to a [`HumanInTheLoop`] config. The Rust SDK's HITL has no
//! LLM risk classifier and ships no Cedar handler, so `smart`, a `*.cedar` path and a
//! natural-language risk policy are configuration errors rather than silently weaker guardrails.

use std::collections::HashSet;
use std::sync::Arc;

use strands_agents::interventions::vended::hitl::{AskFn, HumanInTheLoop};
use strands_agents::interventions::InterventionHandler;
use strands_agents::{Error, Result};

/// One `interventions` entry: a preset/policy string, or a ready-made SDK handler.
#[derive(Clone)]
pub enum InterventionValue {
    /// A preset keyword (`off`/`ask`), or a policy string (unsupported, see the module docs).
    Preset(String),
    /// An SDK handler, passed through untouched.
    Handler(Arc<dyn InterventionHandler>),
}

impl From<&str> for InterventionValue {
    fn from(value: &str) -> Self {
        Self::Preset(value.to_string())
    }
}

/// The `interventions` option: off, or one/many values.
#[derive(Clone, Default)]
pub enum InterventionsOption {
    /// No interventions (the default, TS `undefined`).
    #[default]
    Off,
    /// Preset names and/or handler instances.
    Values(Vec<InterventionValue>),
}

impl InterventionsOption {
    /// Sugar for a list of preset names.
    pub fn presets<S: Into<String>>(presets: impl IntoIterator<Item = S>) -> Self {
        Self::Values(
            presets
                .into_iter()
                .map(|p| InterventionValue::Preset(p.into()))
                .collect(),
        )
    }
}

/// Options for [`resolve_interventions`].
#[derive(Clone, Default)]
pub struct ResolveInterventionsOptions {
    /// How a preset collects approval. `None` leaves the handler in interrupt/resume mode.
    pub ask: Option<AskFn>,
}

fn unsupported(what: &str, value: &str) -> Error {
    Error::other(format!(
        "interventions value '{value}' needs {what}, which the Rust SDK does not provide yet. \
         Use the 'ask' preset, or pass your own InterventionHandler instance."
    ))
}

fn resolve_one(
    value: &InterventionValue,
    ask: Option<&AskFn>,
) -> Result<Option<Arc<dyn InterventionHandler>>> {
    let preset = match value {
        InterventionValue::Handler(handler) => return Ok(Some(handler.clone())),
        InterventionValue::Preset(preset) => preset.trim(),
    };
    match preset {
        "off" => Ok(None),
        "ask" => {
            let mut hitl = HumanInTheLoop::new();
            if let Some(ask) = ask {
                hitl = hitl.ask(ask.clone());
            }
            Ok(Some(Arc::new(hitl)))
        }
        "smart" => Err(unsupported("the LLM risk classifier", preset)),
        other if other.ends_with(".cedar") => Err(unsupported("a Cedar policy handler", other)),
        other => Err(unsupported(
            "a natural-language risk policy (the LLM risk classifier)",
            other,
        )),
    }
}

/// Raise if two handlers share a name — the SDK registers at most one per `name`, so a second
/// would silently win or be dropped.
fn check_handler_collisions(handlers: &[Arc<dyn InterventionHandler>]) -> Result<()> {
    let mut seen: HashSet<&str> = HashSet::new();
    for handler in handlers {
        if !seen.insert(handler.name()) {
            return Err(Error::other(format!(
                "Two interventions share the handler name '{}', but an agent registers at most \
                 one per name. Layer different kinds (e.g. a Cedar policy plus one human-approval \
                 preset), not two of the same kind.",
                handler.name()
            )));
        }
    }
    Ok(())
}

/// Coerce the `interventions` sugar into SDK handlers. `Off`/`"off"` yield `[]`.
pub fn resolve_interventions(
    value: &InterventionsOption,
    options: &ResolveInterventionsOptions,
) -> Result<Vec<Arc<dyn InterventionHandler>>> {
    let values = match value {
        InterventionsOption::Off => return Ok(Vec::new()),
        InterventionsOption::Values(values) => values,
    };
    let mut handlers = Vec::new();
    for value in values {
        if let Some(handler) = resolve_one(value, options.ask.as_ref())? {
            handlers.push(handler);
        }
    }
    check_handler_collisions(&handlers)?;
    Ok(handlers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(value: &InterventionsOption) -> Result<Vec<Arc<dyn InterventionHandler>>> {
        resolve_interventions(value, &ResolveInterventionsOptions::default())
    }

    fn resolve_err(value: &InterventionsOption) -> String {
        match resolve(value) {
            Ok(_) => panic!("expected an error"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn resolves_no_interventions_by_default() {
        assert!(resolve(&InterventionsOption::default()).unwrap().is_empty());
        assert!(resolve(&InterventionsOption::Values(vec![])).unwrap().is_empty());
        assert!(resolve(&InterventionsOption::presets(["off"]))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn resolves_the_ask_preset_to_a_hitl_handler() {
        let handlers = resolve(&InterventionsOption::presets(["ask"])).unwrap();
        assert_eq!(handlers.len(), 1);
        assert_eq!(handlers[0].name(), "strands:human-in-the-loop");
    }

    #[test]
    fn passes_a_handler_instance_through() {
        let handler: Arc<dyn InterventionHandler> = Arc::new(HumanInTheLoop::new().allowed_tools(["*"]));
        let handlers = resolve(&InterventionsOption::Values(vec![InterventionValue::Handler(
            handler.clone(),
        )]))
        .unwrap();
        assert_eq!(handlers.len(), 1);
        assert_eq!(handlers[0].name(), handler.name());
    }

    #[test]
    fn rejects_the_classifier_preset_and_policy_strings() {
        for (value, needle) in [
            ("smart", "LLM risk classifier"),
            ("./policy.cedar", "Cedar policy handler"),
            ("never delete files", "natural-language risk policy"),
        ] {
            let err = resolve_err(&InterventionsOption::presets([value]));
            assert!(err.contains(needle), "{err}");
            assert!(err.contains(value), "{err}");
        }
    }

    #[test]
    fn rejects_two_handlers_sharing_a_name() {
        let err = resolve_err(&InterventionsOption::presets(["ask", "ask"]));
        assert!(
            err.contains("Two interventions share the handler name 'strands:human-in-the-loop'"),
            "{err}"
        );
        assert!(err.contains("an agent registers at most one per name"), "{err}");
    }

    #[tokio::test]
    async fn threads_the_ask_callback_into_the_preset() {
        use strands_agents::types::JsonValue;
        let ask: AskFn = Arc::new(|_prompt| Box::pin(async { Ok(JsonValue::String("n".into())) }));
        let handlers = resolve_interventions(
            &InterventionsOption::presets(["ask"]),
            &ResolveInterventionsOptions { ask: Some(ask) },
        )
        .unwrap();
        assert_eq!(handlers.len(), 1);
    }
}
