//! Intervention sugar (`harness-ts/src/interventions.ts`) — skeleton.
//!
//! In TypeScript, `interventions` coerces presets (`'approve-writes'`, a Cedar policy, a handler)
//! into SDK `InterventionHandler`s that gate tool calls. The Rust SDK has no intervention seam
//! yet, so this module ports the option shape and the resolver's contract only: `None`/
//! [`InterventionsOption::Off`]/an empty list resolve to "no handlers", and anything else is a
//! configuration error instead of being silently ignored (an intervention that does not run is a
//! missing guardrail, not a cosmetic gap).

use strands_agents::{Error, Result};

/// One resolved intervention handler. No handler kind is constructible yet; the type exists so
/// `create_harness` has a stable return shape once the SDK grows an intervention seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterventionHandler {}

impl InterventionHandler {
    /// The handler's registration name (an agent registers at most one per name).
    pub fn name(&self) -> &str {
        match *self {}
    }
}

/// The `interventions` option: off, or one/many named presets.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum InterventionsOption {
    /// No interventions (the default).
    #[default]
    Off,
    /// Named presets, e.g. `"approve-writes"`. Not supported yet.
    Presets(Vec<String>),
}

/// Coerce the `interventions` sugar into handlers.
///
/// `None`, [`InterventionsOption::Off`] and an empty preset list yield no handlers; any named
/// preset is rejected, because the Rust SDK cannot enforce it yet.
pub fn resolve_interventions(value: Option<&InterventionsOption>) -> Result<Vec<InterventionHandler>> {
    match value {
        None | Some(InterventionsOption::Off) => Ok(Vec::new()),
        Some(InterventionsOption::Presets(presets)) if presets.is_empty() => Ok(Vec::new()),
        Some(InterventionsOption::Presets(presets)) => Err(Error::other(format!(
            "interventions are not supported by the Rust SDK yet (requested: {}). Remove the option or gate the tool calls in your own plugin.",
            presets.join(", ")
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_no_interventions_by_default() {
        assert!(resolve_interventions(None).unwrap().is_empty());
        assert!(resolve_interventions(Some(&InterventionsOption::Off))
            .unwrap()
            .is_empty());
        assert!(resolve_interventions(Some(&InterventionsOption::Presets(vec![])))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn rejects_a_requested_preset() {
        let err = resolve_interventions(Some(&InterventionsOption::Presets(vec![
            "approve-writes".to_string()
        ])))
        .unwrap_err()
        .to_string();
        assert!(err.contains("not supported by the Rust SDK yet"), "{err}");
        assert!(err.contains("approve-writes"), "{err}");
    }
}
