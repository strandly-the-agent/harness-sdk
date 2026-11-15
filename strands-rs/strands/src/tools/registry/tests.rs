//! Ported from strands-ts/src/registry/__tests__/tool-registry.test.ts.
//!
//! `throws ToolValidationError for a non-string tool name` and `allows a tool with a null or
//! undefined description` have no Rust form: `Tool::name` and `Tool::description` return `&str`.

use std::sync::Arc;

use super::*;
use crate::error::Error;
use crate::tools::tool::{ToolContext, ToolStream};

struct Named {
    name: String,
    description: String,
}

impl Tool for Named {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn tool_spec(&self) -> ToolSpec {
        ToolSpec::new(self.name.clone(), self.description.clone())
    }

    fn stream(&self, _context: ToolContext) -> ToolStream {
        ToolStream::new(futures::stream::empty())
    }
}

fn tool(name: &str) -> Arc<dyn Tool> {
    tool_with(name, "A test tool")
}

fn tool_with(name: &str, description: &str) -> Arc<dyn Tool> {
    Arc::new(Named {
        name: name.to_owned(),
        description: description.to_owned(),
    })
}

fn names(registry: &ToolRegistry) -> Vec<&str> {
    registry.list().map(|t| t.name()).collect()
}

fn validation_message(error: &Error) -> String {
    assert!(matches!(error, Error::ToolValidation(_)), "{error:?}");
    error.to_string()
}

mod add {
    use super::*;

    #[test]
    fn registers_a_single_tool() {
        let mut registry = ToolRegistry::new();
        registry.add(tool("a")).unwrap();
        assert_eq!(names(&registry), ["a"]);
    }

    #[test]
    fn registers_an_array_of_tools() {
        let mut registry = ToolRegistry::new();
        registry.add_all([tool("a"), tool("b")]).unwrap();
        assert_eq!(names(&registry), ["a", "b"]);
    }

    #[test]
    fn throws_tool_validation_error_for_a_duplicate_tool_name() {
        let mut registry = ToolRegistry::new();
        registry.add(tool("a")).unwrap();
        let error = registry.add(tool("a")).unwrap_err();
        assert_eq!(
            validation_message(&error),
            "Tool with name 'a' already registered"
        );
    }

    #[test]
    fn throws_tool_validation_error_when_a_name_differs_only_by_dash_vs_underscore() {
        let mut registry = ToolRegistry::new();
        registry.add(tool("my-tool")).unwrap();
        let error = registry.add(tool("my_tool")).unwrap_err();
        assert_eq!(
            validation_message(&error),
            "Tool name 'my_tool' already exists as 'my-tool'. Cannot add a duplicate tool which differs by a '-' or '_'"
        );
    }

    #[test]
    fn throws_tool_validation_error_for_an_invalid_tool_name_pattern() {
        let error = ToolRegistry::new().add(tool("bad name!")).unwrap_err();
        assert_eq!(
            validation_message(&error),
            "Tool name must contain only alphanumeric characters, hyphens, and underscores"
        );
    }

    #[test]
    fn throws_tool_validation_error_for_a_tool_name_that_is_too_long() {
        let error = ToolRegistry::new().add(tool(&"a".repeat(65))).unwrap_err();
        assert_eq!(
            validation_message(&error),
            "Tool name must be between 1 and 64 characters"
        );
    }

    #[test]
    fn throws_tool_validation_error_for_a_tool_name_that_is_too_short() {
        let error = ToolRegistry::new().add(tool("")).unwrap_err();
        assert_eq!(
            validation_message(&error),
            "Tool name must be between 1 and 64 characters"
        );
    }

    #[test]
    fn throws_tool_validation_error_for_an_empty_string_description() {
        let error = ToolRegistry::new().add(tool_with("a", "")).unwrap_err();
        assert_eq!(
            validation_message(&error),
            "Tool description must be a non-empty string"
        );
    }

