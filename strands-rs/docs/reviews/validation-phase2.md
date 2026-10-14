# Phase 2 validation — TS→Rust port (SDK modules + harness)

Validator: independent, read-only on source. Repo `/mnt/workspace/harness-sdk`, branch `agent-tasks/strands-rs`,
HEAD `2ffd55c7d`. Workspace `strands-rs/`. One scratch test was added to dump tool specs and deleted
afterwards; `git status --short` is clean.

## 1. Gates (verbatim, from `strands-rs/`)

```
=== fmt ===            cargo fmt --check
fmt_exit=0             (no output)
=== clippy ===         cargo clippy --all-targets --all-features -- -D warnings | tail -1
    Finished `dev` profile [unoptimized] target(s) in 46.83s
=== test ===           cargo test --all-features | grep -E "^test result|FAILED|panicked"
test result: ok. 748 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 15.20s
test result: ok. 33 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.21s
test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.15s
test result: ok. 0 passed; 0 failed; 23 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 173 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.14s
test result: ok. 0 passed; 0 failed; 6 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 1.71s
test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
=== no-default ===     cargo build --no-default-features -p strands-agents | tail -1
    Finished `dev` profile [unoptimized] target(s) in 13.21s
=== examples ===       cargo build --examples --all-features | tail -1
    Finished `dev` profile [unoptimized] target(s) in 3.74s
=== doc warnings ===   cargo doc --no-deps --all-features | grep -cE "^warning"
0
```

All seven gates green: 994 passed / 0 failed / 32 ignored (ignored = the live-credential integ suites:
23 bedrock + 6 harness + 2 + 1), 0 doc warnings.

### Test counts

Per crate (`cargo test -p <crate> -- --list | grep -c ": test"`):

| crate | tests |
|---|---|
| `strands-agents` | 844 |
| `strands-agents-macros` | 1 |
| `strands-harness` | 181 |

Per top-level module (`strands-agents` + `strands-harness`, `--list` prefix):
`tools 186`, `strands-agents::*` loop/model/session/etc. per phase-1 report, and for the harness crate:
`tools 63`, `models 54`, `builtin_tools 24`, `plugins 15`, `agent 11`, `prompt 4`, `interventions 2`,
plus 6 integ (`#[ignore]`-gated) and 1 prompt-parity integration test.

## 2. Matrix audit

Matrices present: `shell-tool-matrix`, `skills-matrix`, `background-tasks-matrix`, `memory-matrix`,
`harness-tools-matrix` (plus phase-1 `session/bedrock/executor/...`). `storage.md` and
`harness-subagent.md` carry an inline matrix. **No matrix at all**: `context-manager.md`,
`harness-models.md`, `harness-plugins.md`, `harness-agent.md`.

Sampled rows (6 per matrix, random), each resolved to a real Rust `fn`:

| matrix | sampled | resolved | wrong/missing |
|---|---|---|---|
| shell-tool | 6 | 6 | none |
| skills | 6 | 6 | none |
| background-tasks | 6 | 6 | none |
| memory | 6 | 6 | none |
| harness-tools | 6 | 6 | none |

Spot-checked intent on `exposes_only_command_and_timeout_with_no_mode_discriminator`,
`read_reports_an_offset_past_the_end`, `tools_use_the_ts_names_and_schemas`,
`force_fails_non_terminal_tasks_on_restore`, `rejects_a_non_http_scheme` — all assert the TS
behaviour named in the row. Rows marked `skipped` are honestly labelled with a reason (e.g.
background-tasks interrupts: `InterruptError` absent from the Rust SDK).

Modules without a matrix — TS `it(...)` count vs Rust test fns:

| module | TS tests | Rust tests | delta |
|---|---|---|---|
| harness-agent (`agent.test.ts` 96 + `agent-generalist-memory` 3 + `config` 26 + `internal` 2) | 127 | `agent::*` 11 + `builtin_tools::*` 24 + 6 integ | ~86 unported |
| harness-models (`models.test.ts` 93 + `telemetry.test.ts` 9) | 102 | `models::*` 54 | ~48 unported |
| harness-plugins (`plugins/todos` 9 + `plugins/environment` 7) | 16 | `plugins::*` 15 | ~1 |
| harness-subagent (`tools/subagent.test.ts`) | 50 | subset of `tools::*` 63 (inline matrix in report) | roughly at parity |
| context-manager (`strands-ts/src/context-manager/__tests__` ×6 files) | see `cm4` | `context_manager` 21 | large gap, undocumented |

