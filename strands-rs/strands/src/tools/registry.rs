//! The agent's tool registry (TS `registry/tool-registry.ts`).

use std::fmt;
use std::sync::Arc;

use indexmap::IndexMap;

use crate::error::{Result, ToolNotFoundError, ToolValidationError};
use crate::tools::tool::Tool;
use crate::types::tools::ToolSpec;

/// TS `MAX_TOOL_NAME_LENGTH`.
const MAX_TOOL_NAME_LENGTH: usize = 64;

/// The tools an agent can call, by name, in insertion order (TS `ToolRegistry`).
///
/// Cloning copies the name map; the tools themselves are shared.
///
/// ```
/// use std::sync::Arc;
/// use strands::tools::{FunctionTool, ToolRegistry};
///
/// let mut registry = ToolRegistry::new();
/// registry.add(Arc::new(FunctionTool::new("get-weather", "Get the weather.", |_input, _ctx| async {
///     Ok::<_, strands::ToolError>("sunny")
/// })))?;
/// assert_eq!(registry.resolve("get_weather")?.name(), "get-weather");
/// # Ok::<(), strands::Error>(())
/// ```
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: IndexMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `tool`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::ToolValidation`] when the name or description is invalid, the name
    /// is taken, or it differs from a registered name only by `-` versus `_`.
    pub fn add(&mut self, tool: Arc<dyn Tool>) -> Result<()> {
        validate_properties(tool.as_ref())?;
        if self.tools.contains_key(tool.name()) {
            return Err(ToolValidationError::new(format!(
                "Tool with name '{}' already registered",
                tool.name()
            ))
            .into());
        }
        self.check_normalized_conflict(tool.name())?;
        self.tools.insert(tool.name().to_owned(), tool);
        Ok(())
    }

    /// Registers each tool in order, stopping at the first failure; earlier tools stay
    /// registered, as in TS.
    ///
    /// # Errors
    ///
    /// As [`Self::add`].
    pub fn add_all(&mut self, tools: impl IntoIterator<Item = Arc<dyn Tool>>) -> Result<()> {
        tools.into_iter().try_for_each(|tool| self.add(tool))
    }

    /// Registers each tool, replacing a registered tool of the same name in place (TS
    /// `addOrReplace`).
    ///
    /// # Errors
    ///
    /// As [`Self::add`], except that an exact name match replaces instead of failing.
    pub fn add_or_replace(&mut self, tools: impl IntoIterator<Item = Arc<dyn Tool>>) -> Result<()> {
        for tool in tools {
            validate_properties(tool.as_ref())?;
            if !self.tools.contains_key(tool.name()) {
                self.check_normalized_conflict(tool.name())?;
            }
            self.tools.insert(tool.name().to_owned(), tool);
        }
        Ok(())
    }

    /// The tool registered under exactly `name`.
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name)
    }

    /// The tool for a model-supplied `name` (TS `resolve`): an exact match, then a match with
    /// `-` read as `_` when `name` has an underscore, then a case-insensitive match.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::ToolNotFound`] when nothing matches.
    pub fn resolve(&self, name: &str) -> Result<&Arc<dyn Tool>> {
        if let Some(tool) = self.tools.get(name) {
            return Ok(tool);
        }
        let underscored = name
            .contains('_')
            .then(|| self.find(|tool| tool.name().replace('-', "_") == name))
            .flatten();
        let lower = name.to_lowercase();
        underscored
            .or_else(|| self.find(|tool| tool.name().to_lowercase() == lower))
            .ok_or_else(|| ToolNotFoundError::new(name).into())
    }

    fn find(&self, matches: impl Fn(&dyn Tool) -> bool) -> Option<&Arc<dyn Tool>> {
        self.tools.values().find(|tool| matches(tool.as_ref()))
    }

    /// Removes the tool registered under `name`, keeping the order of the rest.
    pub fn remove(&mut self, name: &str) {
        self.tools.shift_remove(name);
    }

    /// Removes every tool.
    pub fn clear(&mut self) {
        self.tools.clear();
    }

    /// The registered tools in insertion order.
    pub fn list(&self) -> impl ExactSizeIterator<Item = &Arc<dyn Tool>> {
        self.tools.values()
    }

    /// The specs of the registered tools in insertion order.
    pub fn tool_specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|tool| tool.tool_spec()).collect()
    }

    /// The number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether no tools are registered.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    fn check_normalized_conflict(&self, name: &str) -> Result<()> {
        let normalized = name.replace('-', "_");
        let conflict = self
            .tools
            .keys()
            .find(|existing| *existing != name && existing.replace('-', "_") == normalized);
        match conflict {
            Some(existing) => Err(ToolValidationError::new(format!(
                "Tool name '{name}' already exists as '{existing}'. Cannot add a duplicate tool which differs by a '-' or '_'"
            ))
            .into()),
            None => Ok(()),
        }
    }
}

fn validate_properties(tool: &dyn Tool) -> Result<()> {
    let name = tool.name();
    let length = name.chars().count();
    if !(1..=MAX_TOOL_NAME_LENGTH).contains(&length) {
        return Err(ToolValidationError::new(format!(
            "Tool name must be between 1 and {MAX_TOOL_NAME_LENGTH} characters"
        ))
        .into());
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(ToolValidationError::new(
            "Tool name must contain only alphanumeric characters, hyphens, and underscores",
        )
        .into());
    }
    if tool.description().is_empty() {
        return Err(ToolValidationError::new("Tool description must be a non-empty string").into());
    }
    Ok(())
}

impl fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.tools.keys()).finish()
    }
}

#[cfg(test)]
mod tests;
