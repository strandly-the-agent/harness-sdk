//! Default configuration for the harness (TS `defaults.ts`).
#![expect(
    dead_code,
    reason = "each constant is used once the module that reads it lands; drop this when all are"
)]

pub(crate) const DEFAULT_MODEL: &str = "bedrock/global.anthropic.claude-opus-5";
pub(crate) const DEFAULT_BUILTIN_TOOLS: [&str; 8] = [
    "shell",
    "read",
    "write",
    "edit",
    "web_fetch",
    "web_search",
    "programmatic_tool_caller",
    "subagent",
];
pub(crate) const DEFAULT_BUILTIN_PLUGINS: [&str; 2] = ["todos", "environment"];
pub(crate) const DEFAULT_SUBAGENT_MAX_DEPTH: u32 = 2;
pub(crate) const DEFAULT_SESSION: bool = true;
pub(crate) const DEFAULT_SESSION_DIR: &str = "./.agent/sessions";
pub(crate) const DEFAULT_SKILLS: bool = true;
pub(crate) const DEFAULT_SKILLS_DIR: &str = "./.agent/skills";
pub(crate) const DEFAULT_MEMORY: bool = true;
pub(crate) const DEFAULT_MEMORY_DIR: &str = "./.agent/memory";
