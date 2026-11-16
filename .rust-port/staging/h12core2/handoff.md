# h12core2 handoff (replaces `h12core-handoff.md`)

## Done (code)
- `builtin_tools.rs`: `resolve_builtin_tools`, `enabled_builtin_tools`, `builtin_tool_config`, `web_search_explicit`, `web_search_mode`, `BuildAgent`, `build_builtin_tools`, `select_builtin_tools`, `build_default_subagent` (+ the pure `child_options`).
- `agent.rs`: `create_harness` (public, re-exported from `lib.rs`) and `create_harness_with(options, Seams)`. Every step of `agent.ts` `createHarness` (lines 347-528) is written, in TS order: passthrough check, telemetry, built-ins and web_search mode, model, system prompt, `mcp_servers`, the parent options for subagents, built-in tools, session + stash, skills, built-in plugins, memory, name collisions, interventions, background tasks, `Agent::new`, the sandbox-tools hook.
- Helpers in `agent.rs`: `load_mcp_servers`, `sanitize_session_id`, `random_session_id`, `session_storage`, `has_skills`, `skills_plugin`, `select_builtin_plugins`, `plugin_tools`, `check_name_collisions`, `resolve_background_tasks`, `drop_sandbox_tools`.

## Done (tests)
- `builtin_tools/tests.rs` (14/22 of `builtin-tools.test.ts`), `agent/tests.rs` (58/100 of `agent.test.ts`, 2/3 of `agent-generalist-memory.test.ts`). `parity.md` lists every ported and unported case by TS title.

## Next, in this order
1. `agent.test.ts` cases still open (37, listed in `parity.md`). Cheapest first: interventions (2 left: `registers no interventions by default` and `wires an interventions preset`; `Agent` has no public accessor for its intervention handlers, `InterventionRegistry::handlers` is `pub(crate)`, so both need a small `feat(strands)` accessor; the handler-instance case is ported with the `Observed` double in `agent/tests.rs`), the caching and effort cases (need the provider model's config accessor), `attaches a ModelRouter`, the delegate cases. Two test helpers exist for delegates in `agent/tests.rs`: `delegate()` runs the real `subagent` tool on a shared `MockMessageModel` and returns the child's `ModelRequest` tool names and prompt; `child_options_of()` does the same through `create_harness_from` with a capturing `BuildAgent` and returns the `HarnessAgentOptions` the child was built from. Neither can narrow (`tools`/`mcp_servers` are not offered on the default subagent's schema), so the narrowing cases go on `builtin_tools::child_options` directly, with a parent taken from `child_options_of`.
2. `drops sandbox-vended tools`: done (`VendingSandbox` in `agent/tests.rs`).
3. `internal.test.ts` (2): not started; the crate has no `internal` module, so the first case needs a decision on how Rust exposes the CLI plumbing (`pub mod internal` re-exporting `normalize_harness_agent_config`, `resolve_model`, `resolve_memory`, `resolve_interventions`, `resolve_builtin_tools`, `enabled_builtin_tools`, `builtin_tool_config`), and the second is a compile-time property. `memory.test.ts`: 2 of the 8 `createHarness memory` cases are ported in `agent/tests.rs` (their `test-title-map.json` entries removed); 6 left, cheapest `honors a custom memory dir` and the two `MemoryManager` instance cases. `agent-generalist-memory.test.ts`: 1 case left (`forwards the parent memory stores ...`).
4. `harness_agent_options_from_config` end to end into `create_harness` (tracker item): not started; `config::options::options_from_config` already yields `HarnessAgentOptions`.
   - `loads skills from multiple directories` and `keeps missing directories when multiple are given`: `AgentSkills` has no source accessor, and `get_available_skills(None)` returns only skills given directly or by URL (path sources load through the agent's sandbox at init), so these need an initialized agent that holds the plugin `skills_plugin` returned, then `get_available_skills(Some(agent id))`.
   - No decision numbers were used by `h12core2` (P-964..P-979 are all free).
5. The full gate script (`.rust-port/gates.sh`) was not run; only the per-crate steps were.

## Design choices the next implementer must keep
- `decisions.md` P-950..P-963; `divergences.md`.
- Keep `create_harness_with` returning a `BoxFuture` (P-955), and keep all path and env reads behind `Seams`. `create_harness_from(options, seams, build_agent)` is the body; `create_harness_with` only supplies the recursive `BuildAgent`, so tests can pass a capturing one.
- `resolve_builtin_tools` stays infallible and typed (P-950); don't add a JSON front door, `config/normalize.rs` owns the input-shape errors.
- The child of a subagent is built by `child_options` (pure) then `BuildAgent`; test rules there.
- `warn_once` dedups per process, so tests must not assert on the Exa third-party warning or the MCP-name warnings unless each message is emitted by exactly one test.
