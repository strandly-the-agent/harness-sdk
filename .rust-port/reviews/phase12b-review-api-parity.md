# Phase 12 verification pass: API bar-raiser and parity validator for the fixes

Commit under review: `6dc02c342` (worktree `/mnt/workspace/review-h12v`), fix diff `c2c6d557d..HEAD`.

Evidence crates of this pass (`/tmp/review12b_api/surface`, `/tmp/review12b_api/marker`) are copied to `/mnt/workspace/reviews/phase12b-review-api-parity-probe/` with their output. Nothing in the worktree was changed; no live test, no AWS call.

## Findings

### api-b-1. api-9 is not closed for `HarnessError::InvalidEffort`: `supported` is still a comma-joined `String`

`harness-rs/strands-harness/src/error.rs:47-53` (`InvalidEffort { effort: String, supported: String }`), built at `harness-rs/strands-harness/src/types.rs:80-83` (`supported: EFFORT_LEVELS.map(Self::as_str).join(", ")`).

The api-9 fix (`14c4b2364`) turned the two sibling lists into `Vec<String>` joined in the `#[error]` attribute (`UnknownModelProvider.supported`, `error.rs:44`; `UnknownAgentType.valid`, `error.rs:97`) and left this one, with the disposition "the brief gives `parse_effort`/`InvalidEffort` to track h12ga". The h12ga fix for api-3 (`57dc5aae8`) then rewrote the only construction site (`FromStr for Effort`) and kept the `String`; its disposition row does not mention the field. P-1166 records it as "open for the owner: ... it should become `Vec<String>` or go, the same way". So at `6dc02c342` the variant that api-3 made reachable through a public parser has the one field of the three that api-9 called out which was not changed, and `HarnessError` now has `supported: Vec<String>` on one variant and `supported: String` on the next.

Evidence: `grep -n 'supported' harness-rs/strands-harness/src/error.rs` (`:44 supported: Vec<String>`, `:52 supported: String`).

Fix: `supported: Vec<String>` (or `&'static [Effort]`) and `#[error("Effort {} is not valid. Supported: {}.", json_quote(.effort), .supported.join(", "))]`; the text and `models::tests::rejects_an_effort_outside_the_enum_by_name` stay as they are. Close the open sentence of P-1166.

### api-b-2. The crate rustdoc and `AGENTS.md` still say every module is private; neither names `builders`; the `AGENTS.md` sentence on the MCP cases is the pre-fix behaviour

- `harness-rs/strands-harness/src/lib.rs:86-87`: "The crate root re-exports every public name, as the TypeScript package's entry point does; the modules themselves are private." `lib.rs:93` is `pub mod builders;` (api-2, P-1154), and the twenty builders and their state modules are public names that the root does not re-export. The crate docs do not mention the module anywhere else (`grep -n builders src/lib.rs` finds only the `pub mod` line), so a reader of the crate page is told the opposite of what the sidebar shows. The h12ga disposition row for api-2 lists this as "left for the owner of the text"; nothing after it changed the sentence.
- `harness-rs/AGENTS.md:16`: `lib.rs  # crate docs and the flat re-exports; every module is private`, and the layout block (`:11-31`) has no `builders.rs` (the api-6 fix rewrote this block to "match the tree"; `ls src` has `builders.rs`).
- `harness-rs/AGENTS.md:59`: "set `STRANDS_TEST_NODE` to the node binary and `STRANDS_TEST_NODE_MODULES` to a `node_modules` ...; without them the cases skip with a message". After the parity-12 fix (`e556f8f60`) the file's own header says the reverse (`tests/programmatic_tool_caller_mcp.rs:5-8`: `node` from `PATH` and the repository root's `node_modules` by default, the two variables only override), and that is what `node_binary`/`node_modules` do (`:33-55`).

Fix: `lib.rs`: "The crate root re-exports every public name of the TypeScript package's entry point; the builders of the option and config types are in [`builders`]; every other module is private." `AGENTS.md:16` likewise, plus a `builders.rs` line in the layout; `AGENTS.md:59`: the cases run when `node` is on `PATH` and the repository root has `node_modules` (`npm ci`), the two variables override, and otherwise each case prints `SKIPPED` and passes.

### parity-b-1. `PARITY.md` still says the effort case is ported against `parse_effort`, which the api-3 fix removed

`harness-rs/docs/PARITY.md:24` (the `models.test.ts` row): "`rejects an effort outside the enum by name` is ported against `parse_effort` (P-833)".

