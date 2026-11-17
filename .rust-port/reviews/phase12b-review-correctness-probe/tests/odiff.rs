use strands_harness::{define_harness_agent_config, harness_agent_options_from_config};

#[test]
fn odiff() {
    unsafe { std::env::set_var("HOME", "/home/u") };
    let cases: Vec<serde_json::Value> = serde_json::from_str(&std::fs::read_to_string("/tmp/review12b_correctness/ts/cases.json").unwrap()).unwrap();
    for (i, case) in cases.iter().enumerate() {
        let out = define_harness_agent_config(case)
            .and_then(|config| harness_agent_options_from_config(&config, Some(std::path::Path::new("/tmp/base"))));
        match out {
            Err(e) => println!("{i} ERR {e}"),
            Ok(o) => {
                let a = &o.agent_config;
                println!(
                    "{i} caching={:?} instr={:?} desc={:?} name={:?} tools={:?} plugins={:?} interv={:?} mcp={:?} bg={:?} retry={:?} sandbox={:?} exec={} trace={:?} model={:?} effort={:?} cm={:?} session={:?} skills={:?} memory={:?}",
                    o.caching, o.instructions, a.description, a.name, o.builtin_tools, o.builtin_plugins, o.interventions, o.mcp_servers, o.background_tasks,
                    a.retry_strategy.as_ref().map(Vec::len), a.sandbox.is_some(), a.tool_executor.is_some(), a.trace_attributes, o.model, o.effort,
                    o.context_manager.is_some(), o.session, o.skills, o.memory.clone()
                );
            }
        }
    }
}
