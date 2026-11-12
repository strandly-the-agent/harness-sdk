//! Ported from strands-ts/src/registry/__tests__/tool-registry.test.ts
//!
//! A non-string name or description can't be expressed in Rust (`Tool::name` returns `&str`),
//! and a description is always present, so `throws ToolValidationError for a non-string tool
//! name`, `throws ToolValidationError for an invalid description` and `allows a tool with a null
//! or undefined description` have no equivalent.

use std::sync::Arc;

use super::*;
use crate::error::Error;
use crate::testing::{mock_tool, text_result};
use crate::tools::{Tool, ToolSpec, ToolStream};

fn tool(name: &str) -> Arc<dyn Tool> {
    Arc::new(mock_tool(name, |ctx| text_result(ctx, "ok")))
}

struct Described(&'static str);

impl Tool for Described {
    fn name(&self) -> &str {
        "described"
    }
    fn description(&self) -> &str {
        self.0
    }
    fn tool_spec(&self) -> ToolSpec {
        ToolSpec::new("described", self.0)
    }
    fn stream(&self, context: crate::ToolContext) -> ToolStream {
        ToolStream::from_future(async move { text_result(&context, "ok") })
    }
}

fn validation_message(result: Result<()>) -> String {
    match result.unwrap_err() {
        Error::ToolValidation(error) => error.message().to_owned(),
        other => panic!("unexpected error {other}"),
    }
}

fn names(registry: &ToolRegistry) -> Vec<&str> {
    registry.names().collect()
}

mod add {
    use super::*;

    #[test]
    fn registers_a_single_tool() {
        let mut registry = ToolRegistry::new();
        registry.add(tool("test-tool")).unwrap();
        assert!(registry.get("test-tool").is_some());
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
        registry.add(tool("dup")).unwrap();
        assert_eq!(
            validation_message(registry.add(tool("dup"))),
            "Tool with name 'dup' already registered"
        );
    }

    #[test]
    fn throws_tool_validation_error_when_a_name_differs_only_by_hyphen_vs_underscore() {
        let mut registry = ToolRegistry::new();
        registry.add(tool("my-tool")).unwrap();
        assert_eq!(
            validation_message(registry.add(tool("my_tool"))),
            "Tool name 'my_tool' already exists as 'my-tool'. Cannot add a duplicate tool which differs by a '-' or '_'"
        );
    }

    #[test]
    fn throws_tool_validation_error_for_an_invalid_tool_name_pattern() {
        let mut registry = ToolRegistry::new();
        assert_eq!(
            validation_message(registry.add(tool("invalid name!"))),
            "Tool name must contain only alphanumeric characters, hyphens, and underscores"
        );
    }

    #[test]
    fn throws_tool_validation_error_for_a_tool_name_that_is_too_long() {
        let mut registry = ToolRegistry::new();
        assert_eq!(
            validation_message(registry.add(tool(&"a".repeat(65)))),
            "Tool name must be between 1 and 64 characters"
        );
    }

    #[test]
    fn throws_tool_validation_error_for_a_tool_name_that_is_too_short() {
        let mut registry = ToolRegistry::new();
        assert_eq!(
            validation_message(registry.add(tool(""))),
            "Tool name must be between 1 and 64 characters"
        );
    }

    #[test]
    fn throws_tool_validation_error_for_an_empty_string_description() {
        let mut registry = ToolRegistry::new();
        assert_eq!(
            validation_message(registry.add(Arc::new(Described("")))),
            "Tool description must be a non-empty string"
        );
    }

    #[test]
    fn registers_a_tool_with_a_name_at_the_maximum_length() {
        let mut registry = ToolRegistry::new();
        registry.add(tool(&"a".repeat(64))).unwrap();
        assert_eq!(registry.len(), 1);
    }
}

mod add_or_replace {
    use super::*;

    #[test]
    fn registers_tools() {
        let mut registry = ToolRegistry::new();
        registry.add_or_replace([tool("a"), tool("b")]).unwrap();
        assert_eq!(names(&registry), ["a", "b"]);
    }

    #[test]
    fn replaces_an_existing_tool_with_the_same_name() {
        let mut registry = ToolRegistry::new();
        let first = tool("a");
        let second = tool("a");
        registry.add(Arc::clone(&first)).unwrap();
        registry.add(tool("b")).unwrap();
        registry.add_or_replace([Arc::clone(&second)]).unwrap();
        assert!(Arc::ptr_eq(registry.get("a").unwrap(), &second));
        assert_eq!(names(&registry), ["a", "b"]);
    }

    #[test]
    fn validates_tool_properties() {
        let mut registry = ToolRegistry::new();
        assert!(registry.add_or_replace([tool("bad name")]).is_err());
    }

    #[test]
    fn throws_tool_validation_error_when_a_new_tool_name_differs_only_by_hyphen_vs_underscore() {
        let mut registry = ToolRegistry::new();
        registry.add(tool("my-tool")).unwrap();
        assert!(
            validation_message(registry.add_or_replace([tool("my_tool")]))
                .contains("already exists as 'my-tool'")
        );
    }
}

mod get {
    use super::*;

    #[test]
    fn retrieves_a_tool_by_name() {
        let mut registry = ToolRegistry::new();
        registry.add(tool("x")).unwrap();
        assert_eq!(registry.get("x").map(|tool| tool.name()), Some("x"));
    }

    #[test]
    fn returns_undefined_for_a_non_existent_tool() {
        assert!(ToolRegistry::new().get("missing").is_none());
    }
}

mod resolve {
    use super::*;

    fn registry(names: &[&str]) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.add_all(names.iter().map(|name| tool(name))).unwrap();
        registry
    }

    #[test]
    fn returns_the_tool_for_an_exact_name_match() {
        assert_eq!(
            registry(&["my-tool"]).resolve("my-tool").unwrap().name(),
            "my-tool"
        );
    }

    #[test]
    fn resolves_underscore_to_hyphen_substitution() {
        assert_eq!(
            registry(&["my-tool"]).resolve("my_tool").unwrap().name(),
            "my-tool"
        );
    }

    #[test]
    fn resolves_case_insensitively() {
        assert_eq!(registry(&["MyTool"]).resolve("mytool").unwrap().name(), "MyTool");
    }

    #[test]
    fn prefers_exact_match_over_case_insensitive_match() {
        assert_eq!(
            registry(&["mytool", "MyTool"]).resolve("MyTool").unwrap().name(),
            "MyTool"
        );
    }

    #[test]
    fn prefers_exact_match_over_underscore_to_hyphen_match() {
        let mut registry = ToolRegistry::new();
        registry.add(tool("a_b")).unwrap();
        assert_eq!(registry.resolve("a_b").unwrap().name(), "a_b");
    }

    #[test]
    fn throws_tool_not_found_error_when_no_tool_matches() {
        assert!(matches!(
            registry(&["existing-tool"]).resolve("nonexistent"),
            Err(Error::ToolNotFound(_))
        ));
    }

    #[test]
    fn attaches_the_requested_name_to_the_thrown_tool_not_found_error() {
        let Err(Error::ToolNotFound(error)) = ToolRegistry::new().resolve("missing") else {
            panic!("expected ToolNotFoundError");
        };
        assert_eq!(error.tool_name(), "missing");
        assert_eq!(error.to_string(), "Tool 'missing' not found");
    }

    #[test]
    fn throws_tool_not_found_error_when_registry_is_empty() {
        assert!(ToolRegistry::new().resolve("anything").is_err());
    }
}

