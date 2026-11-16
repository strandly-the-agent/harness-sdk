# Phase 12 review gate: API bar-raiser (`strands-harness`)

Reviewed commit: `c2c6d557d` at `/mnt/workspace/review-h12`. TS spec: `/tmp/ts-spec/harness-ts` (pin `7296caba7`).
Findings are appended as they are verified; the "not reviewed" list and the verdict are at the end.

Evidence crates: `/tmp/review12_api/probe` and `/tmp/review12_api/probe2` (path dependencies on the review worktree; one target per file under `tests/`, see their `README.md`), copied to `/mnt/workspace/reviews/phase12-review-api-bar-raiser-probe/`. Commands were run with the toolchain exports of the common brief; nothing in the worktree was changed, no live test and no AWS call was made.

## Findings

### api-1. The section 13 harness example does not compile: `McpServersOption` has no `From<&str>` / `From<String>`

`harness-rs/strands-harness/src/agent.rs:47` (`McpServersOption`), `agent.rs:257-258` (`#[builder(into)] pub mcp_servers: Option<McpServersOption>`).

PROMPT.md section 13 (`.rust-port/PROMPT.md:1175-1183`, "must compile as doctests or examples") has `HarnessAgentOptions::builder().model("anthropic/claude-opus-5").effort(Effort::High).mcp_servers("./mcp.json").interventions("ask").build()`. The only conversion into `McpServersOption` is `From<IndexMap<String, McpServerConfig>>` (`agent.rs:78`, `mcp` feature only), so the TS `mcpServers: './mcp.json'` form (`harness-ts/src/agent.ts:120`, `string | Record<string, McpServerConfig>`) has to be written `McpServersOption::Path("./mcp.json".into())`.

Evidence: scratch crate `/tmp/review12_api/probe` (`tests/dx.rs` is the section 13 snippet verbatim), `cargo check --offline --test dx` (the only error; with `McpServersOption::Path(..)` instead, `tests/dx_with_path_variant.rs`, the snippet compiles):

```
error[E0277]: the trait bound `McpServersOption: From<&str>` is not satisfied
  --> tests/dx.rs:11:26
11 |             .mcp_servers("./mcp.json")
   = help: the trait `From<&str>` is not implemented for `McpServersOption`
           but trait `From<IndexMap<std::string::String, McpServerConfig>>` is implemented for it
```