The 11 ported `agent::` tests are: `builds_an_agent_with_the_harness_defaults`,
`runs_without_session_memory_or_skills`, `pins_the_builtin_tools_named_in_a_list`,
`edits_the_defaults_with_a_mapping`, `forces_subagent_into_the_always_axis`,
`persists_the_session_under_the_configured_dir`, `sanitizes_a_session_id`,
`rejects_a_blank_session_id`, `rejects_a_consumer_tool_that_collides_with_a_builtin`,
`registers_the_memory_search_tool_when_memory_is_on`, `rejects_mcp_servers`. `harness-agent.md` is
titled "pass A" but contains **no** coverage/not-ported section, so the ~86-test gap is undeclared —
the single biggest documentation finding in this phase.

## 3. Default-behaviour parity (`createHarness` vs `create_harness`)

Source of truth: `survey2/h1-agent-defaults.md`; Rust `crates/strands-harness/src/agent.rs`,
`defaults.rs`.

| TS default | Rust | verdict |
|---|---|---|
| `DEFAULT_MODEL = 'bedrock/global.anthropic.claude-opus-5'` | `defaults.rs:4` same string | MATCH |
| `effort = 'auto'` | `Effort::Auto` is `#[default]` (`models.rs:91`) | MATCH |
| `caching = 'auto'` (on); explicit-vs-unset matters | `caching_on = !matches!(Some(Off))`, `caching_explicit = options.caching.is_some()` → `resolve_model` | MATCH |
| `systemPrompt = buildSystemPrompt(instructions)` | `build_system_prompt(instructions, &[])`; byte-parity test vs `prompt.ts` | MATCH |
| `contextManager = 'auto'` | `ContextManagerOption::Auto` is `#[default]`, forwarded verbatim | MATCH |
| `session = true`, dir `./.agent/sessions`, id = `randomUUID().slice(0,8)`, `saveLatestOn: 'message'`, `storage = LocalFileStorage(join(dir,id))` | `resolve_session`: same dir, `Uuid::new_v4().simple()[..8]`, `SaveLatestStrategy::Message`, `LocalFileStorage(dir/id)` | MATCH |
| `session.id` sanitize `[a-z0-9_-]` → `'default'` fallback; blank → throw `'session.id must be a non-empty string.'` | `sanitize_session_id` + same verbatim error | MATCH |
| `skills = true` → only if `isDir('./.agent/skills')`; explicit sources not existence-filtered | `skills_plugin` identical, incl. consumer-`AgentSkills`-wins | MATCH |
| `memory = true`, dir `./.agent/memory`, default file store + model extractor | `resolve_memory_option` → `resolve_memory{stores,model,dir}` | MATCH |
| …and TS passes `webFetch` config into `resolveMemory` | Rust computes `web_fetch_config` then `let _ = web_fetch;`, passes `web_fetch_model: None` (`agent.rs:~500`) | **DIFF** — a `web_fetch` builtin config does not reach the memory extractor |
| builtin tools & order: `shell, read, write, edit, web_fetch, web_search, programmatic_tool_caller, subagent` (web_search only when Exa; ptc tool present) | observed `agent.tools()`: `shell, read, write, edit, web_fetch, subagent` + plugin/SDK tools. web_search correctly absent (not Exa) | **DIFF** — `programmatic_tool_caller` is not implemented; `select_builtin_tools` drops it with a one-time warning (documented) |
| builtin plugins `['todos','environment']` | `DEFAULT_BUILTIN_PLUGINS` same; `todo_write` present, environment context injected | MATCH |
| background tasks `{ agentic: ['*'] }`, `subagent` forced into `always` and omitted elsewhere | `resolve_background_tasks` mirrors the TS 5 steps incl. `'*'`-keeping `omit`; test `forces_subagent_into_the_always_axis` | MATCH |
| `interventions` default none | `resolve_interventions(None)` → `[]`; any named preset is a hard error (`rejects_a_requested_preset`) | MATCH on default; explicit presets rejected loudly (not silently) — acceptable, but note the resolved vec is bound to `_interventions` and never wired to the agent |
| `await setupTelemetry()` on every `createHarness` | no equivalent call anywhere in `strands-harness` (`telemetry` only appears in `strands-agents/src/tools/executor.rs`) | **DIFF** — undocumented in the harness reports; TS `telemetry.test.ts` (9 tests) unported |
| `mcpServers` → `McpClient.loadServers(..., continueOnError)` | non-empty `mcp_servers` → `Error::other("MCP not supported yet")` | DIFF by design (default is empty → no behavioural difference), declared in the doc comment |
| subagent delegate config: `session: false`, memory instance → `memory: false`, `builtinTools = {'*':true, ...resolved}` | `parent` clone with `session: Off`, `memory: Off` when `Manager(_)`, forwarded mapping rebuilt per child | MATCH (TS web_search-forwarding nuance for a child on a non-search model is simplified; low risk) |
| `agent.addHook(InitializedEvent, dropSandboxTools)` when `sandbox` is set | no sandbox concept in Rust (shell runs on the host) | DIFF by design — see §5 |