The api-3/parity-3 fix (`57dc5aae8`) deleted `parse_effort` (`grep -rn parse_effort harness-rs/strands-harness/src` is empty) and the mapped test now calls the public parser (`harness-rs/strands-harness/src/models/tests.rs:77-82`: `"bogus".parse::<Effort>()`). The h12ga disposition says "No PARITY row change (same test name, same count)", which is right for the count and misses the sentence. This is the parity-3 defect again in the other file: the first pass reported that `DIVERGENCES.md` named a function users cannot reach; that row was corrected (`DIVERGENCES.md:13`) and this one now names a function that does not exist.

Fix: "... is ported against `Effort`'s `FromStr` (`"x".parse::<Effort>()`)".

### api-b-3. Divergence rows written by the fixes cite the internal `PROMPT.md` by section

`harness-rs/docs/DIVERGENCES.md:41` ("... rather than a silent drop (PROMPT.md section 11 item 37)", row rewritten by the parity-7 fix), `:57` ("the name is out of scope by PROMPT.md section 5 (...)", the Logging row of api-14/parity-9), `:58` ("(section 7.3, P-1153)", the `allowedTools: null` row of api-20).

`PROMPT.md` lives only under `.rust-port/`, which is removed before a PR, so a reader of the published file cannot follow "PROMPT.md section 5" or a bare "section 7.3". `git blame` puts the three on fix commits of this pass (`5c44c8323`, `a7589195b`, `ad16e26a2`). One older harness row has the same form (`:25`, "PROMPT.md decision #62"), and `strands-rs/docs/DIVERGENCES.md` has five older hits (`grep -c 'PROMPT.md\|(section [0-9]'`), so settle the wording once for both files; the rows this pass added should not grow the list.

Fix: state the reason itself: `:41` "a key that cannot take effect is an error rather than a silent drop"; `:57` end the reason at "... as in the SDK (`strands-rs/docs/DIVERGENCES.md`, Logging)"; `:58` "the Rust type does not carry the JSON encoding (P-1153)".

### api-b-4. The api-19 setter refuses the crate's own `AgentBuilder`: every `.builder(b)` call that compiled before the fix fails, and the disposition row does not say so

`harness-rs/strands-harness/src/tools/subagent.rs:263` (`#[builder(setters(vis = "", name = builder_internal))] pub builder: AgentBuilder`), `:298-313` (the public `builder<F, Fut>(self, build: F)` with `F: Fn(AgentSpec) -> Fut`).

`AgentBuilder` is a crate-root export and the type of the field the setter fills, but a value of it (`Arc<dyn Fn(AgentSpec) -> BoxFuture<..> + Send + Sync>`) is not `Fn`, so the setter that took it at `c2c6d557d` now rejects it. The reviewer probe was written against that shape: in `phase12-review-api-bar-raiser-probe/probe/tests/axes.rs` the five lines of `fn ok(b: AgentBuilder)` (`:4-8`, the "marker forms" the first pass reported as compiling) and `tests/names.rs:10` all fail at `6dc02c342`:

```
error[E0277]: expected an `Fn(strands_harness::AgentSpec)` closure, found `Arc<dyn Fn(AgentSpec) -> Pin<Box<_>> + Send + Sync>`
   --> tests/axes.rs:4:52
  4 |     let _ = MakeSubagentOptions::builder().builder(b.clone()).instructions(Open).tools(Inherit)...
```

(`cargo check --offline --test axes` in the copy under `/tmp/review12b_api/probes/probe`: 10 errors, 9 of them this one; `--test names`: 3 errors, where the h12ga rerun lists 2.) The api-19 finding asked for a closure setter "keeping the field type" so that the TS shape `builder: (spec) => agent` works; it did not ask to drop the form that worked. The disposition row says "the field type stays `AgentBuilder`" and nothing about the setter no longer taking one; only P-1167 and the setter's rustdoc do ("goes in as `move |spec| builder(spec)`", which double-boxes the future). A caller who stores the builder in a variable of the exported type, reuses `options.builder.clone()` of another `MakeSubagentOptions`, or follows the first-pass doc example gets the error above, whose text does not point at the workaround.

