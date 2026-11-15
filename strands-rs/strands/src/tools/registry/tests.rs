//! Ported from strands-ts/src/registry/__tests__/tool-registry.test.ts

use std::sync::Arc;

use super::*;
use crate::tools::context::ToolContext;
use crate::tools::tool::{ToolStream, ToolStreamEvent, ToolStreamItem};
use crate::types::tools::ToolSpec;

struct MockTool {
    name: String,
    description: String,
}

impl Tool for MockTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn tool_spec(&self) -> ToolSpec {
        ToolSpec::new(&self.name, &self.description)
    }

    fn stream(&self, _context: ToolContext) -> ToolStream {
        ToolStream::new(futures::stream::iter([ToolStreamItem::Event(
            ToolStreamEvent::new().with_data("mock data".into()),
        )]))
    }
}

fn tool(name: &str) -> Arc<dyn Tool> {
    tool_with(name, "A valid tool description.")
}

fn tool_with(name: &str, description: &str) -> Arc<dyn Tool> {
    Arc::new(MockTool {
        name: name.to_owned(),
        description: description.to_owned(),
    })
}

fn names(registry: &ToolRegistry) -> Vec<String> {
    registry
        .list()
        .iter()
        .map(|tool| tool.name().to_owned())
        .collect()
}

fn add_error(registry: &mut ToolRegistry, tool: Arc<dyn Tool>) -> String {
    registry.add(tool).unwrap_err().to_string()
}

#[test]
fn registers_a_single_tool() {
    let mut registry = ToolRegistry::new();
    let valid = tool("valid-tool");
    registry.add(Arc::clone(&valid)).unwrap();
    let listed = registry.list();
    assert_eq!(listed.len(), 1);
    assert!(Arc::ptr_eq(&listed[0], &valid));
}

#[test]
fn registers_an_array_of_tools() {
    let mut registry = ToolRegistry::new();
    registry.add_all([tool("tool-1"), tool("tool-2")]).unwrap();
    assert_eq!(names(&registry), ["tool-1", "tool-2"]);
}

#[test]
fn throws_tool_validation_error_for_a_duplicate_tool_name() {
    let mut registry = ToolRegistry::new();
    registry.add(tool("duplicate")).unwrap();
    assert_eq!(
        add_error(&mut registry, tool("duplicate")),
        "Tool with name 'duplicate' already registered"
    );
}

#[test]
fn throws_tool_validation_error_when_a_name_differs_only_by_hyphen_vs_underscore() {
    let mut registry = ToolRegistry::new();
    registry.add(tool("foo-bar")).unwrap();
    assert_eq!(
        add_error(&mut registry, tool("foo_bar")),
        "Tool name 'foo_bar' already exists as 'foo-bar'. Cannot add a duplicate tool which differs by a '-' or '_'"
    );
}

#[test]
fn throws_tool_validation_error_for_an_invalid_tool_name_pattern() {
    assert_eq!(
        add_error(&mut ToolRegistry::new(), tool("invalid name!")),
        "Tool name must contain only alphanumeric characters, hyphens, and underscores"
    );
}

#[test]
fn throws_tool_validation_error_for_a_tool_name_that_is_too_long() {
    assert_eq!(
        add_error(&mut ToolRegistry::new(), tool(&"a".repeat(65))),
        "Tool name must be between 1 and 64 characters"
    );
}

#[test]
fn throws_tool_validation_error_for_a_tool_name_that_is_too_short() {
    assert_eq!(
        add_error(&mut ToolRegistry::new(), tool("")),
        "Tool name must be between 1 and 64 characters"
    );
}

#[test]
fn throws_tool_validation_error_for_an_empty_string_description() {
    assert_eq!(
        add_error(&mut ToolRegistry::new(), tool_with("valid-tool", "")),
        "Tool description must be a non-empty string"
    );
}

#[test]
fn registers_a_tool_with_a_name_at_the_maximum_length() {
    ToolRegistry::new().add(tool(&"a".repeat(64))).unwrap();
}

#[test]
fn registers_tools() {
    let mut registry = ToolRegistry::new();
    let first = tool("tool-1");
    registry.add_or_replace([Arc::clone(&first)]).unwrap();
    assert!(Arc::ptr_eq(registry.get("tool-1").unwrap(), &first));
}

#[test]
fn replaces_an_existing_tool_with_the_same_name() {
    let mut registry = ToolRegistry::new();
    registry.add(tool_with("tool-1", "original")).unwrap();
    registry.add(tool("tool-2")).unwrap();
    let replacement = tool_with("tool-1", "replacement");
    registry.add_or_replace([Arc::clone(&replacement)]).unwrap();
    assert!(Arc::ptr_eq(registry.get("tool-1").unwrap(), &replacement));
    assert_eq!(names(&registry), ["tool-1", "tool-2"]);
}

#[test]
fn validates_tool_properties() {
    let error = ToolRegistry::new()
        .add_or_replace([tool("invalid name!")])
        .unwrap_err();
    assert_eq!(ToolValidationError::NAME, "ToolValidationError");
    assert_eq!(
        error.to_string(),
        "Tool name must contain only alphanumeric characters, hyphens, and underscores"
    );
}

