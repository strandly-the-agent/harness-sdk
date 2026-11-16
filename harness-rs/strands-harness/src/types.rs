//! Option types for `create_harness` (TS `types/agent.ts`).

use serde::{Deserialize, Serialize};

/// Reasoning effort applied to the resolved model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Effort {
    /// The harness picks the effort for the model.
    #[default]
    Auto,
    /// No reasoning.
    Off,
    /// The least reasoning the model offers.
    Minimal,
    /// Low effort.
    Low,
    /// Medium effort.
    Medium,
    /// High effort.
    High,
    /// Extra-high effort.
    Xhigh,
    /// The most reasoning the model offers.
    Max,
}

/// How `web_fetch` makes its request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum WebFetchTransport {
    /// `curl` inside the agent's sandbox, so the sandbox's network controls apply.
    #[default]
    Curl,
    /// A request from the harness process, bypassing the sandbox.
    Direct,
}
