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

/// The private modules make the `pub use` lines of `lib.rs` the whole root surface.
#[test]
fn keeps_that_plumbing_off_the_root_surface() {
    let root = include_str!("../../lib.rs");
    assert!(
        !root.contains("pub mod "),
        "a public module widens the root surface"
    );
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
