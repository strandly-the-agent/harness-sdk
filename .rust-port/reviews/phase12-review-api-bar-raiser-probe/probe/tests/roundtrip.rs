use serde_json::json;
use strands_harness::*;

#[test]
fn default_round_trips_through_serde() {
    let value = serde_json::to_value(&*DEFAULT_HARNESS_AGENT_CONFIG).unwrap();
    println!("{value}");
    let back: HarnessAgentConfig = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(back, *DEFAULT_HARNESS_AGENT_CONFIG);
    assert_eq!(define_harness_agent_config(&value).unwrap(), *DEFAULT_HARNESS_AGENT_CONFIG);
    assert_eq!(define_harness_agent_config(&json!({})).unwrap(), HarnessAgentConfig::default());
}

#[test]
fn typed_config_round_trips() {
    let config = HarnessAgentConfig::builder()
        .name("x")
        .effort(Effort::High)
        .session(Setting::Config(SessionConfig { id: Some("abc".into()), dir: None }))
        .memory(false)
        .context_manager(HarnessConfigContextManager::Off)
        .builtin_tools(HarnessConfigBuiltinToolsSetting::Mapping(Box::new(HarnessConfigBuiltinTools {
            all: Some(false),
            programmatic_tool_caller: Some(Setting::Config(ProgrammaticToolCallerConfig {
                allowed_tools: Some(None),
                timeout: Some(None),
            })),
            web_search: Some(WebSearchSetting::Exa),
            ..Default::default()
        })))
        .build();
    let value = serde_json::to_value(&config).unwrap();
    println!("{value}");
    let back: HarnessAgentConfig = serde_json::from_value(value).unwrap();
    assert_eq!(back, config);
}

#[test]
fn options_from_typed_config_literal() {
    // A typed config with a value the JSON normalizer rejects.
    let config = HarnessAgentConfig {
        builtin_tools: HarnessConfigBuiltinToolsSetting::Mapping(Box::new(HarnessConfigBuiltinTools {
            programmatic_tool_caller: Some(Setting::Config(ProgrammaticToolCallerConfig {
                allowed_tools: None,
                timeout: Some(Some(f64::NAN)),
            })),
            ..Default::default()
        })),
        ..Default::default()
    };
    let result = harness_agent_options_from_config(&config, None);
    println!("{:?}", result.as_ref().err().map(ToString::to_string));
}

#[test]
fn nested_types_deserialize_alone() {
    let tools: Result<HarnessConfigBuiltinTools, _> = serde_json::from_value(json!({"*": "yes", "shell": {"bogus": 1}}));
    println!("{tools:?}");
    let caller: Result<ProgrammaticToolCallerConfig, _> = serde_json::from_value(json!({"timeout": -5}));
    println!("{caller:?}");
    let effort: Result<Effort, _> = serde_json::from_value(json!("HIGH"));
    println!("{effort:?}");
}