    #[test]
    fn registers_a_tool_with_a_name_at_the_maximum_length() {
        let mut registry = ToolRegistry::new();
        registry.add(tool(&"a".repeat(64))).unwrap();
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn a_failing_batch_keeps_the_tools_added_before_the_failure() {
        let mut registry = ToolRegistry::new();
        assert!(registry.add_all([tool("a"), tool("a"), tool("b")]).is_err());
        assert_eq!(names(&registry), ["a"]);
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
        registry.add_all([tool("a"), tool("b")]).unwrap();
        registry.add_or_replace([tool_with("a", "replacement")]).unwrap();
        assert_eq!(names(&registry), ["a", "b"]);
        assert_eq!(registry.get("a").unwrap().description(), "replacement");
    }

    #[test]
    fn validates_tool_properties() {
        let error = ToolRegistry::new()
            .add_or_replace([tool("bad name")])
            .unwrap_err();
        assert!(matches!(error, Error::ToolValidation(_)));
    }

    #[test]
    fn throws_tool_validation_error_when_a_new_tool_name_differs_only_by_dash_vs_underscore() {
        let mut registry = ToolRegistry::new();
        registry.add(tool("my_tool")).unwrap();
        let error = registry.add_or_replace([tool("my-tool")]).unwrap_err();
        assert!(matches!(error, Error::ToolValidation(_)));
    }
}

mod get_and_resolve {
    use super::*;

    #[test]
    fn retrieves_a_tool_by_name() {
        let mut registry = ToolRegistry::new();
        registry.add(tool("a")).unwrap();
        assert_eq!(registry.get("a").unwrap().name(), "a");
    }

    #[test]
    fn returns_undefined_for_a_non_existent_tool() {
        assert!(ToolRegistry::new().get("missing").is_none());
    }

    #[test]
    fn returns_the_tool_for_an_exact_name_match() {
        let mut registry = ToolRegistry::new();
        registry.add(tool("search")).unwrap();
        assert_eq!(registry.resolve("search").unwrap().name(), "search");
    }

    #[test]
    fn resolves_underscore_to_hyphen_substitution() {
        let mut registry = ToolRegistry::new();
        registry.add(tool("get-weather")).unwrap();
        assert_eq!(registry.resolve("get_weather").unwrap().name(), "get-weather");
    }

    #[test]
    fn resolves_case_insensitively() {
        let mut registry = ToolRegistry::new();
        registry.add(tool("GetWeather")).unwrap();
        assert_eq!(registry.resolve("getweather").unwrap().name(), "GetWeather");
    }

    #[test]
    fn prefers_exact_match_over_case_insensitive_match() {
        let mut registry = ToolRegistry::new();
        registry.add_all([tool("Search"), tool("search")]).unwrap();
        assert_eq!(registry.resolve("search").unwrap().name(), "search");
    }

    #[test]
    fn prefers_exact_match_over_underscore_to_hyphen_match() {
        let mut registry = ToolRegistry::new();
        registry.add(tool("get_weather")).unwrap();
        assert_eq!(registry.resolve("get_weather").unwrap().name(), "get_weather");
    }

    #[test]
    fn throws_tool_not_found_error_when_no_tool_matches() {
        let mut registry = ToolRegistry::new();
        registry.add(tool("a")).unwrap();
        let error = registry.resolve("missing").unwrap_err();
        assert_eq!(error.to_string(), "Tool 'missing' not found");
    }

    #[test]
    fn attaches_the_requested_name_to_the_thrown_tool_not_found_error() {
        let Error::ToolNotFound(error) = ToolRegistry::new().resolve("Missing_Tool").unwrap_err() else {
            panic!("expected ToolNotFound");
        };
        assert_eq!(error.tool_name(), "Missing_Tool");
    }

    #[test]
    fn throws_tool_not_found_error_when_registry_is_empty() {
        assert!(matches!(
            ToolRegistry::new().resolve("a"),
            Err(Error::ToolNotFound(_))
        ));
    }
}

mod remove_list_clear {
    use super::*;

    #[test]
    fn removes_a_tool_by_name() {
        let mut registry = ToolRegistry::new();
        registry.add_all([tool("a"), tool("b"), tool("c")]).unwrap();
        registry.remove("b");
        assert_eq!(names(&registry), ["a", "c"]);
    }

    #[test]
    fn does_not_throw_when_removing_a_non_existent_tool() {
        ToolRegistry::new().remove("missing");
    }

    #[test]
    fn returns_an_empty_array_when_no_tools_are_registered() {
        assert!(ToolRegistry::new().list().next().is_none());
    }

    #[test]
    fn returns_all_registered_tools() {
        let mut registry = ToolRegistry::new();
        registry.add_all([tool("b"), tool("a")]).unwrap();
        assert_eq!(names(&registry), ["b", "a"]);
        let specs: Vec<String> = registry
            .tool_specs()
            .iter()
            .map(|s| s.name().to_owned())
            .collect();
        assert_eq!(specs, ["b", "a"]);
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
        registry.add_all(vec![tool("a")]).unwrap();
        assert_eq!(registry.len(), 1);
    }
}
