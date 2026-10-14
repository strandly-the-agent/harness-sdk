//! Default configuration for the harness (`harness-ts/src/defaults.ts`).

/// Default model, as `provider/name`.
pub const DEFAULT_MODEL: &str = "bedrock/global.anthropic.claude-opus-5";
/// Default reasoning effort.
pub const DEFAULT_EFFORT: &str = "auto";
/// Default context manager preset.
pub const DEFAULT_CONTEXT_MANAGER: &str = "auto";
/// Default prompt-caching mode.
pub const DEFAULT_CACHING: &str = "auto";
/// Built-in tools enabled by default, in registration order.
pub const DEFAULT_BUILTIN_TOOLS: &[&str] = &[
    "shell",
    "read",
    "write",
    "edit",
    "web_fetch",
    "web_search",
    "programmatic_tool_caller",
    "subagent",
];
/// Built-in plugins enabled by default.
pub const DEFAULT_BUILTIN_PLUGINS: &[&str] = &["todos", "environment"];
/// Maximum nesting depth for subagents.
pub const DEFAULT_SUBAGENT_MAX_DEPTH: u32 = 2;
/// Sessions are on by default.
pub const DEFAULT_SESSION: bool = true;
/// Default session directory.
pub const DEFAULT_SESSION_DIR: &str = "./.agent/sessions";
/// Skills are on by default (when the directory exists).
pub const DEFAULT_SKILLS: bool = true;
/// Default skills directory.
pub const DEFAULT_SKILLS_DIR: &str = "./.agent/skills";
/// Memory is on by default.
pub const DEFAULT_MEMORY: bool = true;
/// Default memory directory.
pub const DEFAULT_MEMORY_DIR: &str = "./.agent/memory";
