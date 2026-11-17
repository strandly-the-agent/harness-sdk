use std::sync::Arc;
use serde_json::json;
use strands::models::Model;
use strands::models::anthropic::{AnthropicModel, AnthropicModelOptions};
use strands::models::openai::{OpenAIModel, OpenAIModelOptions};
use strands_harness::{HarnessAgentOptions, define_harness_agent_config, harness_agent_options_from_config};

#[test]
fn debug_of_options_and_config() {
    let anthropic = AnthropicModel::new(AnthropicModelOptions { api_key: Some("s3cret-anthropic".into()), ..Default::default() });
    let openai = OpenAIModel::new(OpenAIModelOptions { api_key: Some("s3cret-openai".to_owned().into()), ..Default::default() });
    let servers = json!({
        "a": {"command": "run", "args": ["--token", "s3cret-arg"], "env": {"K": "s3cret-env"}, "headers": {"h": "s3cret-header"}, "auth": {"clientId": "id", "clientSecret": "s3cret-auth"}},
        "b": {"url": "https://user:s3cret-url@host/mcp?key=s3cret-query", "headers": ["s3cret-list"], "auth": "s3cret-auth-string"},
        "mcpServers": {"c": {"command": "x", "env": {"K": "s3cret-wrapped"}}}
    });
    let config = define_harness_agent_config(&json!({
        "mcpServers": servers.clone(),
        "agentConfig": {"mcpServers": servers.clone(), "traceAttributes": {"k": "v"}}
    })).unwrap();
    let from_config = harness_agent_options_from_config(&config, Some(std::path::Path::new("/tmp")));
    let mut texts = vec![("config {:?}", format!("{config:?}")), ("config {:#?}", format!("{config:#?}")), ("options_from_config", format!("{from_config:?}"))];
    for (label, model) in [("anthropic", anthropic.map(|m| Arc::new(m) as Arc<dyn Model>).map_err(|e| e.to_string())), ("openai", openai.map(|m| Arc::new(m) as Arc<dyn Model>).map_err(|e| e.to_string()))] {
        match model {
            Ok(model) => {
                let options = HarnessAgentOptions::builder().model(model.clone()).mcp_servers(servers.as_object().unwrap().clone());
                texts.push((label, format!("{options:?}")));
                let options = options.build();
                texts.push((label, format!("{options:?} {options:#?}")));
            }
            Err(error) => println!("{label}: model not built: {error}"),
        }
    }
    for (label, text) in texts {
        let leaks: Vec<&str> = text.match_indices("s3cret").map(|(i, _)| &text[i..(i + 20).min(text.len())]).collect();
        println!("{label}: len={} leaks={leaks:?}", text.len());
    }
}
