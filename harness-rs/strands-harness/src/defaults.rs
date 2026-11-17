//! Default configuration for the harness (TS `defaults.ts`).
//!
//! TS `DEFAULT_SESSION`, `DEFAULT_SKILLS` and `DEFAULT_MEMORY` (all `true`) are the `#[default]`
//! variants of `SessionOption`, `SkillsOption` and `MemoryOption`, and `DEFAULT_BUILTIN_PLUGINS`
//! is `BUILTIN_PLUGIN_NAMES`.

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
pub(crate) const DEFAULT_SUBAGENT_MAX_DEPTH: u32 = 2;
pub(crate) const DEFAULT_SESSION_DIR: &str = "./.agent/sessions";
pub(crate) const DEFAULT_SKILLS_DIR: &str = "./.agent/skills";
pub(crate) const DEFAULT_MEMORY_DIR: &str = "./.agent/memory";

#[cfg(test)]
mod tests {
    //! The defaults that Rust keeps on its option types, against TS `defaults.ts`.

    use crate::agent::{MemoryOption, SessionOption, SkillsOption};
    use crate::config::BUILTIN_PLUGIN_NAMES;

    #[test]
    fn session_skills_and_memory_are_on_by_default() {
        assert!(matches!(SessionOption::default(), SessionOption::On));
        assert!(matches!(SkillsOption::default(), SkillsOption::On));
        assert!(matches!(MemoryOption::default(), MemoryOption::On));
    }

    #[test]
    fn the_default_built_in_plugins_are_todos_and_environment() {
        let names: Vec<_> = BUILTIN_PLUGIN_NAMES.iter().map(|name| name.as_str()).collect();
        assert_eq!(names, ["todos", "environment"]);
    }
}
