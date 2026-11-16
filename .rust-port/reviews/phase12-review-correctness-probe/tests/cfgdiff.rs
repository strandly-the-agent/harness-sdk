use strands_harness::*;

fn sort(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(items) => serde_json::Value::Array(items.iter().map(sort).collect()),
        serde_json::Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            let mut out = serde_json::Map::new();
            for (key, value) in entries {
                out.insert(key.clone(), sort(value));
            }
            serde_json::Value::Object(out)
        }
        other => other.clone(),
    }
}

#[test]
fn config_differential() {
    let text = std::fs::read_to_string("/tmp/review12_correctness/ts/cases.json").unwrap();
    let cases: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
    let mut out = String::new();
    for (index, case) in cases.iter().enumerate() {
        let line = match define_harness_agent_config(case) {
            Ok(config) => serde_json::json!({ "ok": sort(&serde_json::to_value(&config).unwrap()) }),
            Err(error) => serde_json::json!({ "err": error.to_string() }),
        };
        out.push_str(&format!("{index}\t{}\n", serde_json::to_string(&line).unwrap()));
    }
    std::fs::write("/tmp/review12_correctness/ts/rs_out.txt", out).unwrap();
}
