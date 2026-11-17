//! Option types for `create_harness` (TS `types/agent.ts`).

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use strands::agent::ModelSource;
use strands::experimental::context_manager::ContextManagerStrategy;
use strands::memory::MemoryStore;

use crate::config::{
    ProgrammaticToolCallerConfig, ReadConfig, ShellConfig, SubagentConfig, WebSearchSetting,
};
use crate::error::HarnessError;
use crate::models::EFFORT_LEVELS;

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

impl Effort {
    /// The level's name as TS and a config write it (`"high"`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

impl std::fmt::Display for Effort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Parses a level's name; any other text is [`HarnessError::InvalidEffort`].
///
/// ```
/// use strands_harness::Effort;
///
/// let effort: Effort = "high".parse()?;
/// assert_eq!(effort, Effort::High);
/// assert_eq!(effort.to_string(), "high");
/// assert!("turbo".parse::<Effort>().is_err());
/// # Ok::<(), strands_harness::HarnessError>(())
/// ```
impl std::str::FromStr for Effort {
    type Err = HarnessError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        EFFORT_LEVELS
            .into_iter()
            .find(|effort| effort.as_str() == name)
            .ok_or_else(|| HarnessError::InvalidEffort {
                effort: name.to_owned(),
                supported: EFFORT_LEVELS.map(Self::as_str).join(", "),
            })
    }
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

/// A `boolean | C` setting: on, off, or on with a config.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    from = "SettingRepr<C>",
    into = "SettingRepr<C>",
    bound(serialize = "C: Clone + Serialize", deserialize = "C: Deserialize<'de>")
)]
#[non_exhaustive]
pub enum Setting<C> {
    /// `true`: enabled with the defaults.
    #[default]
    On,
    /// `false`: disabled.
    Off,
    /// Enabled with this config.
    Config(C),
}

impl<C> Setting<C> {
    /// Whether the setting is anything but [`Off`](Self::Off).
    pub fn is_enabled(&self) -> bool {
        !matches!(self, Self::Off)
    }

    /// The config, when one was given.
    pub fn config(&self) -> Option<&C> {
        match self {
            Self::Config(config) => Some(config),
            Self::On | Self::Off => None,
        }
    }
}

impl<C> From<bool> for Setting<C> {
    fn from(enabled: bool) -> Self {
        if enabled { Self::On } else { Self::Off }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum SettingRepr<C> {
    Bool(bool),
    Config(C),
}

impl<C> From<SettingRepr<C>> for Setting<C> {
    fn from(repr: SettingRepr<C>) -> Self {
        match repr {
            SettingRepr::Bool(enabled) => enabled.into(),
            SettingRepr::Config(config) => Self::Config(config),
        }
    }
}

impl<C> From<Setting<C>> for SettingRepr<C> {
    fn from(setting: Setting<C>) -> Self {
        match setting {
            Setting::On => Self::Bool(true),
            Setting::Off => Self::Bool(false),
            Setting::Config(config) => Self::Config(config),
        }
    }
}

/// Per-tool config for the `web_fetch` built-in: `HarnessConfigWebFetch` with a live model allowed.
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(state_mod(vis = "pub"), derive(Debug))]
pub struct WebFetchConfig {
    /// Model the summarizer runs on: an instance, a router (its default model is used), or a
    /// `"provider/name"` string. `None` uses the small fast model of the main agent's provider.
    #[builder(into)]
    pub model: Option<ModelSource>,
    /// How the request is made; `None` is [`WebFetchTransport::Curl`].
    pub transport: Option<WebFetchTransport>,
}

/// Edits to the default built-in tool set. `all` is the TS `'*'` key (default `true`): `false`
/// starts from nothing so tools are enabled one by one.
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(state_mod(vis = "pub"), derive(Debug))]
pub struct BuiltinToolsConfig {
    /// The `'*'` key: whether the harness's default tools are the baseline.
    pub all: Option<bool>,
    /// The `shell` tool.
    #[builder(into)]
    pub shell: Option<Setting<ShellConfig>>,
    /// The `read` tool.
    #[builder(into)]
    pub read: Option<Setting<ReadConfig>>,
    /// The `write` tool.
    pub write: Option<bool>,
    /// The `edit` tool.
    pub edit: Option<bool>,
    /// The `web_fetch` tool.
    #[builder(into)]
    pub web_fetch: Option<Setting<WebFetchConfig>>,
    /// The `web_search` tool.
    #[builder(into)]
    pub web_search: Option<WebSearchSetting>,
    /// The `programmatic_tool_caller` tool.
    #[builder(into)]
    pub programmatic_tool_caller: Option<Setting<ProgrammaticToolCallerConfig>>,
    /// The `subagent` tool.
    #[builder(into)]
    pub subagent: Option<Setting<SubagentConfig>>,
}

/// Session persistence; the default value selects the defaults.
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, bon::Builder)]
#[builder(on(String, into), state_mod(vis = "pub"), derive(Debug))]
#[serde(deny_unknown_fields)]
pub struct SessionConfig {
    /// Identifier of the session's stored state (sanitized to `[a-z0-9_-]`). Defaults to a fresh
    /// random 8-hex id each run; pass a previous run's id to resume it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Root directory for session state and offloaded artifacts. Defaults to `./.agent/sessions`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
}

/// The harness's default long-term memory; the default value selects the defaults.
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(on(String, into), state_mod(vis = "pub"), derive(Debug))]
pub struct MemoryConfig {
    /// Directory of the default file store. Defaults to `./.agent/memory`.
    pub dir: Option<String>,
    /// Stores that replace the default file store while keeping the harness's memory policy; the
    /// `subagent` delegate shares them read-only.
    pub stores: Option<Vec<Arc<dyn MemoryStore>>>,
}

/// What the `context_manager` option accepts: the SDK's strategy type, whose `Off` disables
/// SDK-managed context.
pub type ContextManagerOption = ContextManagerStrategy;