Fix: this needs the owner's choice, because the obvious repair has a cost. One setter over a sealed conversion trait with a marker parameter (`builder<M>(self, build: impl IntoAgentBuilder<M>)`, one impl for `F: Fn(AgentSpec) -> Fut`, one for `AgentBuilder`) does take both, but I tried it (`/tmp/review12b_api/marker/m.rs`, plain `rustc`): the closure parameter is then no longer inferred (`|spec| async move { spec.task }` is `E0282: type annotations needed`; `|spec: AgentSpec|` compiles), which gives back part of what api-19 asked for. So either (a) keep the closure-only setter, say in the api-19 disposition row that `.builder(AgentBuilder)` stopped compiling and why, and add the wrap form (`move |spec| held(spec)`) to `tests/public_api.rs` (the `make_subagent` doctest, `subagent.rs:354`, covers the closure form from outside the crate; nothing compiles the wrap form the rustdoc recommends, and `public_api.rs` has no `make_subagent` case; my scratch test `/tmp/review12b_api/surface/tests/surface.rs::subagent_shapes` shows it compiles); or (b) add a second setter for a held `AgentBuilder` under a logged name. The `E0277` text does not lead a caller to the workaround, so (a) should also put the one-line wrap in the rustdoc of the `AgentBuilder` alias (`subagent.rs:252`), where a caller holding one looks.

### api-b-5. `shutdown_telemetry` returns an error when a harness agent was built between two calls; the disposition says a second call does nothing

`harness-rs/strands-harness/src/telemetry.rs:148-161` (`shutdown_telemetry` takes the provider out of `TELEMETRY`), `:72-86` (`Telemetry::setup` runs `setup_tracer` again whenever the slot is `None`), `strands-rs/strands/src/telemetry/config/setup.rs:292-296` (the SDK returns its stored provider, already shut down, with the warning `tracer provider already initialized, returning existing provider`).

After the first `shutdown_telemetry()` the harness slot is empty, so the next `create_harness` in the process (a second agent, which is what the test below builds; by reading, also every delegated `subagent` call, whose default builder is `create_harness_with`, `agent.rs:541`, which calls `setup_telemetry`, `agent.rs:574`) asks the SDK for the tracer again, gets the dead provider back and stores it. The next `shutdown_telemetry()` then calls `shutdown()` on it a second time and fails. Evidence, scratch crate `/tmp/review12b_api/surface/tests/telemetry_twice.rs` (mock model, no network), `OTEL_TRACES_EXPORTER=console cargo test --offline --test telemetry_twice -- --nocapture`:

```
FIRST SHUTDOWN: Ok(())
REPEAT SHUTDOWN: Ok(())          <- no agent built in between: the disposition's "a second call does nothing"
SECOND SHUTDOWN: Err("Shutdown already invoked")   <- create_harness ran in between
```

The correctness-8 disposition and the rustdoc ("Does nothing when no exporter was wired", "Agents that run after it export no spans", `telemetry.rs:142-143`) describe the first two lines only. An application that flushes after each job and again at exit, or a test suite that calls it per test, gets an `Err` from a call that had nothing to do. It also breaks the module's own rule ("setup runs at most once per process", `telemetry.rs:9-10`): `setup_tracer` runs again after a shutdown.

Fix: remember that telemetry was shut down instead of returning to the unset state (for example `enum State<P> { Unset, Active(P), Shutdown }` in `Telemetry`): `setup` returns early for `Active` and `Shutdown`, and `shutdown_telemetry` is a no-op for `Unset` and `Shutdown`. Add the three-step sequence above to `telemetry::tests` (the in-memory exporter test already builds a provider).

## First-pass findings: fix checked against the finding's own evidence

Probes: `.rust-port/reviews/phase12-review-api-bar-raiser-probe/` copied to `/tmp/review12b_api/probes/`, path dependencies pointed at `/mnt/workspace/review-h12v`; `cargo check --offline --test <name>` for each target, `cargo test --offline --test <name>` for the run-time ones. My own outside-crate checks of the changed surface are in `/tmp/review12b_api/surface/tests/surface.rs` (5 cases, all pass).