### Tool specs the model sees

Dumped via a scratch test (`MockMessageModel` + `ModelOption::Instance`, session/memory/skills off),
then diffed against `harness-ts/src/tools/*.ts`, `strands-ts/src/vended-tools/shell/*`,
`harness-ts/src/plugins/todos.ts`.

Names in order: `shell, read, write, edit, web_fetch, subagent, todo_write, retrieve_context,
strands_manage_background_task`.

Descriptions **byte-identical to TS** for: `shell` (`SANDBOX_SHELL_DESCRIPTION`), `read`
(`READ_DESCRIPTION`), `write`, `edit`, `web_fetch`, `todo_write`, and `subagent`
(base + `describeRoles` block → `- generalist: a general-purpose agent for a focused subtask that
runs in its own context`). No description differences found.

Schema differences:

1. **`subagent` is missing the `context` and `last_messages` properties.** TS
   `buildDefaultSubagent` passes `context: new Choice([...CONTEXT_MODES])`
   (`survey2/h4-subagent-ptc-plugins.md:125`), so the TS model sees
   `context: 'none'|'all'|'no_tools'` plus `last_messages`. The Rust spec has only
   `task, agent_type, instructions, tools`. Model-visible capability loss (no history sharing).
2. **`subagent.tools` enum is narrower**: Rust `["shell","read","write","edit","web_fetch"]`; TS
   inherits *all* enabled builtin names except `web_search` — i.e. also
   `programmatic_tool_caller` and `subagent`. `ptc` is explained by (1) above; `subagent` being
   un-delegatable is an unflagged narrowing.
3. **Integer-typed numbers are emitted as `"type": "number"`** where TS zod emits `"integer"`:
   `shell.timeout`, `read.offset`, `read.limit`. (TS: `z.number().int()` → `integer`.) Cosmetic for
   most providers, but it is a wire difference in the tool spec.
4. `subagent`'s schema has no `"additionalProperties": false` while the file/shell tools do — this
   matches TS (`buildSchema` returns a bare `{type,properties,required}`), so **not** a diff.

Everything else (property names, order, descriptions, `required`, `minLength`/`minimum`
constraints) matches.

## 4. Live evidence

`strands-rs/docs/reviews/integ-harness-run.log` (14 lines) and `integ-bedrock-run.log` (30 lines) are
genuine `cargo test` output: `Compiling strands-harness v0.1.0 (/mnt/workspace/harness-sdk/strands-rs/...)`,
`Finished \`test\` profile`, `Running tests/integ_harness.rs (/home/sandbox/cargo-target/debug/deps/integ_harness-96e26b623d0c0b30)`
— per-test `... ok` lines with a hashed binary path and real timings, not hand-written text.

- harness: 6 tests / 6 passed in 14.24s — `persists_a_session_with_a_minted_id`,
  `persists_sessions_across_agents`, `persists_memory_across_agents`, `uses_the_shell_tool`,
  `uses_the_file_tools_in_one_turn`, `uses_the_todo_write_tool_from_the_todos_plugin`.
- bedrock: 23 tests / 23 passed in 16.06s — caching (system/message/tool cache points, explicit
  anthropic strategy), guardrail redaction ×3, token counting, reasoning content, sliding-window
  history, max-tokens behaviour, metadata events, invalid-model-id handling.