mod remove_list_clear {
    use super::*;

    #[test]
    fn removes_a_tool_by_name() {
        let mut registry = ToolRegistry::new();
        registry.add(tool("x")).unwrap();
        registry.remove("x");
        assert!(registry.get("x").is_none());
    }

    #[test]
    fn does_not_throw_when_removing_a_non_existent_tool() {
        assert!(ToolRegistry::new().remove("missing").is_none());
    }

    #[test]
    fn returns_an_empty_array_when_no_tools_are_registered() {
        assert!(ToolRegistry::new().list().is_empty());
    }

    #[test]
    fn returns_all_registered_tools() {
        let mut registry = ToolRegistry::new();
        registry.add_all([tool("a"), tool("b")]).unwrap();
        let listed: Vec<String> = registry
            .list()
            .iter()
            .map(|tool| tool.name().to_owned())
            .collect();
        assert_eq!(listed, ["a", "b"]);
    }

    #[test]
    fn should_remove_all_registered_tools() {
        let mut registry = ToolRegistry::new();
        registry.add_all([tool("a"), tool("b")]).unwrap();
        registry.clear();
        assert!(registry.is_empty());
    }

    #[test]
    fn should_be_a_no_op_on_an_empty_registry() {
        let mut registry = ToolRegistry::new();
        registry.clear();
        assert!(registry.is_empty());
    }

    #[test]
    fn accepts_initial_tools() {
        let mut registry = ToolRegistry::new();
        registry.add_all([tool("a")]).unwrap();
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn removing_keeps_the_order_of_the_rest() {
        let mut registry = ToolRegistry::new();
        registry.add_all([tool("a"), tool("b"), tool("c")]).unwrap();
        registry.remove("b");
        assert_eq!(names(&registry), ["a", "c"]);
    }
}
