use strands_harness::*;

#[test]
fn options_differential() {
    unsafe { std::env::set_var("HOME", "/home/u"); std::env::remove_var("UNSET_VAR_XYZ"); }
    let text = std::fs::read_to_string("/tmp/review12_correctness/ts/ocases.json").unwrap();
    let cases: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
    let mut out = String::new();
    for (index, case) in cases.iter().enumerate() {
        let options = define_harness_agent_config(case)
            .and_then(|config| harness_agent_options_from_config(&config, Some(std::path::Path::new("/tmp/review12_correctness/ts/base"))));
        let line = match options {
            Ok(o) => {
                let mcp = match &o.mcp_servers {
                    Some(McpServersOption::Servers(servers)) => serde_json::Value::Object(servers.clone()).to_string(),
                    Some(other) => format!("{other:?}"),
                    None => "-".to_owned(),
                };
                format!(
                    "ok name={:?} desc={:?} instr={:?} model={:?} effort={:?} caching={:?} cm={:?} session={:?} skills={:?} memory={:?} plugins={:?} interventions={:?} builtin={:?} bg={:?} sp={:?} printer={:?} id={:?} app={:?} mcp={}",
                    o.agent_config.name, o.agent_config.description, o.instructions, o.model, o.effort, o.caching, o.context_manager,
                    o.session, o.skills, o.memory, o.builtin_plugins, o.interventions, o.builtin_tools, o.background_tasks,
                    o.agent_config.system_prompt, o.agent_config.printer, o.agent_config.id, o.agent_config.app_state, mcp
                )
            }
            Err(error) => format!("err {error}"),
        };
        out.push_str(&format!("{index}\t{line}\n"));
    }
    std::fs::write("/tmp/review12_correctness/ts/ors_out.txt", out).unwrap();
}
