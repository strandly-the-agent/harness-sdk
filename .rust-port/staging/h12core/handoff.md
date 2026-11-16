# h12core handoff

## Done (code)
- `builtin_tools.rs`: `resolve_builtin_tools`, `enabled_builtin_tools`, `builtin_tool_config`, `web_search_explicit`, `web_search_mode`, `BuildAgent`, `build_builtin_tools`, `select_builtin_tools`, `build_default_subagent` (+ the pure `child_options`).
- `agent.rs`: `create_harness` (public, re-exported from `lib.rs`) and `create_harness_with(options, Seams)`. Every step of `agent.ts` `createHarness` (lines 347-528) is written, in TS order: passthrough check, telemetry, built-ins and web_search mode, model, system prompt, `mcp_servers`, the parent options for subagents, built-in tools, session + stash, skills, built-in plugins, memory, name collisions, interventions, background tasks, `Agent::new`, the sandbox-tools hook.
- Helpers in `agent.rs`: `load_mcp_servers`, `sanitize_session_id`, `random_session_id`, `session_storage`, `has_skills`, `skills_plugin`, `select_builtin_plugins`, `plugin_tools`, `check_name_collisions`, `resolve_background_tasks`, `drop_sandbox_tools`.

## Done (tests)
- `builtin_tools/tests.rs` (14/22 of `builtin-tools.test.ts`), `agent/tests.rs` (52/100 of `agent.test.ts`). `parity.md` lists every ported and unported case by TS title.

## Next, in this order
1. `agent.test.ts` cases still open (43, listed in `parity.md`). Cheapest first: interventions (3), `lets an explicit context manager win`, the caching and effort cases (need the provider model's config accessor), `attaches a ModelRouter`, the delegate cases. For the delegate cases, `agent/tests.rs` `delegate()` runs the real `subagent` tool on a shared `MockMessageModel` and reads the child's `ModelRequest`; what it cannot see (child session manager, memory writability, interventions) needs the parent options: extract the `parent` literal in `create_harness_with` into a function and test `child_options` + `create_harness_with` on its result.
2. `drops sandbox-vended tools`: `drop_sandbox_tools` is untested; write a `Sandbox` double whose `get_tools()` returns two tools.
3. `internal.test.ts` (2) and `agent-generalist-memory.test.ts` (3): not started.
4. `harness_agent_options_from_config` end to end into `create_harness` (tracker item): not started; `config::options::options_from_config` already yields `HarnessAgentOptions`.
5. The full gate script (`.rust-port/gates.sh`) was not run; only the per-crate steps were.

## Design choices the next implementer must keep
- `decisions.md` P-950..P-963; `divergences.md`.
- Keep `create_harness_with` returning a `BoxFuture` (P-955), and keep all path and env reads behind `Seams`.
- `resolve_builtin_tools` stays infallible and typed (P-950); don't add a JSON front door, `config/normalize.rs` owns the input-shape errors.
- The child of a subagent is built by `child_options` (pure) then `BuildAgent`; test rules there.
- `warn_once` dedups per process, so tests must not assert on the Exa third-party warning or the MCP-name warnings unless each message is emitted by exactly one test.
