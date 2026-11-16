Track `h12b`: close the `create_harness` cases that other specs left open, `internal.test.ts`, and the portable config end to end. Read `.rust-port/briefs/run5-common.md` first and follow it.

Worktree `/mnt/workspace/wt-h12b` (branch `wip/h12b`), `CARGO_TARGET_DIR=/mnt/workspace/.cache/ct-h12b`, gate output `/mnt/workspace/gates-h12b`, staging dir `.rust-port/staging/h12b/`, decisions P-1070 to P-1089.

Read `.rust-port/handoff/h12core-handoff.md` (including the notes at its end) for how `create_harness`, `create_harness_with(options, Seams)` and `create_harness_from(.., build_agent)` are tested, and the helpers at the top of `harness-rs/strands-harness/src/agent/tests.rs`.

Another track (`h12a`) owns `agent/tests.rs` and `builtin_tools/tests.rs` and appends to them. You add exactly one block of `mod` lines to `agent/tests.rs`, directly after its `use` block, in your first commit, and put your tests in new child modules `src/agent/tests/{memory,plugins,config_e2e,internal}.rs` (child modules can use the private helpers of `agent/tests.rs` through `super::`). Don't edit anything else in `agent/tests.rs`.

Cases (TS titles are in `harness-rs/docs/test-title-map.json` as declared gaps whose reason says not ported yet, or missing in the title checker; remove a map entry when its test exists):
1. `harness-ts/test/memory.test.ts`: the 6 open `createHarness memory` cases (cheapest: `honors a custom memory dir`, the two `MemoryManager` instance cases).
2. `harness-ts/test/openai-cache.test.ts` (2 cases).
3. `harness-ts/test/plugins/todos.test.ts` (3 open `createHarness` cases) and `harness-ts/test/plugins/environment.test.ts` (1 open case).
4. `harness-ts/test/internal.test.ts` (2 cases): TS `harness-ts/src/internal.ts` re-exports CLI plumbing. Decide how Rust exposes it (the handoff suggests a `pub mod internal` re-exporting `normalize_harness_agent_config`, `resolve_model`, `resolve_memory`, `resolve_interventions`, `resolve_builtin_tools`, `enabled_builtin_tools`, `builtin_tool_config`; weigh that against PROMPT.md section 7.1 "`@internal` becomes `pub(crate)` or `#[doc(hidden)]`" and section 7.4), log the decision, and port or declare the cases accordingly.
5. `harness_agent_options_from_config` end to end into `create_harness` (tracker item "create_harness ..."): tests that a portable `HarnessAgentConfig` JSON (name, instructions, model string, effort, caching, builtinTools list and mapping, session, skills, memory, interventions, mcpServers inline, `${VAR}` and relative-path expansion against `base_dir`) goes through `normalize` and `options_from_config` into `create_harness_with` and yields an agent with the expected tools, prompt, session and so on, and that a module reference anywhere fails with the section 10 text. Fix any wiring defect you find in its own `fix(harness)` commit with a test.
6. The declared gaps of `config.test.ts` (3) and `interventions.test.ts` (2): check each reason is true and specific; port the case if it can be expressed.

If an SDK accessor is unavoidable (for example to read an agent's memory manager or plugins), make the smallest `feat(strands)` commit of its own with a pending decision; `h12a` adds an intervention-handler accessor to `strands-rs/strands/src/agent/agent.rs` at the same time, so keep your SDK edit away from that code.

Finish with the title checker printing ok for your specs, the staged PARITY rows (`memory`, `openai-cache`, `todos`, `environment`, `internal`, `config`, `interventions`), and a short `handoff.md` in your staging dir listing anything left, case by case.