Fix: add `impl From<&str> for McpServersOption`, `impl From<String>` (and `From<&Path>`/`From<PathBuf>` if paths are wanted) mapping to `Path`, plus `impl From<JsonObject>` for `Servers` (the variant's own payload has no conversion either); add the section 13 snippet as a `no_run` doctest on `create_harness` so it stays compiling (`create_harness` and `HarnessAgentOptions` have no doc example, see api-2).

The same probe (`tests/names.rs`) shows the one other TS union member without a conversion: `skills?: boolean | SkillSource | SkillSource[] | AgentSkills` (`harness-ts/src/agent.ts:173`) has no `From<SkillSource>` for `SkillsOption` (`E0277: SkillsOption: From<SkillSource> is not satisfied`); add it next to `From<Vec<SkillSource>>` (`agent.rs:183`).

### api-2. No `bon` builder type or state module of the crate is nameable from outside; four builders also lack `Debug`

`harness-rs/strands-harness/src/lib.rs:28-58`: the crate root re-exports the config structs but none of their builders (`HarnessAgentOptionsBuilder`, `HarnessAgentConfigBuilder`, `WebFetchConfigBuilder`, `BuiltinToolsConfigBuilder`, `SessionConfigBuilder`, `MemoryConfigBuilder`, `PresetBuilder`, `MakeSubagentOptionsBuilder`, `MakeWebFetchOptionsBuilder`, `MakeProgrammaticToolCallerOptionsBuilder`, `TodosConfigBuilder`, `EnvironmentContextConfigBuilder`, `ReadConfigBuilder`, `ShellConfigBuilder`, `SubagentConfigBuilder`, `ProgrammaticToolCallerConfigBuilder`, `HarnessConfig{WebFetch,BuiltinTools,Memory}Builder`, `HarnessDependenciesBuilder`) nor their state modules, and every module is private. The `state_mod(vis = "pub")` on them (for example `agent.rs:235`) therefore has no effect: a caller cannot write a function that takes or returns a partly built `HarnessAgentOptionsBuilder<S>` (the usual way to share builder setup with bon's typestate).

`strands-rs/AGENTS.md:70` (applies to the harness "unchanged", `harness-rs/AGENTS.md:3`): "every public `bon` builder has `#[builder(state_mod(vis = "pub"))]`, and the builder and its state module are re-exported in every module-level `pub use` that names the config ... Every builder implements `Debug`". Four builders break the second half too: `plugins/todos.rs:116-117` and `plugins/environment.rs:77-78` (`#[builder(on(String, into))]` only), `tools/web_fetch.rs:319` (no `#[builder(..)]` at all), `tools/subagent.rs:240-241` (no `Debug`; it holds a callback, so `opaque_builder_debug!`-style).

Evidence (`/tmp/review12_api/probe/tests/names.rs`, `cargo check --offline --tests`):

```
error[E0425]: cannot find type `HarnessAgentOptionsBuilder` in crate `strands_harness`
error[E0277]: `strands_harness::plugins::todos::TodosConfigBuilder` doesn't implement `std::fmt::Debug`
error[E0277]: `strands_harness::plugins::environment::EnvironmentContextConfigBuilder` doesn't implement `std::fmt::Debug`
error[E0277]: `MakeWebFetchOptionsBuilder<SetModel>` doesn't implement `std::fmt::Debug`
error[E0277]: `MakeSubagentOptionsBuilder<SetBuilder>` doesn't implement `std::fmt::Debug`
```

The builders are also absent from the rendered docs: `cargo doc -p strands-harness --all-features --no-deps` writes no `struct.*Builder.html` (`find $CARGO_TARGET_DIR/doc/strands_harness -name '*Builder*'` finds only the `AgentBuilder` alias), and `struct.HarnessAgentOptions.html` shows `pub fn builder() -> HarnessAgentOptionsBuilder` as unlinked text, so the setters of the primary API (#32) and what each accepts through `into` are documented nowhere.

The workspace lint `missing_debug_implementations` (`Cargo.toml`, `[workspace.lints.rust]`) does not see the four because the types are unreachable; it will once they are exported.

Fix: re-export each builder and its state module where the config is re-exported. The harness has no public module, so either add them to the crate-root `pub use` lists or add one public `builders` module holding them (log the choice, since the SDK rule keeps builders off its crate root). Add `state_mod(vis = "pub"), derive(Debug)` to `TodosConfig`, `EnvironmentContextConfig` and `MakeWebFetchOptions`, and a type-name-only `Debug` for `MakeSubagentOptionsBuilder`.

### api-3. `Effort` cannot be parsed from or printed as its TS name; `parse_effort` and `HarnessError::InvalidEffort` are unreachable

`harness-rs/strands-harness/src/types.rs:18` (`Effort`: no `FromStr`, `Display` or `as_str`), `src/models.rs:128-143` (`pub(crate) fn parse_effort`, `expect(dead_code, reason = "used by the config normalization, which lands separately")`), `src/error.rs:41-48` (`HarnessError::InvalidEffort`), `src/config/normalize.rs:60,327-335` (a second effort table that reports through `InvalidConfig` instead).

The config normalization has landed and does not use `parse_effort` (the `expect(dead_code)` still holds, so nothing outside tests calls it), which leaves the public variant `HarnessError::InvalidEffort` with no producer. `harness-rs/docs/DIVERGENCES.md:13` tells users "the same text comes from parsing a name (`parse_effort`)", a function they cannot call. A CLI or config caller that has `"high"` as a string (TS: `effort: 'high'`, `harness-ts/src/agent.ts:92`) must go through `serde_json`. The other name enums expose `as_str` (`BuiltinToolName`, `BuiltinPluginName`, `ContextMode`, `HarnessModuleKind`, `HarnessModuleLanguage`).

Evidence (`tests/names.rs`):

```
error[E0277]: the trait bound `strands_harness::Effort: FromStr` is not satisfied
error[E0599]: `strands_harness::Effort` doesn't implement `std::fmt::Display`
```

Fix: `impl FromStr for Effort { type Err = HarnessError; .. }` delegating to `parse_effort` (drop the `expect(dead_code)`), `Effort::as_str` from the existing `effort_name`, and build the `normalize.rs` table from `models::EFFORT_LEVELS` so one table remains. If the lead prefers no parser, delete `parse_effort` and `InvalidEffort` and correct `DIVERGENCES.md:13`.

### api-4. Spans cannot be flushed: `shutdown_telemetry` is dead `pub(crate)` code and the provider is private (P-923 is still open)

`harness-rs/strands-harness/src/telemetry.rs:93` (`static TELEMETRY`, holds the `SdkTracerProvider`), `telemetry.rs:126-142` (`pub(crate) async fn shutdown_telemetry`, `expect(dead_code, reason = "called at exit by the harness's callers, which land separately")`).

`create_harness` calls `setup_telemetry` (`agent.rs:391`) on every build with the default `otel` feature, and keeps the provider `setup_tracer` returns in a private static. The SDK's own docs (`strands-rs/strands/src/telemetry/mod.rs:20-26`) say the caller must run `tracer_provider.shutdown()` before the process exits to flush the batch exporter; a harness user has no handle to do that, and nothing in the crate calls `shutdown_telemetry` (`grep -rn shutdown_telemetry harness-rs` finds only the definition and its unit test). P-923 records the question and leaves it to "the `create_harness` track or the lead"; `create_harness` has landed without a caller. With `OTEL_TRACES_EXPORTER=otlp` a short harness run therefore drops its last batch of spans. TS does not have the problem: the SDK's `setupTracer` registers `process.once('beforeExit', () => _provider.forceFlush())` (`strands-ts/src/telemetry/config.ts:213-214`) and returns the existing provider when called again (`config.ts:187-188`), so a TS harness run flushes on exit without the user doing anything. Rust has no exit hook and `opentelemetry::global` has no shutdown; the SDK's answer (`strands-rs/docs/DIVERGENCES.md:175`: "The caller calls `shutdown()` on the returned provider") is exactly what the harness withholds.

Fix: make it public (`pub async fn shutdown_telemetry() -> Result<()>`, re-exported from the crate root, with a no-op twin without `otel` so the name does not change with features), document it on `create_harness` next to `agent.shutdown()`, and add the row to `DIVERGENCES.md` (a Rust-only name needed because the provider handle is private). Resolve P-923 either way; leaving the function dead with a "lands separately" reason is not a state to ship.

### api-5. SDK: two public `ToolAgent` methods for one job; `call_tool` has no caller and the two mint different id shapes

`strands-rs/strands/src/types/agent.rs:618-651` (`ToolAgent::call_tool` and `ToolAgent::call_tool_with_id_prefix`, both added in this phase's diff).

Section 7.4 asks for "the narrowest public API that does the same job" and names `ToolAgent::call_tool`. The phase adds that method and, for P-992, a second one beside it. The only non-test caller of either is `harness-rs/strands-harness/src/tools/programmatic_tool_caller.rs:114` (`call_tool_with_id_prefix(&name, "ptc_", ..)`); `grep -rn '\.call_tool(' strands-rs/strands/src harness-rs/strands-harness/src` outside `mcp/` finds `call_tool` only in `types/agent/tests.rs`. The two also disagree on the id: `call_tool` mints `call_<uuid v7, simple>` (32 hex, no hyphens), the prefix form `<prefix><uuid v7, hyphenated>`, and the rustdoc of `call_tool` does not say which id hooks and spans will see. P-992 rejected changing `call_tool`'s signature because it "changes the existing signature", but that signature is new in this same unreleased diff.

Fix: keep one method. Either `call_tool(name, input, invocation_state)` gains the prefix as the only form (`call_tool(name, id_prefix, input, state)`), or it takes the caller's `ToolUseBlock` (`ToolUseBlock::new(name, id, input)` is already public and is what `executor::call_tool` takes), with the harness minting `ptc_<uuid>`. Document the id on whichever remains, and that an unknown tool name, a dropped stream update and an unrecorded interrupt behave as `executor.rs` describes (today only the `pub(crate)` function's comment says so).

### api-6. Crate docs, README and `AGENTS.md` do not match the bar: no `create_harness` example anywhere, inert docs.rs metadata, stale layout

- `harness-rs/strands-harness/src/lib.rs:1-7` (crate rustdoc) and `src/agent.rs:332-343` (`create_harness`), `agent.rs:228-236` (`HarnessAgentOptions`): no example and no feature overview. The crate has five doc examples in total (`prompt.rs:29`, `interventions.rs:81`, `models.rs:571,594`, `tools/subagent.rs:305`); none shows `create_harness`, the builder, the struct-literal form or the `agent_config` passthrough. Section 13 requires its harness snippet to compile as a doctest or example (it does not compile today, api-1), and the SDK's Phase 11 bar was "crate-level rustdoc with a module and feature overview, and the DX examples in section 13 as doctests" (`.rust-port/PROMPT.md:1004`).
- Nothing in the public docs says to call `agent.shutdown().await` (`grep -n shutdown harness-rs/strands-harness/src/agent.rs src/lib.rs src/types.rs harness-rs/README.md` is empty). TS documents it on `memory` (`harness-ts/src/agent.ts:183-187`: the latest turns are unsaved until shutdown) and relies on `await using`; Rust has neither a destructor nor the note, and the default options turn memory extraction and MCP clients on.
- `agent.rs:268-270` (`context_manager`: "SDK context management; `None` is the `auto` preset") does not say how to ask for the other two TS values. `'agentic'` is `strands::experimental::context_manager::ContextManagerPreset::Agentic` (not re-exported by the harness) and `false` is `ContextManagerOption::Off`; `.context_manager("agentic")` and `.context_manager(false)` do not compile (`tests/forms2.rs`).
- `harness-rs/README.md` (11 lines): no usage example, no cargo feature list (`bedrock`, `anthropic`, `openai`, `mcp`, `otel`, all default), no word that `default-features = false` turns providers into `HarnessError::ModelFeatureDisabled`, no pointer to `docs/DIVERGENCES.md`.
- `harness-rs/strands-harness/Cargo.toml:49-51`: `rustdoc-args = ["--cfg", "docsrs"]`, but `lib.rs` never reads `docsrs` (`grep -rn docsrs harness-rs/strands-harness/src` is empty; the SDK has `#![cfg_attr(docsrs, feature(doc_cfg))]` at `strands-rs/strands/src/lib.rs:252`). Built with `RUSTDOCFLAGS="--cfg docsrs" cargo doc -p strands-harness --all-features --no-deps`, `fn.exa_web_search.html` and `fn.make_exa_web_search.html` carry no "Available on crate feature `mcp`" note (`grep -c 'Available on'` is 0 for both).
- `harness-rs/docs/DIVERGENCES.md:48-52`: five rows have two cells in the four-column table (`Area | TS behaviour | Rust behaviour | Reason`), so the Rust behaviour renders under "TS behaviour" and the TS side is missing.
- `harness-rs/AGENTS.md:11-20` (layout): omits `types.rs`, `error.rs`, `env.rs`, `logging.rs` and `test_support.rs`, and lists `docs/reviews/`, which does not exist (`ls harness-rs/docs`: `DIVERGENCES.md PARITY.md test-title-map.json`). `AGENTS.md:32` gives the default session storage as `LocalFileStorage::new("./.agent").with_namespace("sessions")`; the code calls the `Storage::namespace` trait method on the parent of the configured directory (`src/agent.rs:684-701`), as #70 words it.

Fix: add the section 13 snippet as a `no_run` doctest on `create_harness` and a struct-literal plus `agent_config` example on `HarnessAgentOptions`; give `lib.rs` a short feature table; either use `docsrs` (`#![cfg_attr(docsrs, feature(doc_cfg))]` plus `doc(cfg(feature = "mcp"))` on the two Exa functions and the `From<IndexMap<..>>` impl) or drop the `rustdoc-args` line; add usage and features to the README; give the five divergence rows their four cells; correct the layout block.

### api-7. `Debug` of `HarnessAgentOptions` prints an MCP server's `auth.clientSecret`

`harness-rs/strands-harness/src/agent.rs:55-75` (hand-written `Debug` of `McpServersOption`, redacts only `env` and `headers`). The SDK's `McpClientCredentials` redacts the secret in its own `Debug` (`strands-rs/strands/src/mcp/client.rs:127`), and the typed map goes through `From<IndexMap<String, McpServerConfig>>` (`agent.rs:77-86`) into JSON, where the secret is a plain string again. This is the 12a fix for correctness-a8 being incomplete (the Phase 12 correctness report lists the same hole as its correctness-4 addendum; reported here because it is on the public options type).

Evidence: `/tmp/review12_api/probe/tests/debug_secret.rs`, `cargo test --offline --test debug_secret`:

```
HarnessAgentOptions { .. mcp_servers: Some(Servers({"remote": Object {"url": String("https://example.com/mcp"), "auth": Object {"clientId": String("id"), "clientSecret": String("s3cr3t-value")}}})), ..
test result: FAILED. 0 passed; 1 failed
```

Fix: redact `auth.clientSecret` (or every value under `auth`) in the same function, and share it with `HarnessConfigMcpServers`.

### api-8. `AgentSpec` and `TodoItem` are exhaustive although they are a result and a wire struct (#65)

- `harness-rs/strands-harness/src/tools/subagent.rs:191-214`: `AgentSpec` is `#[expect(clippy::exhaustive_structs, reason = "data struct the builder reads field by field")]`. It is not a config: the harness builds it (`resolve.rs`) and hands it to the caller's `AgentBuilder`, who only reads it, so it falls under "results ... are `#[non_exhaustive]`" (`.rust-port/PROMPT.md:241`). It has one field per axis (`harness-ts/src/tools/subagent.ts:168-176`), so a new axis is a new field, and a new field breaks every caller that builds one with a struct literal, for example in tests of their builder. It already has the constructor (`AgentSpec::new`, `subagent.rs:218`), as TS has (`subagent.ts:178`). The SDK's twin type is `#[non_exhaustive]` with public fields (`strands-rs/strands/src/vended_tools/subagent/spec.rs:216-218`, P-281).
- `harness-rs/strands-harness/src/plugins/todos.rs:49-63`: `TodoItem` is the persisted app-state shape (serde, camelCase), a wire struct, and is exhaustive with no constructor. #65: "wire structs are `#[non_exhaustive]`, and wire structs get constructors (`new(..)` plus `with_*`)".

Neither has a pending-decision entry (`grep -n 'TodoItem\|AgentSpec' .rust-port/DECISIONS-pending.md`: P-892 and P-910 give the field types only).

Fix: `#[non_exhaustive]` on both; `TodoItem::new(content, active_form, status)`. Reading fields and `AgentSpec::new(..)` followed by field assignment keep working from other crates.

### api-9. `HarnessError` fields that are strings where a type or a source exists

`harness-rs/strands-harness/src/error.rs`:

- `:145-151` `InvalidAgentConfigValue { key, message: String }`, built at `src/config/options.rs:164-167` with `message: error.to_string()`: the `serde_json::Error` is dropped, so `Error::source()` is `None`. `McpConfigInvalidJson` (`error.rs:121-128`) keeps its cause as `#[source]`; this one should too (`#[source] source: serde_json::Error`, message text unchanged through `{source}`).
- `:50-58` `UnsupportedEffort { effort: String, .. }`, built at `src/models.rs:265-269` from `effort_name(effort)`: the caller holds an `Effort` and gets its name back as a string. Make the field `effort: Effort` and format the name in the `#[error]` attribute.
- `:42-48` `InvalidEffort { supported: String }`, `:27-40` `UnknownModelProvider { supported: String }`, `:87-93` `UnknownAgentType { valid: String }`: comma-joined lists as public fields. They exist only to render the TS text; either hold `Vec<String>`/`&'static [&'static str]` and join in the `#[error]` attribute (as `InvalidConfig { issues: Vec<String> }` does at `:106-110`), or do not expose them as fields.

All are public fields of a `#[non_exhaustive]` enum's variants, so changing a field type later is a breaking change; this is the cheap moment.

### api-10. P-993 (open): `GENERALIST`, `GENERALIST_INSTRUCTIONS` and `CONTEXT_MODES` should not be at the crate root

`harness-rs/strands-harness/src/lib.rs:47-48`, `src/tools/subagent.rs:76,182,185`.

`harness-ts/src/index.ts:43-73` and `harness-ts/src/tools/index.ts:18-28` export none of the three; section 7.1 maps anything outside the `index.ts` barrel to `pub(crate)`. `GENERALIST` also puts `std::sync::LazyLock<Preset>` into the public API as a `static`, a type commitment TS does not have. P-993 kept them public only because the `make_subagent` doc example (`subagent.rs:305-321`) imports `GENERALIST`. That example does not need it: `Preset::builder().instructions("...").description("...").build()` is the public way to make a preset and is what a caller would write.

Fix: option (a) of P-993: `pub(crate)` for the three, and the example builds its own `Preset`. If the lead keeps the preset public, make it a function `generalist() -> Preset`, as the SDK's vended subagent already does (`strands-rs/strands/src/vended_tools/subagent.rs:67`) and as this crate does for the other TS constants that cannot be `const` (`read()`, `programmatic_tool_caller()`, P-874), not a `LazyLock` static. (`McpServersMode`, also absent from the TS barrel, is different: it is the type of the public field `MakeSubagentOptions::mcp_servers`, so it has to be nameable; P-892 covers it.)

### api-11. The portable sub-config types derive `Deserialize` with rules that differ from the normalizer

`harness-rs/strands-harness/src/config.rs:237,252,277,301,327` (`ReadConfig`, `ShellConfig`, `HarnessConfigWebFetch`, `ProgrammaticToolCallerConfig`, `SubagentConfig`), `:395` (`HarnessConfigBuiltinTools`), `:454` (`HarnessConfigBuiltinToolsSetting`), `:507,517,528,541` (`HarnessConfigMemory`, `HarnessConfigMcpServers`, `HarnessConfigSkills`, `HarnessConfigInterventions`), `:555` (`HarnessDependencies`), `src/types.rs:174` (`SessionConfig`).

`HarnessAgentConfig` deserializes through `normalize_harness_agent_config` (`config.rs:655-660`), which is the only place the TS rules and texts live. The nested public types each derive their own `Deserialize`, which nothing in the crate calls (`grep -rn 'from_value\|from_str' src` outside tests: only `agentConfig` values and the MCP file), so a caller who deserializes one of them gets a second, looser validator with serde's messages. Example (`/tmp/review12_api/probe/tests/roundtrip.rs`, `nested_types_deserialize_alone`):

```
serde_json::from_value::<ProgrammaticToolCallerConfig>(json!({"timeout": -5}))
=> Ok(ProgrammaticToolCallerConfig { allowed_tools: None, timeout: Some(Some(-5.0)) })
```

The normalizer rejects the same value with `must be a positive number or null.` (`config/normalize.rs:585-594`), and `builtin_tools.rs:239-240` then turns a negative or non-finite value into a zero timeout (`Duration::try_from_secs_f64(..).unwrap_or_default()`).

Fix: drop `Deserialize` from the nested portable types (keep `Serialize`, which `HarnessAgentConfig`'s serializer needs), or implement it on each through the normalizer's function for that type so there is one validator. If they stay as they are, say on each type that only `HarnessAgentConfig`/`define_harness_agent_config` validate.

### api-12. Fixed-length array constants over `#[non_exhaustive]` enums

`harness-rs/strands-harness/src/config.rs:65` (`pub const BUILTIN_TOOL_NAMES: [BuiltinToolName; 8]`), `config.rs:98` (`BUILTIN_PLUGIN_NAMES: [BuiltinPluginName; 2]`), `src/tools/subagent.rs:76` (`CONTEXT_MODES: [ContextMode; 3]`).

The three enums are `#[non_exhaustive]` so a variant can be added without a breaking change, but the length is part of each constant's type: a ninth built-in tool changes `[BuiltinToolName; 8]` to `[_; 9]` and breaks `let names: [BuiltinToolName; 8] = BUILTIN_TOOL_NAMES;`, array patterns and `const` contexts sized from it. (TS has `as const` tuples for the first two, `harness-ts/src/config.ts:54-65`, and `readonly ContextMode[]` for the third, `tools/subagent.ts:204`; TS's name unions are closed as well, so nothing is lost there. In Rust the enums were opened on purpose by #65 and the array types close them again.)

Fix: `pub const BUILTIN_TOOL_NAMES: &[BuiltinToolName] = &[..];` (same for the other two; `CONTEXT_MODES` goes away from the public surface under api-10). The SDK has the same shape in `SnapshotField::ALL: [Self; 5]` (`strands-rs/strands/src/agent/snapshot.rs:35,89`), so decide it once for both crates. The in-crate users take `.to_vec()` or compare with a `Vec` (`config.rs:605,629`, `config/options.rs:74,126`, `agent.rs:527`), which keep working, or call the array `.map(..)` (`config/normalize.rs:470,676`, `builtin_tools.rs:111,399`), which becomes `.iter().map(..)`.

### api-13. Cargo: two unused tokio features, and a manifest warning on every command

- `harness-rs/strands-harness/Cargo.toml:26`: `tokio = { .., features = ["rt", "sync", "time", "macros", "fs", "io-util"] }`. The library uses `tokio::{select, spawn, sync, task, time}` only (`grep -rno 'tokio::[a-z_]*' src` outside tests); `tokio::io` appears only in `src/tools/web_fetch/tests.rs:19`, which the dev-dependency's `full` covers, and every file read goes through the sandbox or `std::fs`. `fs` and `io-util` are unused.
- `Cargo.toml:17` (workspace root): `strands-harness` is declared under `[workspace.dependencies]` and no member depends on it, so every cargo command prints `warning: unused workspace dependency 'strands-harness'` (`cargo::unused_workspace_dependencies`; seen in `cargo doc`, `cargo check` and `cargo test` here with cargo 1.100).

Fix: drop `fs` and `io-util`; drop the workspace entry (or use it from a member).

### api-14. The missing `configureLogging` and the silent-by-default warnings are not documented for the harness

`harness-rs/strands-harness/src/logging.rs:1` ("`configureLogging` is replaced by `tracing` subscribers"), `harness-rs/docs/DIVERGENCES.md` (no row), `harness-rs/README.md`, `src/lib.rs:1-7`.

`configureLogging` is a public export of the TS harness (`harness-ts/src/index.ts:42`), whose default logger prints warnings to the console (`harness-ts/src/logging.ts:11-16`). Rust has no counterpart by design (`.rust-port/PROMPT.md:111`), and the harness's 13 `warn_once` sites (the Exa third-party notice, ignored `agentConfig` keys, unknown config keys, caching on a provider without it, the `otel` notice) go to `tracing::warn!`, which prints nothing unless the application installed a subscriber. The SDK records this in `strands-rs/docs/DIVERGENCES.md:194`; the harness's own divergence table, README and crate docs say nothing (`grep -n -i 'tracing\|subscriber\|logging' harness-rs/docs/DIVERGENCES.md harness-rs/README.md harness-rs/strands-harness/src/lib.rs` finds only the `otel` row). The name is the one TS barrel export with neither a Rust counterpart nor a harness divergence row or pending decision.

Fix: a `DIVERGENCES.md` row (TS `configureLogging` and console warnings by default; Rust `tracing` events, visible only with a subscriber), and one sentence in the crate docs and README telling users to install a `tracing` subscriber to see the harness's warnings.

### api-15. A `Todos` or `EnvironmentContext` passed in `plugins` under its own name does not replace the built-in (P-958 is still open); the field doc says it does

`harness-rs/strands-harness/src/agent.rs:252-255` (rustdoc of `HarnessAgentOptions::plugins`: "An `AgentSkills`, `Todos` or `EnvironmentContext` instance replaces the one the harness would add"), `agent.rs:731-746` (`select_builtin_plugins` compares `plugin.name()` with the default-configured built-in's name), `agent.rs:703-705` (`has_skills`).

TS recognises the instance by type (`harness-ts/src/agent.ts:246,256`, `instanceof`). Rust compares names, and `name` is the public, documented config field of both plugins (`plugins/todos.rs:119-120`, `plugins/environment.rs:80-81`; it is the only field of `EnvironmentContextConfig`). Evidence (`/tmp/review12_api/probe/tests/plugins.rs` and `tests/env_plugin.rs`, mock model, no network):

```
Todos::new(TodosConfig { name: Some("my:todos".into()), .. }) in `plugins`:
create_harness: Tool name "todo_write" is registered more than once (from a built-in plugin and plugins). Rename the tool you passed, or drop the built-in via builtinTools / builtinPlugins.

EnvironmentContext::new(EnvironmentContextConfig { name: Some("my:environment".into()) }) in `plugins`:
"<environment>" blocks in the first model request: baseline=1 default_named=1 renamed=2
```

The second case is silent: the model gets the environment block and `AGENTS.md` twice on every turn. `DIVERGENCES.md:50` has the row, but the rustdoc a caller reads states the TS rule.

Fix (decide P-958): the least API is `pub trait Plugin: Any + Send + Sync` in the SDK (every implementor is already `'static`, so no implementor changes and no user ever touches `Any`), and the harness tests `(plugin.as_ref() as &dyn Any).is::<Todos>()` through trait upcasting (stable since 1.86, MSRV is 1.96). That restores the TS rule for all three plugins without an `as_any` method. If the SDK trait stays as it is, correct the rustdoc of `plugins`, `TodosConfig::name` and `EnvironmentContextConfig::name` to say that only an instance with the default name replaces the built-in and that a renamed one needs `builtin_plugins` without that entry.

### api-16. Stale `expect(dead_code)` with "lands separately" reasons, dead internals behind them, and review-finding ids in test comments

- `harness-rs/strands-harness/src/defaults.rs:2-5`: file-level `#![expect(dead_code, reason = "each constant is used once the module that reads it lands; drop this when all are")]`. Every module has landed; `DEFAULT_BUILTIN_PLUGINS`, `DEFAULT_SESSION`, `DEFAULT_SKILLS` and `DEFAULT_MEMORY` (`defaults.rs:18,20,22,24`) have no reader anywhere (`grep -rnw` over `src` finds only the definitions), and the file-level attribute now hides any future dead constant.
- `src/interventions.rs:31-32`: `#[expect(dead_code, reason = "named by create_harness, which lands separately")] pub(crate) type InterventionAsk = Ask;` has no user; `create_harness` has landed.
- `src/test_support.rs:2-5`: `reason = "used once the modules that assert on warnings land; drop this when they have"`.
- `src/models.rs:128-134` (api-3) and `src/telemetry.rs:126-132` (api-4) are the same pattern.

- Eleven test doc comments name a review finding: `/// Review finding correctness-a8: ..` and the like at `src/agent.rs:819`, `src/config/tests.rs:1091`, `src/tools/web_search/tests.rs:444`, `src/tools/web_fetch/tests.rs:797`, `src/tools/programmatic_tool_caller/tests.rs:594,609,619,827,953`, `src/tools/file_tools/tests.rs:420`, and in the SDK part of the diff `strands-rs/strands/src/types/agent/tests.rs:551`. `strands-rs/docs/TESTING.md:21`: "Tests written during development carry no issue references"; the ids point into `.rust-port/`, which is removed before any PR (`.rust-port/PROMPT.md:558`).

Section 7.6: comments never narrate the change. These reasons describe a past plan, and each still holds only because the code under it has no caller.

Fix: delete the four constants and `InterventionAsk`; replace the file-level attributes by item-level ones (or none) with a reason that is true today; reword the eleven comments to say what the test asserts.

### api-17. No test uses the public API from outside the crate; the builder (the primary API, #32) has one in-crate case

`harness-rs/strands-harness/tests/` holds only `programmatic_tool_caller_mcp.rs` (imports `strands_harness::programmatic_tool_caller` and nothing else). Every `create_harness` case is in `src/agent/tests.rs` and goes through the `pub(crate)` `create_harness_with` seam with struct literals (`grep -rn 'HarnessAgentOptions {' src | wc -l` is 53); `HarnessAgentOptions::builder()` appears once (`src/config/tests.rs:992`, three setters). So nothing committed checks what #65 promises ("`..Default::default()` works from other crates"), that the crate-root re-exports are enough to write the options, or the section 13 snippet; api-1 and api-2 are both of that kind and both are invisible from inside the crate.

The scratch crate shows the rest holds: `/tmp/review12_api/probe/tests/literals.rs` (a struct literal with `..Default::default()` for each of the 18 config structs, plus `TodoItem`, `AgentSpec::new`, `HarnessModuleReference::new`) and `tests/forms.rs` (`caching(false)`, `session(false | SessionConfig | SessionManager)`, `skills(false)`, `memory(false)`, `background_tasks(false)`, `builtin_tools(Vec | BuiltinToolsConfig)`, `interventions([..])`, `model(Arc<dyn Model>)`, `HarnessError: Send + Sync + 'static`, the `create_harness` future is `Send`) compile, and `tests/runtime.rs` builds an agent on a current-thread runtime with a mock model.

`harness-rs/docs/PARITY.md:21` also still lists `harness-ts/test/internal.test.ts` as `0/2`, "Not ported yet", without a reason; its two cases are about this surface (the plumbing exists, and `resolveModel`, `resolveMemory`, `resolveInterventions`, `resolveBuiltinTools`, `EFFORT_LEVELS`, `DEFAULT_MEMORY_DIR`, `DEFAULT_SKILLS_DIR` stay off the root). The Rust names are all `pub(crate)` today (checked: every top-level `pub` item under `src/` is named in `lib.rs`), so the second case holds by construction and the row needs that reason, not "not yet".

Fix: add `harness-rs/strands-harness/tests/public_api.rs` with those compile-level cases (struct literals, each builder form, the section 13 snippet under `#[allow(dead_code)]`) and one mock-model `create_harness` run through the public function.

### api-18. `ModelFeatureDisabled` tells the user to enable a feature of the wrong crate

`harness-rs/strands-harness/src/error.rs:13-20` (`#[error("model '{spec}' requires the strands feature \"{feature}\"")]`), `src/models.rs:415-421` (`disabled`, behind `#[cfg(not(all(feature = "bedrock", feature = "anthropic", feature = "openai")))]`, the harness crate's own features).

The text is the one section 10 prescribes (`.rust-port/PROMPT.md:662`, P-831), but the gate it reports is `strands-harness`'s feature, not `strands`'s. An application that already has `strands/anthropic` on and depends on `strands-harness` with `default-features = false` gets told to enable what it has. Evidence, `/tmp/review12_api/probe2` (`strands = { default-features = false, features = ["anthropic"] }`, `strands-harness = { default-features = false }`), `cargo test --offline --test feature_message`:

```
model 'anthropic/claude-opus-5' requires the strands feature "anthropic"
```

while `strands::models::AnthropicModel::builder()` compiles in the same test. The sibling variant for the same kind of failure says it correctly: `FeatureDisabled`, `"{option} requires the strands-harness feature \"{feature}\""` (`error.rs:168-175`).

Fix: `model '<spec>' requires the strands-harness feature "<feature>"` (and the `DIVERGENCES.md:9` row), which needs the owner's sign-off because it changes a section 10 text; then fold the two variants into `FeatureDisabled { option, feature }`, since callers branch on "a cargo feature is missing", not on which option asked for it.

### api-19. `make_subagent`: the TS call shapes for `Fixed(..)` and for the builder callback need wrapping the caller has to discover

`harness-rs/strands-harness/src/tools/subagent.rs:136-150` (`axis_from!` conversions), `:233` (`AgentBuilder`), `:244` (`pub builder: AgentBuilder`).

P-892 says the conversions exist "so `Open.into()` and `Fixed(Some(..)).into()` read as in TS". The marker forms do (`.instructions(Open)`, `.tools(Inherit)`, `.model(Inherit)`, `.context(Fixed(ContextMode::All))`, `.tools(Choice::multiple(["read", "write"]))` compile). The value forms do not (`/tmp/review12_api/probe/tests/axes.rs`, `cargo check --offline --tests`):

```
.instructions(Fixed("x"))                     E0277: `InstructionsMode: From<Fixed<&str>>` is not satisfied
.tools(Fixed(vec!["read".to_owned()]))        E0277: `ToolsMode: From<Fixed<Vec<String>>>` is not satisfied
.model(Fixed("anthropic/claude-haiku-4-5"))   E0277: `ModelMode: From<Fixed<&str>>` is not satisfied
.builder(|spec: AgentSpec| async move { .. }) E0308: mismatched types
```

TS: `instructions: new Fixed('...')`, `tools: new Fixed(['read'])`, `model: new Fixed('provider/name')`, `builder: (spec) => agent` (`harness-ts/src/tools/subagent.ts:57-58,191,722-746`). In Rust the caller writes `Fixed(Some("x".to_owned()))`, `Fixed(Some(vec!["read".to_owned()]))`, `Fixed(Some(ModelSource::from("..")))` and `let builder: AgentBuilder = Arc::new(|spec| Box::pin(async move { .. }));` (the annotation is needed, as in the crate's own doc example at `subagent.rs:312-313`).

Fix: add `From<Fixed<&str>>` and `From<Fixed<String>>` for `InstructionsMode` and `ModelMode`, `From<Fixed<Vec<String>>>` (and `From<Fixed<[&str; N]>>`) for `ToolsMode`; give the `builder` member a bon closure setter (`#[builder(with = |f: impl Fn(AgentSpec) -> Fut + Send + Sync + 'static| ..)]` boxing the future) so an async closure can be passed directly, keeping the field type.

### api-20. `ProgrammaticToolCallerConfig` exposes the JSON encoding as the Rust type: `Option<Option<f64>>` seconds and `Option<Option<Vec<String>>>`

`harness-rs/strands-harness/src/config.rs:292-317` (`allowed_tools: Option<Option<Vec<String>>>`, `timeout: Option<Option<f64>>`), consumed at `src/builtin_tools.rs:237-242`.

This struct is not only the portable JSON form: it is the config a Rust caller writes in `BuiltinToolsConfig::programmatic_tool_caller` (`src/types.rs:160`). Three problems for that caller:

- `timeout` is `f64` seconds. Section 7.3: "use `Duration`, not `f64` milliseconds". The sibling `MakeProgrammaticToolCallerOptions::timeout` is `Option<Option<Duration>>` (`src/tools/programmatic_tool_caller.rs:85`), so the same knob has two types in one crate.
- A negative, NaN or infinite value is accepted by the typed path and becomes a zero timeout: `builtin_tools.rs:240`, `Duration::try_from_secs_f64(seconds).unwrap_or_default()`. Only the JSON path validates (`config/normalize.rs:585-594`, `must be a positive number or null.`). With `Duration` the invalid states do not exist.
- `allowed_tools` distinguishes absent from `null`, but the code treats them alike (`builtin_tools.rs:238`, `caller.allowed_tools.flatten()`; TS too, `harness-ts/src/builtin-tools.ts:175-177`), and the field doc says so ("absent or `null` for all of them"). The caller still has to write `Some(Some(vec![..]))`.

P-934 chose `Option<Option<T>>` "because TS keeps `null` distinct from absent"; that is a property of the wire, and only `timeout` needs it.

Fix: `allowed_tools: Option<Vec<String>>` (deserialize `null` as `None`), and `timeout: Option<Option<Duration>>` with a serde adapter that reads and writes seconds as a JSON number and rejects non-positive values, so both entry points validate alike and `builtin_tools.rs` passes the value through.

### api-21. The options builder does not take tools and plugins the way the SDK builder it is meant to follow does (#32)

`harness-rs/strands-harness/src/agent.rs:249-255` (`#[builder(default)] pub tools: Vec<ToolSource>`, `#[builder(default)] pub plugins: Vec<Arc<dyn Plugin>>`).

Decision #32: the builder is the primary API and "`HarnessAgentOptions` follows the same pattern" as `Agent::builder()`. The SDK builder converts for the caller (`strands-rs/strands/src/agent/config.rs:441-486`: `tool(impl Into<ToolSource>)`, `tools(impl IntoIterator<Item = impl Into<ToolSource>>)`, `plugin(impl Into<SharedSource<dyn Plugin>>)`); the derived harness setters take the field types as they are. Evidence (`/tmp/review12_api/probe/tests/tools_forms.rs`, `cargo check --offline --test tools_forms`):

```
strands::Agent::builder().tool(read()).tools([write(), edit()]).plugin(Todos::default())   compiles
HarnessAgentOptions::builder().tools(vec![read()])        E0308: expected `ToolSource`, found `FunctionTool`
HarnessAgentOptions::builder().tools([read()])            E0308: expected `Vec<ToolSource>`, found `[FunctionTool; 1]`
HarnessAgentOptions::builder().plugins(vec![Todos::default()])   E0308: expected `Arc<dyn Plugin>`, found `Todos`
```

What works is `.tools(vec![read().into()])` and `.plugins(vec![Arc::new(Todos::default())])`. Passing the harness's own exported tools and plugins to the harness's own builder is the first thing a caller of `tools`/`plugins` does (TS: `tools: [myTool]`, `plugins: [new Todos()]`).

Fix: `#[builder(with = |tools: impl IntoIterator<Item = impl Into<ToolSource>>| tools.into_iter().map(Into::into).collect())]` on `tools`, and the matching closure on `plugins` (`impl IntoIterator<Item = impl Into<SharedSource<dyn Plugin>>>`, `.into_arc()`), keeping the field types.

## Checked and found in order

No finding, stated so the next reader does not repeat it:

- Every name of `harness-ts/src/index.ts` has its Rust counterpart at the crate root except `configureLogging` (api-14) and `ToolConfig` (P-932, `Setting<C>`); the Rust-only root names all have a pending-decision entry (P-892, P-910, P-917, P-931, P-932, P-934) except the three of P-993 (api-10). Every top-level `pub` item under `src/` is named in `lib.rs`; no glob re-export; the `internal.ts` names are `pub(crate)`.
- `HarnessAgentOptions` has the 15 TS fields plus `agent_config`; struct-literal construction with `..Default::default()` compiles from another crate for all 18 config structs (`tests/literals.rs`); the builder's `build()` is infallible (P-930); `PassthroughOption` fires for a harness-owned key (`tests/runtime.rs`).
- All public enums and `HarnessModuleReference` are `#[non_exhaustive]`; config structs are exhaustive with a `bon` builder and `Default` (`MakeSubagentOptions` and `MakeWebFetchOptions` have a required member and so no `Default`).
- `HarnessError` and `HarnessAgentOptions` are `Send + Sync + 'static`; the `create_harness` future is `Send`; it builds on a current-thread runtime and does not panic when polled outside a runtime (`tests/forms.rs`, `tests/runtime.rs`).
- No tokio type, `Any`, `TypeId` or downcast in a public signature; `monty` and `monty-types` are harness-only dependencies; every dependency in `Cargo.toml` is used; the five provider features forward to `strands`; `publish = false`.
- `cargo doc -p strands-harness --no-deps` with `RUSTDOCFLAGS="-D warnings"` (all features, and no default features), `cargo test -p strands-harness --doc` (all features, and no default features: 5 doctests pass), `cargo check -p strands-harness --no-default-features` (and with `otel` alone), `cargo clippy -p strands-harness -- -D warnings` (all features, and no default features): all clean. `cargo test -p strands-harness --all-features --lib`: 493 passed.
- `HarnessAgentConfig` round-trips through its own `Serialize`/`Deserialize` and equals `DEFAULT_HARNESS_AGENT_CONFIG` for `{}` (`tests/roundtrip.rs`).

## Not reviewed

- Behaviour, as opposed to surface, of `src/agent.rs:345-833` (read once), `src/builtin_tools.rs` (read in part), `src/config/normalize.rs` and `src/config/options.rs` (only the effort, timeout, `agentConfig` and MCP parts), `src/memory.rs`, `src/models.rs` beyond effort parsing, the feature gates and `supports_*`, `src/models/build.rs`, `src/models/web_fetch.rs`, `src/prompt.rs` text, `src/sandbox.rs`.
- `src/plugins/environment.rs` and `src/plugins/todos.rs` beyond their public types; `src/tools/file_tools.rs`, `shell.rs`, `web_fetch.rs`, `web_search.rs` beyond their public signatures; `src/tools/subagent/context.rs`, `src/tools/subagent/resolve.rs`; `src/tools/programmatic_tool_caller/run.rs`, `output.rs`.
- Every `tests.rs` file and `tests/programmatic_tool_caller_mcp.rs`; `harness-rs/docs/PARITY.md` counts and `docs/test-title-map.json`.
- SDK: `strands-rs/strands/src/tools/executors/executor.rs` `call_tool` was read, not tested; `types/agent/tests.rs` not read beyond the case names.
- Not run: `tests/programmatic_tool_caller_mcp.rs`, clippy with `--all-targets`, a build on the MSRV (the toolchain here is rustc 1.100, the MSRV is 1.96), `cargo deny`, any live test.

## Verdict

Not ready to pass the gate: api-1 (the section 13 snippet does not compile), api-2 (builders unnameable and undocumented), api-4 and api-15 (two open decisions with user-visible effects), api-7 (a secret in `Debug`) and api-18 (an error text that names the wrong crate's feature) need a fix or an owner decision first; the others are cheap to change before a release and breaking after it.
