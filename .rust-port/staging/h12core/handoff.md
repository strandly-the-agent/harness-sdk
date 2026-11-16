# h12core handoff

## Done
- `builtin_tools.rs`: `resolve_builtin_tools`, `enabled_builtin_tools`, `builtin_tool_config`, `web_search_explicit`, `web_search_mode`, `BuildAgent`, `build_builtin_tools`, `select_builtin_tools`, `build_default_subagent` (+ `child_options`). Spec: `builtin_tools/tests.rs`.
- `agent.rs` `create_harness` / `create_harness_with(options, Seams)`: every step of `agent.ts` lines 347-522 is written in TS order (passthrough check, telemetry, built-ins and web_search mode, model, system prompt, `mcp_servers`, the parent options for subagents, built-in tools, session + stash, skills, built-in plugins, memory, name collisions, interventions, background tasks, `Agent::new`).

## Next
- `agent.ts` lines 523-526: drop an explicit sandbox's vended tools after initialize (`InitializedEvent` hook). Not written.
- Port the rest of `agent.test.ts` (see `parity.md` for what is ported), `internal.test.ts`, `agent-generalist-memory.test.ts`.
- `lib.rs`: `pub use agent::create_harness` is not added yet if this file still says so.

## Design choices to keep
- See `decisions.md`.
- `create_harness_with` returns a `BoxFuture` (not `async fn`) because the default subagent's builder calls back into it; an `async fn` makes the `Send` proof cyclic.
- `Seams { base_dir, env }` is the test seam: relative default paths (`./.agent/sessions`, `./.agent/skills`, `./.agent/memory`) are joined to `base_dir` when it is `Some`; production passes `None` and the paths stay as TS writes them.