Both match the `#[ignore]`-gated counts seen in the unit-test gate (23 + 6 ignored), so the logs
correspond to the suites in the tree.

## 5. Sensitive surface

`rg "std::env|env::var|tokio::fs|std::fs|Command::new|reqwest|TcpStream"` (non-test):
`session/file_storage.rs 21`, `tools/file_tools.rs 17`, `storage/local_file_storage.rs 12`,
`harness/models.rs 10`, `tools/web_search.rs 6`, `plugins/environment.rs 5`,
`tools/web_fetch.rs 4`, `tools/vended/shell.rs 2`, `plugins/vended/skills.rs 1`. No `TcpStream`,
no raw socket use.

Diffs vs TS worth a maintainer's eye:

1. **`web_fetch` default transport changed from sandboxed `curl` to a host-process HTTP client.**
   TS defaults to `transport: 'curl'` running *inside the agent sandbox* with
   `--proto '=http,https' --proto-redir '=http,https' --fail --max-time` (`harness-ts/src/tools/web-fetch.ts:84`).
   Rust always uses `reqwest` from the harness process (`web_fetch.rs:76-81`: `user_agent`,
   `timeout`, no explicit `redirect::Policy`). Consequences: (a) egress originates on the *host*,
   not the sandbox, so link-local/metadata (`169.254.169.254`) and host-local services are
   reachable where TS's default was not; (b) redirects follow `reqwest`'s default (up to 10) with no
   explicit cap — scheme downgrade is implicitly bounded because `reqwest` only speaks http(s), so
   the `--proto-redir` guarantee is preserved, but the hop limit and the SSRF reach are not TS's.
   Neither side filters private address space. This is the one item I would not ship silently.
2. **`shell` executes on the host, not through a sandbox seam.** `tools/vended/shell.rs:132-143`:
   `Command::new("sh")` with the **inherited** process environment plus `ShellConfig::env`, optional
   `cwd`. TS routes every command through `sandbox.execute` (`make-shell.ts:62`). The module doc
   states the decision ("this port runs on the host, reproducing the local-environment executor"),
   and it matches TS's `LocalSandbox` behaviour, but the harness-level default in TS is a sandbox,
   so full env passthrough (incl. `AWS_*`, tokens) is a broader default surface. Flagged, not
   blocking, since it is declared.
3. `plugins/environment.rs` reads env vars to build the environment block — same as the TS plugin.
   `models.rs` env reads are credential/region lookups. `file_tools.rs` / storage `std::fs` use is
   path-validated (`read_rejects_a_relative_path` and friends). No new surface there.

## Verdict: **PASS-WITH-FINDINGS**

No blocking item: all gates are green, the live logs are real, every sampled matrix row checks out,
and every default in `h1` either matches or differs for a declared reason.

Findings, in priority order:

1. **`subagent` spec is missing `context` / `last_messages`** (model-visible capability the TS
   harness exposes by default) and narrows the `tools` enum by dropping `subagent`. Fix or declare.
2. **`setupTelemetry()` has no Rust equivalent** and `telemetry.test.ts` (9 tests) is unported, with
   no mention in any report. Decide: port, or declare as a gap.
3. **`harness-agent.md` has no coverage section** while ~86 of 127 TS `agent/config/internal` tests
   are unported; `context-manager` has no matrix either. Add matrices so pass B has a target list.
4. **`web_fetch` default egress moved from the sandbox to the host process** with no explicit
   redirect policy — security-relevant default change (item 1 of §5).
5. `web_fetch` builtin config is not forwarded to the memory extractor (TS passes `webFetch` into
   `resolveMemory`).
6. `shell`/`read` integer schema fields serialize as `"type": "number"` instead of `"integer"`.
7. `resolve_interventions`' result is computed and dropped (`_interventions`); harmless today
   because any non-empty option errors, but it will silently no-op once presets land.

---

### Working-tree note

`git status --short` ends with exactly one entry, `M strands-rs/AGENTS.md`, which is **not mine**:
the tree was clean at 16:05:02 when this run started and the file was written at 16:05:13 by a
concurrent process (the diff is an implementer's crate-layout doc update, and it even leaves a
duplicate `plugins/` line in the tree diagram — worth fixing). My own scratch test
(`crates/strands-harness/tests/zz_scratch_specs.rs`) was deleted; no other source file was touched.
