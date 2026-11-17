use serde_json::json;
use strands_harness::define_harness_agent_config;

#[test]
fn max_depth_forms() {
    for text in ["-0", "-0.0", "0", "2.0", "1e3", "4294967296", "9007199254740991", "9007199254740992", "1.5", "-1"] {
        let depth: serde_json::Value = serde_json::from_str(text).unwrap();
        let out = define_harness_agent_config(&json!({"builtinTools": {"subagent": {"maxDepth": depth}}}));
        println!("maxDepth={text} -> {:?}", out.map(|c| serde_json::to_value(&c).unwrap()["builtinTools"]["subagent"].clone()).map_err(|e| e.to_string()));
    }
}
