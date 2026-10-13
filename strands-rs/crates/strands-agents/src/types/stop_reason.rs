//! Stop reasons (`strands-ts/src/types/messages.ts` `StopReason`). Open set: unknown provider
//! strings round-trip through `Other`.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Why the model (or agent loop) stopped producing output.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StopReason {
    /// The invocation was cancelled.
    Cancelled,
    /// Stopped at a checkpoint.
    Checkpoint,
    /// Content was filtered by the provider.
    ContentFiltered,
    /// Natural end of the assistant turn.
    EndTurn,
    /// A guardrail intervened.
    GuardrailIntervened,
    /// Paused for an interrupt.
    Interrupt,
    /// The model hit its output-token cap.
    MaxTokens,
    /// Agent output-token limit reached.
    LimitOutputTokens,
    /// Agent total-token limit reached.
    LimitTotalTokens,
    /// Agent turn limit reached.
    LimitTurns,
    /// Provider paused the turn.
    PauseTurn,
    /// The model refused.
    Refusal,
    /// A stop sequence was hit.
    StopSequence,
    /// The model requested tool use.
    ToolUse,
    /// The model's context window was exceeded.
    ModelContextWindowExceeded,
    /// Any other provider-specific value.
    Other(String),
}

impl StopReason {
    /// Wire string (camelCase).
    pub fn as_str(&self) -> &str {
        match self {
            StopReason::Cancelled => "cancelled",
            StopReason::Checkpoint => "checkpoint",
            StopReason::ContentFiltered => "contentFiltered",
            StopReason::EndTurn => "endTurn",
            StopReason::GuardrailIntervened => "guardrailIntervened",
            StopReason::Interrupt => "interrupt",
            StopReason::MaxTokens => "maxTokens",
            StopReason::LimitOutputTokens => "limitOutputTokens",
            StopReason::LimitTotalTokens => "limitTotalTokens",
            StopReason::LimitTurns => "limitTurns",
            StopReason::PauseTurn => "pauseTurn",
            StopReason::Refusal => "refusal",
            StopReason::StopSequence => "stopSequence",
            StopReason::ToolUse => "toolUse",
            StopReason::ModelContextWindowExceeded => "modelContextWindowExceeded",
            StopReason::Other(s) => s,
        }
    }
}

impl FromStr for StopReason {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "cancelled" => StopReason::Cancelled,
            "checkpoint" => StopReason::Checkpoint,
            "contentFiltered" => StopReason::ContentFiltered,
            "endTurn" => StopReason::EndTurn,
            "guardrailIntervened" => StopReason::GuardrailIntervened,
            "interrupt" => StopReason::Interrupt,
            "maxTokens" => StopReason::MaxTokens,
            "limitOutputTokens" => StopReason::LimitOutputTokens,
            "limitTotalTokens" => StopReason::LimitTotalTokens,
            "limitTurns" => StopReason::LimitTurns,
            "pauseTurn" => StopReason::PauseTurn,
            "refusal" => StopReason::Refusal,
            "stopSequence" => StopReason::StopSequence,
            "toolUse" => StopReason::ToolUse,
            "modelContextWindowExceeded" => StopReason::ModelContextWindowExceeded,
            other => StopReason::Other(other.to_owned()),
        })
    }
}

impl From<&str> for StopReason {
    fn from(s: &str) -> Self {
        s.parse().expect("infallible")
    }
}

impl fmt::Display for StopReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for StopReason {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for StopReason {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(String::deserialize(d)?.as_str().into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_values_round_trip() {
        for s in ["endTurn", "toolUse", "maxTokens", "modelContextWindowExceeded"] {
            let r: StopReason = s.into();
            assert_ne!(r, StopReason::Other(s.into()));
            assert_eq!(r.as_str(), s);
            assert_eq!(serde_json::to_string(&r).unwrap(), format!("\"{s}\""));
        }
    }

    #[test]
    fn unknown_values_are_preserved() {
        let r: StopReason = serde_json::from_str("\"provider_special\"").unwrap();
        assert_eq!(r, StopReason::Other("provider_special".into()));
        assert_eq!(serde_json::to_string(&r).unwrap(), "\"provider_special\"");
    }
}