#[test]
fn throws_tool_validation_error_when_a_new_tool_name_differs_only_by_hyphen_vs_underscore() {
    let mut registry = ToolRegistry::new();
    registry.add(tool("foo-bar")).unwrap();
    assert!(registry.add_or_replace([tool("foo_bar")]).is_err());
}

#[test]
fn retrieves_a_tool_by_name() {
    let mut registry = ToolRegistry::new();
    let find_me = tool("find-me");
    registry.add(Arc::clone(&find_me)).unwrap();
    assert!(Arc::ptr_eq(registry.get("find-me").unwrap(), &find_me));
}

#[test]
fn returns_undefined_for_a_non_existent_tool() {
    assert!(ToolRegistry::new().get("non-existent").is_none());
}

#[test]
fn returns_the_tool_for_an_exact_name_match() {
    let mut registry = ToolRegistry::new();
    let my_tool = tool("my-tool");
    registry.add(Arc::clone(&my_tool)).unwrap();
    assert!(Arc::ptr_eq(registry.resolve("my-tool").unwrap(), &my_tool));
}

#[test]
fn resolves_underscore_to_hyphen_substitution() {
    let mut registry = ToolRegistry::new();
    let my_tool = tool("my-tool");
    registry.add(Arc::clone(&my_tool)).unwrap();
    assert!(Arc::ptr_eq(registry.resolve("my_tool").unwrap(), &my_tool));
}

#[test]
fn resolves_case_insensitively() {
    let mut registry = ToolRegistry::new();
    let my_tool = tool("MyTool");
    registry.add(Arc::clone(&my_tool)).unwrap();
    assert!(Arc::ptr_eq(registry.resolve("mytool").unwrap(), &my_tool));
}

#[test]
fn prefers_exact_match_over_case_insensitive_match() {
    let mut registry = ToolRegistry::new();
    let exact = tool("mytool");
    registry.add_all([Arc::clone(&exact), tool("MYTOOL")]).unwrap();
    assert!(Arc::ptr_eq(registry.resolve("mytool").unwrap(), &exact));
}

#[test]
fn prefers_exact_match_over_underscore_to_hyphen_match() {
    let mut registry = ToolRegistry::new();
    let exact = tool("my_tool");
    registry.add(Arc::clone(&exact)).unwrap();
    assert!(Arc::ptr_eq(registry.resolve("my_tool").unwrap(), &exact));
}

#[test]
fn throws_tool_not_found_error_when_no_tool_matches() {
    let mut registry = ToolRegistry::new();
    registry.add(tool("existing-tool")).unwrap();
    assert!(registry.resolve("nonexistent").is_err());
}

#[test]
fn attaches_the_requested_name_to_the_thrown_tool_not_found_error() {
    let error = ToolRegistry::new().resolve("missing").unwrap_err();
    assert_eq!(error.tool_name(), "missing");
    assert_eq!(error.to_string(), "Tool 'missing' not found");
}

#[test]
fn throws_tool_not_found_error_when_registry_is_empty() {
    assert!(ToolRegistry::new().resolve("anything").is_err());
}

#[test]
fn removes_a_tool_by_name() {
    let mut registry = ToolRegistry::new();
    registry.add_all([tool("remove-me"), tool("keep-me")]).unwrap();
    registry.remove("remove-me");
    assert!(registry.get("remove-me").is_none());
    assert_eq!(names(&registry), ["keep-me"]);
}

#[test]
fn does_not_throw_when_removing_a_non_existent_tool() {
    ToolRegistry::new().remove("non-existent");
}

#[test]
fn returns_an_empty_array_when_no_tools_are_registered() {
    assert!(ToolRegistry::new().list().is_empty());
}

#[test]
fn returns_all_registered_tools() {
    let mut registry = ToolRegistry::new();
    registry.add_all([tool("tool-1"), tool("tool-2")]).unwrap();
    assert_eq!(names(&registry), ["tool-1", "tool-2"]);
}

#[test]
fn should_remove_all_registered_tools() {
    let mut registry = ToolRegistry::new();
    registry.add_all([tool("tool-1"), tool("tool-2")]).unwrap();
    registry.clear();
    assert!(registry.list().is_empty());
}

#[test]
fn should_be_a_no_op_on_an_empty_registry() {
    let mut registry = ToolRegistry::new();
    registry.clear();
    assert!(registry.list().is_empty());
}

#[test]
fn accepts_initial_tools() {
    let valid = tool("valid-tool");
    let registry = ToolRegistry::with_tools([Arc::clone(&valid)]).unwrap();
    assert!(Arc::ptr_eq(&registry.list()[0], &valid));
}

#[test]
fn keeps_the_tools_before_an_invalid_one() {
    let mut registry = ToolRegistry::new();
    assert!(
        registry
            .add_all([tool("tool-1"), tool("bad name"), tool("tool-3")])
            .is_err()
    );
    assert_eq!(names(&registry), ["tool-1"]);
}
