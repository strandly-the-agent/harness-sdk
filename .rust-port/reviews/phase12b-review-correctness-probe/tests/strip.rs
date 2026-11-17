use std::collections::HashMap;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use strands::tools::function_tool::InputValidator;
use strands::tools::{FunctionTool, Tool, ToolContext, ToolError};

fn schema_of<I: serde::de::DeserializeOwned + JsonSchema + Send + 'static>() -> serde_json::Value {
    let tool = FunctionTool::typed("t", "d", |_i: I, _c: ToolContext| async move { Ok::<_, ToolError>("ok".to_owned()) });
    tool.tool_spec().input_schema().cloned().unwrap()
}

fn check<I: serde::de::DeserializeOwned + JsonSchema + Send + std::fmt::Debug + 'static>(label: &str, input: serde_json::Value) {
    let schema = schema_of::<I>();
    let out = InputValidator::new(&schema).parse::<I>(input);
    println!("--- {label}\nschema={schema}\nresult={:?}", out.map_err(|e| e.to_string()));
}

#[derive(Debug, Deserialize, JsonSchema)]
struct Inner { a: String }
#[derive(Debug, Deserialize, JsonSchema)]
struct Outer { inner: Inner, items: Vec<Inner>, opt: Option<Inner> }
#[derive(Debug, Deserialize, JsonSchema)]
struct Empty {}
#[derive(Debug, Deserialize, JsonSchema)]
struct Flat { #[serde(flatten)] inner: Inner, b: Option<u32> }
#[derive(Debug, Deserialize, JsonSchema)]
struct Alias { #[serde(alias = "p")] path: Option<String> }
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct StrictInner { a: String }
#[derive(Debug, Deserialize, JsonSchema)]
struct LooseOuter { inner: StrictInner }
#[derive(Debug, Deserialize, JsonSchema)]
struct Map { m: HashMap<String, u32>, x: u32 }
#[derive(Debug, Deserialize, JsonSchema)]
struct Tuple { t: (Inner, u32) }
#[derive(Debug, Deserialize, JsonSchema)]
struct Defaulted { #[serde(default)] n: u32 }

#[test]
fn probes() {
    check::<Outer>("nested/array/option", json!({"inner": {"a":"x","z":1}, "items":[{"a":"y","z":2}], "opt": {"a":"q","z":3}, "top": 1}));
    check::<Empty>("empty struct", json!({"z":1}));
    check::<Flat>("flatten", json!({"a":"x","b":1,"z":1}));
    check::<Alias>("alias", json!({"p":"/tmp"}));
    check::<LooseOuter>("strict nested", json!({"inner": {"a":"x","z":1}}));
    check::<LooseOuter>("strict nested, extra on top only", json!({"inner": {"a":"x"}, "z":1}));
    check::<Map>("map", json!({"m": {"k": 1}, "x": 1, "z": 2}));
    check::<Tuple>("tuple", json!({"t": [{"a":"x","z":1}, 2]}));
    check::<Defaulted>("non-object input", json!([1]));
    check::<Defaulted>("null input", json!(null));
}
