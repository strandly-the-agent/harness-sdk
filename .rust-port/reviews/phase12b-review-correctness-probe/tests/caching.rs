use serde_json::json;
use strands_harness::{define_harness_agent_config, harness_agent_options_from_config};

#[test]
fn agent_config_caching_auto() {
    for value in [json!("auto"), json!(true), json!(false), json!(null)] {
        let config = define_harness_agent_config(&json!({
            "name": "a",
            "model": "bedrock/anthropic.claude-3-haiku-20240307-v1:0",
            "agentConfig": { "caching": value }
        }))
        .unwrap();
        let options = harness_agent_options_from_config(&config, Some(std::path::Path::new("/tmp"))).unwrap();
        println!("agentConfig.caching={value} -> options.caching={:?}", options.caching);
    }
}
