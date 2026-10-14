# harness wiring to phase-3 SDK features

Branch `agent-tasks/strands-rs-h-wire` (worktree `/home/sandbox/wt/h-wire`), 3 commits.

## Public API changes (`strands-harness`)
- `interventions::{InterventionsOption::{Off, Values(Vec<InterventionValue>)}, InterventionValue::{Preset, Handler}, ResolveInterventionsOptions{ask}, InterventionsOption::presets(..)}`;
  `resolve_interventions(&InterventionsOption, &ResolveInterventionsOptions) -> Result<Vec<Arc<dyn strands_agents::InterventionHandler>>>`.
  The placeholder empty `enum InterventionHandler` is gone.
- new `telemetry` module: `TRACES_EXPORTER_ENV_VAR`, `SUPPORTED_EXPORTERS`, `resolve_exporters()`,
  `setup_telemetry() -> Option<TelemetryConfig>`, `reset_telemetry()`.
- `HarnessAgentOptions`: `mcp_servers: Vec<String>` → `Option<McpServersOption>`
  (`McpServersOption::{Path(PathBuf), Servers(IndexMap<String, McpServerConfig>)}`); new fields
  `interventions_ask: Option<AskFn>`, `sandbox: SandboxOption` (`Default`/`Off`/`Instance`),
  `trace_attributes: Vec<(String, AttrValue)>`.
- new cargo feature `mcp` (on by default) = `strands-agents/mcp`; new dep `indexmap`.

## Landed
1. **interventions** — `off` → no handler, `ask` → `HumanInTheLoop` (optional `ask` callback
   threaded through), handler instances pass through, TS collision text verbatim
   (`"Two interventions share the handler name '…'"`). `create_harness` → `AgentBuilder::interventions(..)`.
   `smart`, `*.cedar`, and prose policies are hard errors naming the missing capability
   (the Rust `HumanInTheLoop` has no classifier and the SDK ships no Cedar handler) —
   TS supports them, Rust can't yet; a silently weaker guardrail would be worse.
2. **MCP** — `load_mcp_tools` calls `McpClient::load_servers(source, LoadOptions{continue_on_error:true},
   ListOptions{prefix_with_server_name:true})` and appends the tools to the **consumer** tool list
   (so they are name-collision checked, matching TS `consumer = [...tools, ...mcpServers?]`).
3. **telemetry** — `OTEL_TRACES_EXPORTER` parsing per TS (unset/`none` → off, case-insensitive,
   unsupported names `warn_once`-skipped with TS's message), once-per-process, result wired via
   `AgentBuilder::telemetry(TelemetryConfig::from_env())` + `trace_attributes` passthrough.
   **Gap (documented in the module docs):** the Rust SDK has no `setupTracer` and this crate pulls in
   no OTLP/console exporter dep, so a selected exporter cannot be *installed* here — the harness
   configures the agent for tracing and warns once that the `TracerProvider` is the embedder's to
   install on the OTel global API.
4. **sandbox** — `SandboxOption::Default` registers `LocalSandbox` as the process default when none
   is registered (TS default-slot semantics); `Off` → `no_sandbox()`; `Instance` → `sandbox_arc(..)`
   plus `drop_sandbox_tools` (h1 step 23: remove the sandbox's vended tools from the registry after
   build, since the harness ships its own `shell`/`read`/`write`/`edit`).

## Skipped
5. **`models.rs` provider arms** (`anthropic`/`openai`/`bedrock-mantle` → real models with the TS
   effort/thinking/caching mapping) — not started; the 15-minute budget went to items 1–4 + tests.
   `litellm`/`gemini`/`openrouter` were already the "unsupported" arms and are unchanged.

## Tests
Unit: 6 interventions (default/off, `ask` → HITL, handler passthrough, classifier+cedar+prose
rejection, collision text, ask callback threading), 6 telemetry (unset, `none`, case-insensitive
multi, unsupported-name skip, once-per-process, semconv opt-in from env), 3 MCP option
(unset/empty, missing path errors, disabled server skipped), 3 agent wiring (sandbox default slot,
explicit-sandbox tool drop, interventions resolved into the agent incl. `smart` error).
Live: `integ_harness.rs::interventions_deny_the_shell_tool` (`#[ignore = "live Bedrock"]`) — scripted
`ask` returns `"n"`, asserts the callback ran and the reply reports the denial. Not executed (no
live run in this budget).

## Gates (verbatim last lines, from `strands-rs/`)
- `cargo fmt --check` → `FMT CLEAN` (no output; cargo fmt applied)
- `cargo clippy --all-targets --all-features -p strands-harness -- -D warnings` → `    Finished \`dev\` profile [unoptimized] target(s) in 0.13s`
- `cargo test --all-features -p strands-harness` → `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` (lib: `188 passed; 0 failed`; integ: `0 passed; 7 ignored`; prompt_parity: `1 passed`)
- `cargo build --examples --all-features` → `    Finished \`dev\` profile [unoptimized] target(s) in 19.46s`
