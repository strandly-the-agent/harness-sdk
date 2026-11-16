# h12a handoff

## State
- `agent.test.ts` 95/100 with 5 declared gaps (typed option, P-950), `builtin-tools.test.ts` 15/22 with 7 declared gaps, `agent-generalist-memory.test.ts` 3/3. No case of the three specs is left open.
- The title checker prints `ok` for the three specs only once the rows in `parity.md` (this folder) replace the three rows of `harness-rs/docs/PARITY.md`: the new tests live in `src/agent/tests/{provider_models, delegates, factories, sessions, mcp_delegates}.rs`, which the current rows don't name, and the counts changed. Checked by applying the staged rows to a scratch copy of `PARITY.md` in the worktree and reverting it. `test-title-map.json` needed no change (every new test has the snake_case title).

## For whoever integrates
- `Seams` has a third field, `on_resolve_model` (P-1051). Any `Seams { .. }` literal in another track's files (`h12b` adds `src/agent/tests/*.rs`) needs `on_resolve_model: None`; the `seams(&dir)` helper in `agent/tests.rs` already sets it.
- `agent/tests.rs` ends with the `mod` lines of this track's child modules (`delegates`, `factories`, `mcp_delegates`, `provider_models`, `sessions`); `h12b` puts its own after the `use` block.
- `strands-harness` has a new dev-dependency, `rmcp` with `server` (P-1055); `Cargo.lock` gained one line.
- SDK change: `Agent::interventions()` (P-1050), commit `feat(strands): Agent::interventions lists the agent's intervention handlers`.
- Production edits outside the test files, all without behaviour change: `agent.rs` (`Seams::on_resolve_model` and its call), `models.rs` and `models/build.rs` (`plan`, `Plan`, `Provider::name`, `build` and its four `*_options` functions are `pub(crate)`), `logging.rs` (test-only `warned`), `builtin_tools.rs` (`programmatic_tool_caller_options` extracted from `build_builtin_tools`).

## Limits of the ported cases (also in the PARITY row)
- The provider-model cases cover what `create_harness` asks `resolve_model` for and what that request builds; they don't read fields off the built model object.
- The cases whose TS parent takes `mcpServers: { srv: { command } }` pass a named client in `tools`.
- `keeps missing directories when multiple are given` shows the absent directory doesn't fail the load; the plugin's source list has no accessor.
