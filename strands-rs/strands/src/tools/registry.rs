//! The tool registry (TS `registry/tool-registry.ts`).

use std::fmt;
use std::sync::Arc;

use indexmap::IndexMap;

use crate::error::{Result, ToolNotFoundError, ToolValidationError};
use crate::tools::Tool;
use crate::tools::tool::MAX_TOOL_NAME_LENGTH;

/// The tools an agent can call, in registration order (TS `ToolRegistry`).
///
/// Names are unique, and two names that differ only by `-` versus `_` count as the same.
/// Clones are cheap and independent.
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
    /// [`ToolValidationError`] when the name or description is invalid, or the name is already
    /// registered (also when it differs from a registered name only by `-` versus `_`).
    pub fn add(&mut self, tool: Arc<dyn Tool>) -> Result<()> {
        validate_properties(tool.as_ref())?;
        let name = tool.name().to_owned();
        if self.tools.contains_key(&name) {
            return Err(
                ToolValidationError::new(format!("Tool with name '{name}' already registered")).into(),
            );
        }
        self.check_normalized_conflict(&name)?;
        self.tools.insert(name, tool);
        Ok(())
    }

    /// Registers each tool in order, stopping at the first failure.
    ///
    /// # Errors
    ///
    /// See [`ToolRegistry::add`].
    pub fn add_all(&mut self, tools: impl IntoIterator<Item = Arc<dyn Tool>>) -> Result<()> {
        tools.into_iter().try_for_each(|tool| self.add(tool))
    }

    /// Registers each tool, replacing a registered tool of the same name in place (TS
    /// `addOrReplace`).
    ///
    /// # Errors
    ///
    /// See [`ToolRegistry::add`]; replacing never fails on the name.
    pub fn add_or_replace(&mut self, tools: impl IntoIterator<Item = Arc<dyn Tool>>) -> Result<()> {
        for tool in tools {
            validate_properties(tool.as_ref())?;
            let name = tool.name().to_owned();
            if !self.tools.contains_key(&name) {
                self.check_normalized_conflict(&name)?;
            }
            self.tools.insert(name, tool);
        }
        Ok(())
    }

    /// The tool registered under exactly `name`.
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name)
    }

    /// The tool for `name`: an exact match, then a `_` name matching a `-` name, then a
    /// case-insensitive match (TS `resolve`).
    ///
    /// # Errors
    ///
    /// [`ToolNotFoundError`] when nothing matches.
    pub fn resolve(&self, name: &str) -> Result<&Arc<dyn Tool>> {
        if let Some(tool) = self.tools.get(name) {
            return Ok(tool);
        }
        if name.contains('_')
            && let Some(tool) = self
                .tools
                .values()
                .find(|tool| tool.name().replace('-', "_") == name)
        {
            return Ok(tool);
        }
        let lower = name.to_lowercase();
        self.tools
            .values()
            .find(|tool| tool.name().to_lowercase() == lower)
            .ok_or_else(|| ToolNotFoundError::new(name).into())
    }

    /// Removes the tool named `name`, keeping the order of the rest.
    pub fn remove(&mut self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.shift_remove(name)
    }

    /// Removes every tool.
    pub fn clear(&mut self) {
        self.tools.clear();
    }

    /// The tools in registration order.
    pub fn list(&self) -> Vec<Arc<dyn Tool>> {
        self.tools.values().cloned().collect()
    }

    /// The registered names in registration order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.tools.keys().map(String::as_str)
    }

    /// The number of tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether no tool is registered.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    fn check_normalized_conflict(&self, name: &str) -> Result<()> {
        let normalized = name.replace('-', "_");
        match self
            .tools
            .keys()
            .find(|existing| existing.as_str() != name && existing.replace('-', "_") == normalized)
        {
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
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
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