| finding | disposition found | state at `6dc02c342` |
|---|---|---|
| api-1 | h12ga | Closed. Probe `dx` (the section 13 snippet) and `dx_with_path_variant` compile; `From<&str>`, `From<String>`, `From<&String>`, `From<JsonObject>` for `McpServersOption`, `From<&str>`/`From<String>`/`From<SkillSource>` for `SkillsOption`, array and slice for `BuiltinToolsOption` compile from outside (`surface.rs`). |
| api-2 | h12ga, P-1154 | Closed in code: `cargo doc` writes 20 `builders/struct.*Builder.html`, `struct.HarnessAgentOptions.html` links to `builders/struct.HarnessAgentOptionsBuilder.html`, the private `builder_internal` setter is not rendered. Probe `names` still has the root-path error the row explains. The text the row left "for the owner" is still wrong: api-b-2. |
| api-3, parity-3 | h12ga | Closed in code (`as_str`, `Display`, `FromStr`, TS text checked from outside). Two leftovers: api-b-1 (the `supported` field) and parity-b-1 (`PARITY.md:24`). |
| api-4, parity-2 | h12fb (as correctness-8), P-1130 | Closed for the reported case; `shutdown_telemetry` is public with and without `otel`, documented, with a divergence row (`DIVERGENCES.md:55`). New defect: api-b-5. |
| api-5 | h12gb, P-1165 | Closed: one method, `call_tool(name, id_prefix, input, invocation_state)`; no reference to the removed name is left in `harness-rs` or `strands-rs`. |
| api-6, parity-10 | h12gc | Closed: doctests on `create_harness`, `HarnessAgentOptions`, the crate docs; README sections; all 54 rows of `DIVERGENCES.md` have four cells. Leftovers in api-b-2. Probe `forms2` still fails as in the first pass (`.context_manager("agentic")`, `.context_manager(false)`); the row answers with documentation, which is what the finding asked for. |
| api-7 | h12fa (as correctness-4) | Closed: probe `debug_secret` passes. |
| api-8 | h12gb, P-1171 | Closed. Probe `literals` no longer compiles (`tests/literals.rs:24`, `E0639` on the `TodoItem` struct literal), which is the intended effect; the h12ga rerun note that lists `literals` as compiling predates this commit. |
| api-9 | h12gb, P-1166 | Not closed for `InvalidEffort`: api-b-1. The other three variants are as the row says. |
| api-10 | `phase12a-dispositions.md` (as parity-a6), P-1110 | Closed: the three are `pub(crate)`, the `make_subagent` doctest builds its own `Preset`. |
| api-11 | h12ga, P-1152 | Closed: `from_value::<ProgrammaticToolCallerConfig>({"timeout": 0})` is the normalizer's `must be a positive number or null.` (`surface.rs`). |
| api-12 | h12gb, P-1168 | Closed for the two public constants. |
| api-13 | h12gb, P-1169 | Closed: no warning in any gate output; `cargo package --list` passes for the three crates. |
| api-14, parity-9 | h12gc | Closed (`DIVERGENCES.md:57`, README, crate docs, `tests/logging.rs`). The row's reason cell cites `PROMPT.md`: api-b-3. |
| api-15 | h12fc (as correctness-3), P-1142 | Closed: probes `plugins` (2 cases) and `env_plugin` pass. |
| api-16, parity-4 | h12gb, P-1170 | Closed: no `expect(dead_code)`, no "lands separately", no finding id left under `harness-rs/` or `strands-rs/strands/src`. |
| api-17 | h12gc | Closed: `tests/public_api.rs`, 11 cases, in the gate. |
| api-18 | h12gb, P-1172 | Declined; the reason holds (`.rust-port/PROMPT.md:662` prescribes the text) and the rustdoc now says which crate's feature to enable. Probe `probe2/feature_message` still fails, as the row implies. Open for the owner. |
| api-19 | h12gb, P-1167 | The value forms and the closure compile from outside (`surface.rs::subagent_shapes`). The setter no longer takes an `AgentBuilder`: api-b-4 (probe `axes`: 10 errors, 9 of them this; the tenth, `ContextAxis: From<ContextMode>`, was never a finding). |
| api-20 | h12ga, P-1153 | Closed: `{"allowedTools":["read"],"timeout":1.5}` round-trips, `timeout: Some(None)` writes `null`; divergence row `:58` is true of the code. Probe `roundtrip` does not compile, by design, as the row says. |
| api-21 | h12ga, P-1150, P-1151 | Closed: `.tool`, `.tools`, `.plugin`, `.plugins` take the SDK builder's forms and extend, in any order with the other setters (`surface.rs`). Probe `tools_forms`: one error, the ambiguous `.tools(vec![read().into()])`, as the row says. |
| parity-1 | h12gd, P-1196 | Not rechecked beyond the row and the gate (the named tests pass). |
| parity-5 | h12gd, P-1197 | Closed: the four cases assert the resolved policy (`always`, `never`, `agentic`, `wait_for_completion`) and then the built agent's behaviour for `subagent`, the consumer tool and `read`; the TS cases compare the policy map of the built agent (`harness-ts/test/agent.test.ts:197-235`). |
| parity-6 | h12gd | Closed: `provider_models.rs:190-208` asserts `options.web_search` and the Anthropic native tool, the counterpart of `builtInTools: [{ googleSearch: {} }]` (`agent.test.ts:451-455`). |
| parity-7 | h12fa (as correctness-2), P-1120 | Closed: the five keys are applied (`config/options.rs`), with tests (`config/tests.rs:1348-1351`, `:1518-1530`) and the row `DIVERGENCES.md:41` (reason cell: api-b-3). |
| parity-8 | h12gd | Closed: builds the harness with both, asserts one `skills` tool and the supplied plugin's skill only (`agent/tests.rs:928-960`; TS `agent.test.ts:745-751`). |
| parity-11 | h12gd | Closed: renamed, declared gap, row 28/32; the title checker agrees. |
| parity-12 | h12gd, P-1198 | Closed for the gate: the six cases ran (0.69 s, no `SKIPPED` line). `AGENTS.md:59` still describes the old rule (api-b-2). |
| parity-13 | h12gd, P-1195 | Closed: the harness tests assert the TS results of the first-pass table (`Edited <path>.`, the numbered lines, `1 todos remaining`), and the SDK row (`strands-rs/docs/DIVERGENCES.md:206`) lists what the strip does not reach. |
| parity-14 | h12gd, P-1200 | Closed: the test asserts the Node text for `user:pw@`, `user@`, `:pw@`, that the server saw no request, and that `@` alone is fetched. |

