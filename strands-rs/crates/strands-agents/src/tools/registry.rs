//! Insertion-ordered tool registry (`strands-ts/src/registry/tool-registry.ts`).

use indexmap::IndexMap;

use super::tool::ToolRef;
use crate::error::{Error, Result};
use crate::types::ToolSpec;

/// Maximum tool name length.
pub const MAX_TOOL_NAME_LENGTH: usize = 64;

/// Registry of tools keyed by name; `list()` preserves insertion order.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: IndexMap<String, ToolRef>,
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.tools.keys()).finish()
    }
}

fn validate(tool: &dyn super::Tool) -> Result<()> {
    let name = tool.name();
    if name.is_empty() || name.len() > MAX_TOOL_NAME_LENGTH {
        return Err(Error::ToolValidation(format!(
            "Tool name must be between 1 and {MAX_TOOL_NAME_LENGTH} characters"
        )));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(Error::ToolValidation(
            "Tool name must contain only alphanumeric characters, hyphens, and underscores".into(),
        ));
    }
    if tool.description().is_empty() {
        return Err(Error::ToolValidation(
            "Tool description must be a non-empty string".into(),
        ));
    }
    Ok(())
}

impl ToolRegistry {
    /// Empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registry pre-populated with `tools`.
    pub fn with_tools(tools: impl IntoIterator<Item = ToolRef>) -> Result<Self> {
        let mut registry = Self::new();
        for tool in tools {
            registry.add(tool)?;
        }
        Ok(registry)
    }

    fn check_normalized_conflict(&self, name: &str) -> Result<()> {
        let normalized = name.replace('-', "_");
        for existing in self.tools.keys() {
            if existing != name && existing.replace('-', "_") == normalized {
                return Err(Error::ToolValidation(format!(
                    "Tool name '{name}' already exists as '{existing}'. Cannot add a duplicate tool which differs by a '-' or '_'"
                )));
            }
        }
        Ok(())
    }

    /// Register a tool; rejects duplicates and `-`/`_` near-duplicates.
    pub fn add(&mut self, tool: ToolRef) -> Result<()> {
        validate(tool.as_ref())?;
        let name = tool.name().to_owned();
        if self.tools.contains_key(&name) {
            return Err(Error::ToolValidation(format!(
                "Tool with name '{name}' already registered"
            )));
        }
        self.check_normalized_conflict(&name)?;
        self.tools.insert(name, tool);
        Ok(())
    }

    /// Register or overwrite tools by exact name.
    pub fn add_or_replace(&mut self, tools: impl IntoIterator<Item = ToolRef>) -> Result<()> {
        for tool in tools {
            validate(tool.as_ref())?;
            let name = tool.name().to_owned();
            if !self.tools.contains_key(&name) {
                self.check_normalized_conflict(&name)?;
            }
            self.tools.insert(name, tool);
        }
        Ok(())
    }

    /// Exact-name lookup.
    pub fn get(&self, name: &str) -> Option<ToolRef> {
        self.tools.get(name).cloned()
    }

    /// Fuzzy lookup: exact, then `-`→`_` normalized (only when `name` contains `_`), then
    /// case-insensitive.
    pub fn resolve(&self, name: &str) -> Result<ToolRef> {
        if let Some(t) = self.tools.get(name) {
            return Ok(t.clone());
        }
        if name.contains('_') {
            if let Some(t) = self.tools.values().find(|t| t.name().replace('-', "_") == name) {
                return Ok(t.clone());
            }
        }
        if let Some(t) = self.tools.values().find(|t| t.name().eq_ignore_ascii_case(name)) {
            return Ok(t.clone());
        }
        Err(Error::ToolNotFound {
            tool_name: name.to_owned(),
        })
    }

    /// Remove a tool; silent when absent.
    pub fn remove(&mut self, name: &str) {
        self.tools.shift_remove(name);
    }

    /// Remove all tools.
    pub fn clear(&mut self) {
        self.tools.clear();
    }

    /// All tools in insertion order.
    pub fn list(&self) -> Vec<ToolRef> {
        self.tools.values().cloned().collect()
    }

    /// Specs of all tools in insertion order.
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|t| t.spec().clone()).collect()
    }

    /// Number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// `true` when no tools are registered.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;

    use super::*;
    use crate::tools::{Tool, ToolContext};
    use crate::types::ToolResultBlock;

    struct Named(ToolSpec);

    #[async_trait]
    impl Tool for Named {
        fn name(&self) -> &str {
            &self.0.name
        }
        fn description(&self) -> &str {
            &self.0.description
        }
        fn spec(&self) -> &ToolSpec {
            &self.0
        }
        async fn invoke(&self, ctx: ToolContext) -> Result<ToolResultBlock> {
            Ok(ToolResultBlock::success(&ctx.tool_use.tool_use_id, vec![]))
        }
    }

    fn tool(name: &str) -> ToolRef {
        Arc::new(Named(ToolSpec::new(name, "d")))
    }

    #[test]
    fn add_list_get_remove_preserve_order() {
        let mut r = ToolRegistry::new();
        r.add(tool("b")).unwrap();
        r.add(tool("a")).unwrap();
        assert_eq!(
            r.list().iter().map(|t| t.name().to_owned()).collect::<Vec<_>>(),
            vec!["b", "a"]
        );
        assert!(r.get("a").is_some());
        r.remove("b");
        r.remove("missing");
        assert_eq!(r.len(), 1);
        r.clear();
        assert!(r.is_empty());
    }

    #[test]
    fn validation_messages() {
        let mut r = ToolRegistry::new();
        let err = r.add(tool("")).unwrap_err().to_string();
        assert_eq!(err, "Tool name must be between 1 and 64 characters");
        let err = r.add(tool("bad name")).unwrap_err().to_string();
        assert_eq!(
            err,
            "Tool name must contain only alphanumeric characters, hyphens, and underscores"
        );
        let err = r
            .add(Arc::new(Named(ToolSpec::new("ok", ""))))
            .unwrap_err()
            .to_string();
        assert_eq!(err, "Tool description must be a non-empty string");
    }

    #[test]
    fn duplicates_and_normalized_conflicts() {
        let mut r = ToolRegistry::new();
        r.add(tool("my-tool")).unwrap();
        assert_eq!(
            r.add(tool("my-tool")).unwrap_err().to_string(),
            "Tool with name 'my-tool' already registered"
        );
        assert_eq!(
            r.add(tool("my_tool")).unwrap_err().to_string(),
            "Tool name 'my_tool' already exists as 'my-tool'. Cannot add a duplicate tool which differs by a '-' or '_'"
        );
        r.add_or_replace(vec![tool("my-tool")]).unwrap();
        assert!(r.add_or_replace(vec![tool("my_tool")]).is_err());
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn resolve_precedence() {
        let mut r = ToolRegistry::new();
        r.add(tool("my-tool")).unwrap();
        r.add(tool("Other")).unwrap();
        assert_eq!(r.resolve("my-tool").unwrap().name(), "my-tool");
        assert_eq!(r.resolve("my_tool").unwrap().name(), "my-tool");
        assert_eq!(r.resolve("other").unwrap().name(), "Other");
        match r.resolve("nope") {
            Err(Error::ToolNotFound { tool_name }) => assert_eq!(tool_name, "nope"),
            Err(other) => panic!("unexpected {other:?}"),
            Ok(_) => panic!("expected ToolNotFound"),
        }
    }
}
