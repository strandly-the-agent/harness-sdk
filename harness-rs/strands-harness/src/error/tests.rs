//! The typed fields of `HarnessError` variants; TS throws plain `Error`s, so there is no spec file.

use std::error::Error as _;
use std::path::Path;

use serde_json::json;

use super::*;
use crate::config::{define_harness_agent_config, harness_agent_options_from_config};
use crate::models::{ResolveModelOptions, plan};

fn plan_error(spec: &str, effort: Effort) -> HarnessError {
    let options = ResolveModelOptions {
        effort,
        ..Default::default()
    };
    match plan(spec, options) {
        Ok(_) => panic!("{spec} resolved"),
        Err(error) => error,
    }
}

#[test]
fn an_unsupported_effort_carries_the_effort_and_prints_its_ts_name() {
    let error = plan_error("openai/gpt-5.6-sol", Effort::Max);

    assert!(
        matches!(
            error,
            HarnessError::UnsupportedEffort {
                effort: Effort::Max,
                ..
            }
        ),
        "{error:?}"
    );
    assert!(
        error
            .to_string()
            .starts_with("Effort \"max\" is not supported by "),
        "{error}"
    );
}

#[test]
fn an_unknown_model_provider_lists_the_supported_providers_one_per_item() {
    let error = plan_error("mistral/whatever", Effort::Auto);

    let HarnessError::UnknownModelProvider { supported, .. } = &error else {
        panic!("{error:?}");
    };
    assert_eq!(*supported, ["anthropic", "bedrock", "bedrock-mantle", "openai"]);
    assert!(
        error
            .to_string()
            .contains("Supported providers: anthropic, bedrock, bedrock-mantle, openai. "),
        "{error}"
    );
}

#[test]
fn an_unknown_agent_type_lists_the_presets_one_per_item() {
    let error = HarnessError::UnknownAgentType {
        value: "\"nope\"".to_owned(),
        valid: vec!["generalist".to_owned(), "researcher".to_owned()],
    };

    assert_eq!(
        error.to_string(),
        "Unknown agent_type \"nope\"; valid values: generalist, researcher."
    );
}

#[test]
fn an_agent_config_value_that_does_not_deserialize_keeps_the_serde_error_as_its_source() {
    let config = define_harness_agent_config(&json!({
        "agentConfig": { "backgroundTasks": { "sometimes": [] } }
    }))
    .expect("valid config");

    let error = harness_agent_options_from_config(&config, Some(Path::new("/project")))
        .expect_err("invalid agentConfig");

    let source = error.source().expect("a source");
    assert!(source.is::<serde_json::Error>(), "{source:?}");
    assert_eq!(
        error.to_string(),
        format!("agentConfig.backgroundTasks is invalid: {source}")
    );
}

#[test]
fn an_agent_config_value_rejected_by_the_harness_has_no_source() {
    let config =
        define_harness_agent_config(&json!({ "agentConfig": { "sandbox": true } })).expect("valid config");

    let error = harness_agent_options_from_config(&config, Some(Path::new("/project")))
        .expect_err("invalid agentConfig");

    assert!(error.source().is_none(), "{error:?}");
    assert_eq!(
        error.to_string(),
        "agentConfig.sandbox is invalid: only false has a JSON form"
    );
}
