# h12b handoff

## Done
- `agent/tests.rs`: one block of `mod` lines after the `use` block (`config_e2e`, `internal`, `memory`, `openai_cache`, `plugins`); nothing else in that file changed. `openai_cache` is a fifth module beyond the four the brief names: it holds the two `openai-cache.test.ts` cases and the localhost `ResponsesServer` that `config_e2e` also uses.
- `memory.test.ts` 18/19: the five `createHarness memory` cases in `agent/tests/memory.rs`; `treats null as disabled` is a declared gap (P-1072).
- `openai-cache.test.ts` 2/2 in `agent/tests/openai_cache.rs` (P-1074).
- `plugins/todos.test.ts` 8/9 and `plugins/environment.test.ts` 11/11 in `agent/tests/plugins.rs`; `throws on an unknown built-in plugin name` is a declared gap (P-1073).
- `internal.test.ts` 2/2 in `agent/tests/internal.rs`; no `internal` module, the plumbing stays `pub(crate)` (P-1070).
- Portable config end to end: 9 tests in `agent/tests/config_e2e.rs` (P-1075). No wiring defect found, so there is no `fix(harness)` commit.
- `config.test.ts` (3 gaps) and `interventions.test.ts` (2 gaps): each reason checked against the TS case and the Rust types and rewritten per case in `test-title-map.json`; none can be expressed in Rust.
- No SDK change. One edit outside the test modules: `harness-rs/strands-harness/src/config.rs` gained `#[cfg(test)] pub(crate) use self::options::options_from_config;`.

## Left, case by case
- `openai-cache.test.ts`, both cases: the `bedrock-mantle/openai.gpt-6-luna` and `bedrock-mantle/openai.gpt-oss-120b` rows are not run. To run them the Mantle provider needs an endpoint override and a token provider reachable from the harness env seam (today `BedrockMantleConfig` has `region`, `credentials`, `expires_in_seconds` only).
- `keeps durable checkpoints ...`: the date assertions (`Date: 2030-01-0N` per turn, the stale date absent later) are replaced by counting `<environment>` blocks, because `plugins/environment.rs` reads `jiff::Zoned::now()` directly. A `pub(crate)` clock on `EnvironmentContext` would let the case assert the dates as TS does. The `expect(agent.model.getConfig()).toMatchObject({ cacheConfig: { strategy: 'auto' } })` line is not ported (no accessor for a provider model's config; `h12a` has the same need for the caching and effort cases of `agent.test.ts`).
- `memory.test.ts` `createHarness memory`: identity is asserted by behaviour (P-1071). If the SDK gains `MemoryManager` store access or pointer equality, the three instance cases can assert identity directly.
- Config end to end, not covered: `~` expansion, the `smart` and natural-language intervention presets, `contextManager: "auto"`, the per-tool `web_fetch`, `subagent` and `programmatic_tool_caller` configs, a Bedrock or Anthropic model string (only `openai/` can be pointed at a localhost server through the env seam).
- Not this track (`h12a`): the open cases of `agent.test.ts`, `builtin-tools.test.ts` and `agent-generalist-memory.test.ts`; the title checker still prints FAIL for those three only.

## Notes for whoever touches these tests
- `ResponsesServer::start(replies)` answers request N with `replies[N]` and a request past the script with a 500; `server.env(&[..])` is the env seam with the dummy key and `OPENAI_BASE_URL`. `Reply::ToolCall` always uses call id `call_1`.
- The stdio MCP double in `config_e2e.rs` is a `sh` loop (unix only, `mcp` feature). It must answer unknown requests with a JSON-RPC error: the SDK client first sends `server/discover` and waits 10 s for a reply before it falls back to `initialize`.
- Plugin tools (`todo_write`, `skills`, `search_memory`) are registered at `agent.initialize()`, not at `create_harness`; a test that looks for them on an uninitialized agent passes vacuously for the absent case.
- Title check with the staged rows applied: `internal 2/0/0/0`, `memory 18/0/1/0`, `openai-cache 2/0/0/0`, `todos 8/0/1/0`, `environment 11/0/0/0`, `config 29/0/3/0`, `interventions 9/0/2/0`, all `ok`.

## Gates
- `GATE_OUT=/mnt/workspace/gates-h12b .rust-port/gates.sh` (default mode) on commit `745a1bf09`: `fmt`, `clippy`, `clippy_nodef`, `test_all`, `test_nodef`, `doc` all 0; 5121 tests passed with all features.
- After the last test was added: `cargo fmt --all --check`, both `cargo clippy -p strands-harness` runs with `-D warnings`, and both `cargo test --locked -p strands-harness` runs (514 lib tests with all features, 413 without default features) pass. `strands-harness --lib` was run six times in a row with no failure.
