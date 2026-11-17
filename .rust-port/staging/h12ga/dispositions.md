# h12ga dispositions (Phase 12 API review: options, builders, typed values)

| finding | disposition | commit |
| --- | --- | --- |
| api-1 | Fixed. `McpServersOption` has `From<&str>`, `From<String>`, `From<&String>` (a path) and `From<JsonObject>` (the server map); `SkillsOption` has `From<SkillSource>`, `From<&str>` and `From<String>` (TS `SkillSource` alone); `BuiltinToolsOption` also takes `[BuiltinToolName; N]` and `&[BuiltinToolName]`. The other TS union members of `HarnessAgentOptions` were checked and already convert (`model`, `background_tasks`, `caching`, `context_manager`, `session`, `memory`, `interventions`). The section 13 snippet is a `no_run` doctest on `create_harness`. Tests: `agent::tests::options::the_documented_builder_snippet_compiles_with_a_path_for_mcp_servers`, `mcp_servers_takes_a_path_or_a_server_map`, `skills_takes_one_source_as_ts_does`, `builtin_tools_takes_an_array_or_a_slice_of_names`. | 3c578eef2 |
