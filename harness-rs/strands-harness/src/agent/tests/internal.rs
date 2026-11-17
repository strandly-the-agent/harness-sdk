//! Ports `harness-ts/test/internal.test.ts`. TS ships the factory plumbing on a second entry
//! point, `@strands-agents/harness/internal`, for the TypeScript CLI. Rust has no such entry
//! point: the plumbing is `pub(crate)`, and the crate root is the whole public surface.

use serde_json::json;

use super::*;
use crate::builtin_tools::{builtin_tool_config, enabled_builtin_tools, resolve_builtin_tools};
use crate::config::normalize_harness_agent_config;

/// Each function TS names exists under its re-cased name and runs.
#[test]
fn exposes_the_plumbing_the_cli_consumes() {
    let env: EnvLookup = Arc::new(|_| None);
    let model: Arc<dyn Model> = Arc::new(MockMessageModel::new());

    let config = normalize_harness_agent_config(&json!({ "name": "plumbing" })).unwrap();
    assert_eq!(config.name, "plumbing");

    let resolved = resolve_model(
        Some(&ModelSource::Model(model.clone())),
        DEFAULT_MODEL,
        ResolveModelOptions::default(),
        &env,
    )
    .unwrap();
    assert!(matches!(resolved, ModelSource::Model(_)));

    let dir = tempfile::tempdir().unwrap();
    let options = ResolveMemoryOptions::builder()
        .model(ModelSource::Model(model))
        .dir(dir.path().to_string_lossy().into_owned())
        .build();
    resolve_memory(options, &env).unwrap();

    assert!(resolve_interventions(None, None).unwrap().is_empty());

    let builtin = resolve_builtin_tools(None);
    assert!(enabled_builtin_tools(&builtin).contains(&BuiltinToolName::Shell));
    assert!(builtin_tool_config(&builtin.shell).is_some());
}

/// The private modules make the `pub use` lines of `lib.rs` the whole root surface; the one
/// public module, `builders`, holds the builders and their state modules only.
#[test]
fn keeps_that_plumbing_off_the_root_surface() {
    let root = include_str!("../../lib.rs");
    let public_modules: Vec<&str> = root.lines().filter(|line| line.starts_with("pub mod ")).collect();
    assert_eq!(
        public_modules,
        ["pub mod builders;"],
        "a public module other than `builders` widens the root surface"
    );
    let builders = include_str!("../../builders.rs");
    let reexports = builders
        .split("pub use ")
        .skip(1)
        .filter_map(|item| item.split_once('{')?.1.split_once('}'))
        .flat_map(|(names, _)| names.split(','))
        .map(str::trim)
        .filter(|name| !name.is_empty());
    for name in reexports {
        assert!(
            name.ends_with("Builder") || name.ends_with("_builder"),
            "{name} is not a builder or a builder state module"
        );
    }
    let exported: Vec<&str> = root.split(|c: char| !(c.is_alphanumeric() || c == '_')).collect();
    for name in [
        "resolve_model",
        "resolve_memory",
        "resolve_interventions",
        "resolve_builtin_tools",
        "EFFORT_LEVELS",
        "DEFAULT_MEMORY_DIR",
        "DEFAULT_SKILLS_DIR",
    ] {
        assert!(!exported.contains(&name), "{name} is on the root surface");
    }
}
