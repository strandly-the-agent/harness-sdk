//! The tool registry (TS `registry/tool-registry.ts`).

use std::fmt;
use std::sync::Arc;

use indexmap::IndexMap;

use crate::error::{ToolNotFoundError, ToolValidationError};
use crate::tools::tool::{MAX_TOOL_NAME_LENGTH, Tool, is_tool_name_byte};

/// The tools an agent can call, by name, in registration order (TS `ToolRegistry`).
///
/// ```
/// use std::sync::Arc;
/// use strands::tools::{FunctionTool, Tool, ToolRegistry};
///
/// let weather = FunctionTool::new("get-weather", "Gets the weather.", |_input, _ctx| async { Ok("sunny") });
/// let mut registry = ToolRegistry::new();
/// registry.add(Arc::new(weather))?;
/// assert_eq!(registry.resolve("get_weather")?.name(), "get-weather");
/// # Ok::<(), strands::Error>(())
/// ```
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: IndexMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A registry holding `tools` (TS `new ToolRegistry(tools)`).
    ///
    /// # Errors
    ///
    /// As [`add_all`](Self::add_all).
    pub fn with_tools(tools: impl IntoIterator<Item = Arc<dyn Tool>>) -> Result<Self, ToolValidationError> {
        let mut registry = Self::new();
        registry.add_all(tools)?;
        Ok(registry)
    }

    /// Registers `tool` (TS `add`).
    ///
    /// # Errors
    ///
    /// [`ToolValidationError`] when the tool's name or description is invalid, the name is already
    /// registered, or it differs from a registered name only by `-` versus `_`.
    pub fn add(&mut self, tool: Arc<dyn Tool>) -> Result<(), ToolValidationError> {
        validate_properties(tool.as_ref())?;
        let name = tool.name();
        if self.tools.contains_key(name) {
            return Err(ToolValidationError::new(format!(
                "Tool with name '{name}' already registered"
            )));
        }
        self.check_normalized_conflict(name)?;
        self.tools.insert(name.to_owned(), tool);
        Ok(())
    }

    /// Registers each of `tools` in order, stopping at the first invalid one (TS `add` with an
    /// array).
    ///
    /// # Errors
    ///
    /// As [`add`](Self::add); tools before the failing one stay registered.
    pub fn add_all(
        &mut self,
        tools: impl IntoIterator<Item = Arc<dyn Tool>>,
    ) -> Result<(), ToolValidationError> {
        tools.into_iter().try_for_each(|tool| self.add(tool))
    }

    /// Registers each of `tools`, replacing a registered tool of the same name in place (TS
    /// `addOrReplace`).
    ///
    /// # Errors
    ///
    /// [`ToolValidationError`] when a tool's name or description is invalid, or a new name differs
    /// from a registered one only by `-` versus `_`.
    pub fn add_or_replace(
        &mut self,
        tools: impl IntoIterator<Item = Arc<dyn Tool>>,
    ) -> Result<(), ToolValidationError> {
        for tool in tools {
            validate_properties(tool.as_ref())?;
            let name = tool.name();
            if !self.tools.contains_key(name) {
                self.check_normalized_conflict(name)?;
            }
            self.tools.insert(name.to_owned(), tool);
        }
        Ok(())
    }

    /// The tool registered under exactly `name`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name)
    }

    /// The tool the model means by `name` (TS `resolve`): an exact match, then a registered name
    /// equal to `name` once its `-` become `_`, then a case-insensitive match.
    ///
    /// # Errors
    ///
    /// [`ToolNotFoundError`] when nothing matches.
    pub fn resolve(&self, name: &str) -> Result<&Arc<dyn Tool>, ToolNotFoundError> {
        if let Some(tool) = self.tools.get(name) {
            return Ok(tool);
        }
        let underscored = name
            .contains('_')
            .then(|| {
                self.tools
                    .values()
                    .find(|tool| tool.name().replace('-', "_") == name)
            })
            .flatten();
        let lower = name.to_lowercase();
        underscored
            .or_else(|| {
                self.tools
                    .values()
                    .find(|tool| tool.name().to_lowercase() == lower)
            })
            .ok_or_else(|| ToolNotFoundError::new(name))
    }

    /// Removes the tool named `name`; does nothing when there is none (TS `remove`).
    pub fn remove(&mut self, name: &str) {
        self.tools.shift_remove(name);
    }

    /// Removes every tool (TS `clear`).
    pub fn clear(&mut self) {
        self.tools.clear();
    }

    /// The registered tools in registration order (TS `list`).
    #[must_use]
    pub fn list(&self) -> Vec<Arc<dyn Tool>> {
        self.tools.values().cloned().collect()
    }

    fn check_normalized_conflict(&self, name: &str) -> Result<(), ToolValidationError> {
        let normalized = name.replace('-', "_");
        let existing = self
            .tools
            .keys()
            .find(|existing| existing.as_str() != name && existing.replace('-', "_") == normalized);
        match existing {
            Some(existing) => Err(ToolValidationError::new(format!(
                "Tool name '{name}' already exists as '{existing}'. Cannot add a duplicate tool which differs by a '-' or '_'"
            ))),
            None => Ok(()),
        }
    }
}

impl fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.tools.keys()).finish()
    }
}

fn validate_properties(tool: &dyn Tool) -> Result<(), ToolValidationError> {
    let name = tool.name();
    if !(1..=MAX_TOOL_NAME_LENGTH).contains(&name.encode_utf16().count()) {
        return Err(ToolValidationError::new(format!(
            "Tool name must be between 1 and {MAX_TOOL_NAME_LENGTH} characters"
        )));
    }
    if !name.bytes().all(is_tool_name_byte) {
        return Err(ToolValidationError::new(
            "Tool name must contain only alphanumeric characters, hyphens, and underscores",
        ));
    }
    if tool.description().is_empty() {
        return Err(ToolValidationError::new(
            "Tool description must be a non-empty string",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