## Documentation checks

- `NODE_PATH=/mnt/workspace/harness-sdk/node_modules node strands-rs/docs/check-test-titles.mjs --parity harness-rs/docs/PARITY.md`: exit 0, no `FAIL`, every row count equals the checker's (20 spec rows; `(root) 285/7/25/0`, `plugins 19/0/1/0`, `tools 145/0/3/0`).
- `harness-rs/docs/DIVERGENCES.md`: 54 rows, four cells each. The rows the fixes added or rewrote (`:13`, `:41`, `:46`, `:48-58`) were read against the code and are true of it; `:53` ("JS `trim`") states no difference.
- `harness-rs/docs/reviews/live.md`: the 17 `live_*` functions under `harness-rs/strands-harness/tests` are all named in it and it names no other.
- No track name, `.rust-port` path or host path under `harness-rs/` or in `strands-rs/docs/DIVERGENCES.md` (the `PROMPT.md` citations are api-b-3).

## Gate results

`.rust-port/gates.sh full` at `6dc02c342` (`GATE_OUT=/tmp/review12b_api/gates`, `CARGO_TARGET_DIR=/mnt/workspace/.cache/ct-review12b-api`), every step 0:

```
fmt: 0
clippy: 0
clippy_nodef: 0
test_all: 0
test_nodef: 0
doc: 0
msrv: 0
feat_bedrock: 0
feat_anthropic: 0
feat_openai: 0
feat_mcp: 0
feat_otel: 0
deny: 0
conformance: 0
tests all-features: passed=5241 failed=0 ignored=84
```

`test_nodef`: 3825 passed, 0 failed. No compiler or cargo warning in any gate output. The six `programmatic_tool_caller over MCP` cases ran (6 passed in 0.69 s, no `SKIPPED` line). The worktree is unchanged after the run (`git status --short`: only the two expected untracked symlinks).

`cargo package --list -p <crate> --allow-dirty --locked`: exit 0 for `strands` (426 files), `strands-macros` (78) and `strands-harness` (83), no warning.

## Not covered

- The SDK fixes of the correctness tracks (`agent/run.rs`, `background_tasks/background_tasks.rs`, `mcp/agent_tools.rs`, `mcp/client.rs`, `mcp/config.rs`) and the harness fixes for correctness-1 to correctness-9 beyond what the table above cites; they belong to the other reviewer of this pass.
- parity-1: the rewritten warning tests were not read; only the disposition row and the gate result.
- `ToolAgent::call_tool`: signature, rustdoc and the harness call site were read; no outside-crate call was written.
- The `docsrs` rendering ("Available on crate feature `mcp`") was not rebuilt with `--cfg docsrs`.
- The README code blocks are not compiled by anything; the first two are the same text as the `create_harness` and `HarnessAgentOptions` doctests, the telemetry one matches the `shutdown_telemetry` doctest, the `tracing_subscriber` line was read only.
- No live test, no AWS call, nothing with `--ignored`.

## Verdict

The fixes close the first-pass API and parity findings and every gate is green; before the gate is ticked, api-b-5 (an `Err` from `shutdown_telemetry` after a later build) needs a code fix, api-b-1 is a field type that is cheap now and breaking later, api-b-4 needs the owner's choice recorded, and api-b-2, api-b-3 and parity-b-1 are text corrections.
