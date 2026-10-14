# Phase 2 API bar-raise — strands-harness + new strands-agents surface

Branch `agent-tasks/strands-rs`, read-only, 15-min budget. Paths relative to `strands-rs/crates/`.
Sibling checked: `harness-ts/src/agent.ts` (`HarnessAgentOptions`), `harness-ts/src/index.ts`.

VERDICT: **accept-with-conditions** — fix the three 🔴 before anything builds on `strands-harness`;
`Agent::memory_manager` + the options-shape change are hard to add later without churn.

## 🔴 painful / misleading / parity-breaking

1. **No handle to the memory manager the harness builds for you** — `strands-harness/src/agent.rs:172`
   `pub async fn create_harness(options: HarnessAgentOptions) -> Result<Agent>` and
   `strands-agents/src/agent/mod.rs` has no `memory_manager()` accessor (only
   `AgentBuilder::memory_manager`, `agent/builder.rs:95`, whose own doc says "call `flush` on shutdown").
   `MemoryOption::On` is the default, so every default user has an un-flushable manager. TS exposes
   `agent.memoryManager`. Evidence: `tests/integ_harness.rs:22-35` rebuilds the manager by hand
   (incl. `resolve_model(None, DEFAULT_MODEL, Effort::Auto, false, true, false)`) just to `flush()`.
   **Fix (SDK, TS parity, no wrapper type):** store the Arc in `AgentInner` and add
   `impl Agent { pub fn memory_manager(&self) -> Option<Arc<MemoryManager>> }`; `create_harness` keeps returning `Agent`.

2. **`CachingOption::Auto` does not do what it says** — `agent.rs:36-46,192-193`:
   `caching_explicit = options.caching.is_some()`, so `Some(CachingOption::Auto)` → explicit=true →
   `require_or_warn` *errors* on a non-caching provider (`models.rs:506,516`), while the variant doc
   says "On where the provider supports it". Only `None` is actually auto, which makes `#[default] Auto`
   on an `Option<_>` field a trap. **Fix:** `pub caching: CachingOption` (drop the `Option`), and
   `let caching_explicit = matches!(options.caching, CachingOption::On)`.

3. **`Option<Enum-with-Off-variant>` double-encodes "unset" on three fields** — `agent.rs:147,149,159`:
   `background_tasks: Option<BackgroundTasksOption>` (`None | Some(On)` are one arm at `agent.rs:554`),
   `caching: Option<CachingOption>` (above), `interventions: Option<InterventionsOption>` (`None | Some(Off)`
   one arm at `interventions.rs:40`). Meanwhile `session/skills/memory/context_manager` are non-`Option`.
   TS uses `boolean | Config` with a single "absent" state. **Fix:** make all three plain enums with
   `#[derive(Default)]` + `#[default]` on the harness default (`BackgroundTasksOption::On`,
   `CachingOption::Auto`, `InterventionsOption::Off`).

## 🟡 fix before 0.1

4. **"Pre-built instance" variant is named two ways** — `agent.rs:79` `SessionOption::Manager(Arc<SessionManager>)`,
   `agent.rs:116` `MemoryOption::Manager(Arc<MemoryManager>)` vs `agent.rs:93` `SkillsOption::Instance(Arc<AgentSkills>)`
   and SDK `context_manager/mod.rs:119` `ContextManagerOption::Instance(Arc<ContextManager>)`. Pick `Instance`
   everywhere (SDK precedent). Also `SkillsOption::Sources(Vec<SkillSource>)` vs `Config(..)` elsewhere — acceptable
   since it isn't a config struct, but document the rule.

5. **Three shapes for "attach a manager" on `AgentBuilder`** — `agent/builder.rs:95`
   `memory_manager(Arc<MemoryManager>)`, `:153` `session_manager(impl Into<Arc<SessionManager>>)`,
   `:121` `context_manager(ContextManagerOption)`. **Fix:** `memory_manager(manager: impl Into<Arc<MemoryManager>>)`.
   Related: `strands-harness/src/memory.rs:54` `resolve_memory(..) -> Result<MemoryManager>` while the only consumer
   (`MemoryOption::Manager`) wants `Arc<MemoryManager>` and `MemoryManager::memory_tools(self: &Arc<Self>)` /
   `bind_self()` only work through an Arc → return `Result<Arc<MemoryManager>>` (there is already `MemoryManager::shared`).

6. **`resolve_model` is six positional args, three of them `bool`** — `models.rs:490-497`
   `resolve_model(model: Option<ModelOption>, default: &str, effort: Effort, web_search: bool, caching: bool, caching_explicit: bool)`.
   Public, and the integ test had to call it. Sibling: `ResolveMemoryOptions` struct (`memory.rs:30`).
   **Fix:** `pub struct ResolveModelOptions { model, default, effort, web_search, caching: CachingOption }` + `..Default::default()`.

7. **`ModelOption` has `From<&str>/From<String>` but no `From<Arc<dyn Model>>`** — `models.rs:170-180`.
   `model: Some(my_model.into())` works for a string and not for the instance; callers must spell
   `Some(ModelOption::Instance(Arc::new(m)))`. Add `impl From<Arc<dyn Model>> for ModelOption` and
   `impl<M: Model + 'static> From<M>` is not possible (coherence with `From<String>`), so document the Arc path.

8. **`Agent::set_session_id` is `pub` but documented as an internal seam** — `agent/mod.rs:221-224`
   "used by `SessionManager` during initialization". A consumer calling it after init silently desyncs
   storage keys. Make it `pub(crate)` or `#[doc(hidden)]`; TS has no public setter.

9. **Over-exported implementation detail at the crate root** — `strands-harness/src/lib.rs:21-26,29`:
   `builtin_tool_config, enabled_builtin_tools, resolve_builtin_tools, select_builtin_plugins, select_builtin_tools,
   web_search_explicit, web_search_mode, WebSearchModel<'a>, ResolvedBuiltinTools`, plus `models.rs:73,82`
   `pub fn warn_once / reset_warn_once` and `subagent.rs:744-862` `fork_messages/with_history/render_context/with_context`.
   TS `index.ts` exports only `BUILTIN_*_NAMES`, the types and the tool factories. Each pub fn is a 0.1 compat
   promise; make them `pub(crate)` (keep `BuiltinToolName`, `BuiltinToolsConfig`, `BuiltinToolSetting`, consts).

10. **Options that only error** — `agent.rs:140-141` `mcp_servers: Vec<String>` ("a non-empty value is an error"),
    `agent.rs:159` `interventions` (any preset errors, `interventions.rs:42`). A field whose only valid value is empty is
    a trap for readers coming from TS. Drop both until implemented (adding a field is non-breaking given `..Default::default()`).

11. **Stringly-typed plugin names and `String` paths** — `agent.rs:145` `builtin_plugins: Option<Vec<String>>`
    (TS: `readonly BuiltinPluginName[]`; Rust already has `BuiltinToolName` enum) → `Option<Vec<BuiltinPluginName>>`.
    `SessionConfig.dir: Option<String>` (`agent.rs:65`), `MemoryConfig.dir` (`:100`), `ResolveMemoryOptions.dir`
    (`memory.rs:38`) → `Option<PathBuf>`; the test shows the cost: `dir.join("sessions").to_string_lossy().into_owned()`
    (`tests/integ_harness.rs:44`).

12. **No escape hatch to the rest of `AgentConfig`** — TS `HarnessAgentOptions extends Omit<AgentConfig, ...>` so
    `hooks`, `messages`, `state`, `conversationManager`, etc. pass through; Rust exposes only `id/name/description`
    (`agent.rs:163-167`). Either add `pub configure: Option<Box<dyn FnOnce(AgentBuilder) -> AgentBuilder + Send>>`
    or a second entry point `pub async fn create_harness_builder(options) -> Result<AgentBuilder>`.

13. **Config errors go through `Error::other`** — 21 call sites in `strands-harness/src`, none use a typed variant;
    the SDK `error.rs` has `ToolValidation`, `Plugin`, `Session` but no `Configuration`. Add `Error::Configuration(String)`
    (or a harness `HarnessError` with `From` into `strands_agents::Error`) so callers can match on misconfiguration vs runtime.

## 🟢 nits

14. `HarnessAgentOptions` all-pub + `Default` is the right call for TS-literal parity, but it is **incompatible with
    `#[non_exhaustive]`** (functional-update syntax is rejected cross-crate on non-exhaustive structs). Keep it as is,
    state "always spread `..Default::default()`" in the struct doc (`agent.rs:125`), and add `Debug` (currently missing on
    `HarnessAgentOptions`, `BackgroundTasksOption`, `MemoryConfig`, `SkillsOption`, `MemoryOption`; `BackgroundTasksConfig` already derives it).
15. `lib.rs:32` re-exports `read`, `write`, `edit` at the crate root. No std-prelude collision (`std::fs::read/write` aren't
    in the prelude; `write!` is macro-namespace), but `use strands_harness::*; use std::fs::*;` makes `read/write` ambiguous
    at use, and the crate itself needed `write as write_tool` (`agent.rs:31`). Leave them under `strands_harness::tools::`
    only; TS parity is preserved via `tools::{read, write, edit}`.
16. Factory naming is TS-parity (`make_read/read`, `make_exa_web_search/exa_web_search`, `make_web_fetch`) — fine. But the
    root does not re-export the subagent surface TS does (`make_subagent, MakeSubagentOptions, Preset, AgentSpec`,
    `subagent.rs:371,541`); add them or say the subagent is harness-internal.
17. `Skill::from_content(content, strict: bool, path)`, `from_file(_, strict: bool)`, `from_directory(_, strict: bool)`
    (`plugins/vended/skills.rs:90,147,186`) — positional bools; `AgentSkills::new/strict` already shows the better shape.
18. `Agent::session_id(&self) -> String` (`agent/mod.rs:212`) allocates per call; acceptable behind the lock, but
    `Arc<str>` would make the lazily-generated id cheap to hand around.
19. `MemoryOption::is_off` (`agent.rs:119-123`) exists only for memory; either add `is_off()` to all option enums or inline it.
20. No `parking_lot`/`indexmap` types found in any `pub` signature (checked `pub .*Mutex<|RwLock<|IndexMap`); `BeforeModelCallEvent.messages: Vec<Message>`
    (`hooks/events.rs:186`) is a clean, documented seam — note it clones the full conversation per model call for every subscriber.

Process: no `needs-api-review` / `completed-api-review` label visible on this branch work; items 1, 3, 12 touch the shape of
the primary entry point and warrant the API meeting before 0.1.

## Fixes applied / skipped (branch `agent-tasks/harness-rs-fix2`, harness crate only)

Gates from `strands-rs/`: `cargo fmt --check` clean; `clippy --all-targets --all-features -D warnings`
→ `Finished \`dev\` profile [unoptimized] target(s) in 33.17s`; `cargo test --all-features -p strands-harness`
→ `test result: ok. 173 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.11s`;
`cargo build --examples --all-features` → `Finished \`dev\` profile [unoptimized] target(s) in 26.32s`.

APPLIED
- **2, 3** (🔴): `caching: CachingOption` (`#[default] Auto`), `background_tasks: BackgroundTasksOption`
  (`#[default] On`), `interventions: InterventionsOption` (`#[default] Off`) — no more `Option<enum-with-Off>`.
  `resolve_model` treats `Auto` as TS-unset (warn) and only `On` as explicit; `resolve_interventions`/
  `resolve_background_tasks` take `&Option`-free enums. Tests updated.
- **4** (🟡): `SessionOption::Instance`, `MemoryOption::Instance` (was `Manager`), matching
  `ContextManagerOption::Instance`.
- **5/6/7** (🟡): `resolve_memory -> Result<Arc<MemoryManager>>` (via `MemoryManager::shared`);
  positional `resolve_model(..6 args)` **replaced** by `resolve_model(ResolveModelOptions { .. })`
  (`Default` = `DEFAULT_MODEL`, `Effort::Auto`, caching `Auto`), all 8 call sites updated;
  `impl From<Arc<dyn Model>> for ModelOption` added (`From<&str>/From<String>` already existed).
- **10/11** (🟡): `builtin_plugins: Option<Vec<BuiltinPluginName>>` (new enum; kept `Option` because TS
  distinguishes unset → defaults from `[]` → none), `SessionConfig.dir`/`MemoryConfig.dir`/
  `ResolveMemoryOptions.dir` → `Option<PathBuf>`. `mcp_servers`/`interventions` kept as-is (still error).
- Root re-exports trimmed to `create_harness` + options + `build_system_prompt`/`HARNESS_CONTRACT` +
  `resolve_model`/`ResolveModelOptions`/`Effort`/`ModelOption` + `resolve_memory`/`ResolveMemoryOptions` +
  `Todos`/`EnvironmentContext`; `resolve_builtin_tools`/`select_*`/`web_search_mode`/`WebSearchModel`/
  `ResolvedBuiltinTools` and the tool factories are reachable only via `builtin_tools::`/`tools::`;
  `#[doc(hidden)]` on `warn_once`/`reset_warn_once`; added `pub mod prelude`.
- Validation report: default `subagent` schema now offers `context` (Choice over `CONTEXT_MODES`) and
  `last_messages`, and its `tools` enum includes `subagent` (TS `builtin-tools.ts:293-302`);
  `read.offset`/`read.limit` → `"type": "integer"`; `web_fetch` gets an explicit
  `reqwest::redirect::Policy` (http(s) targets only, ≤10 hops) + a module-doc note that the default
  transport is host-process egress, not a sandbox; the `web_fetch` builtin config's `model` is now
  forwarded to `resolve_memory` as `web_fetch_model` (was `let _ = web_fetch`).
- Drive-by: 3 `resolve_caching` tests in `models.rs` took the `WARN_LOCK`; they were racing the
  warn-once set (intermittent `does_not_warn_when_caching_is_supported` failure, pre-existing).

SKIPPED / NOTED
- **1** (🔴) `harness_memory_manager`: `rg -n "fn memory_manager" crates/strands-agents/src/agent/mod.rs`
  → no match on this branch, so **needs the SDK accessor `Agent::memory_manager()` (other branch)**.
  `create_harness` still returns `Agent`; `tests/integ_harness.rs` still rebuilds the manager, now via
  `resolve_model(ResolveModelOptions::default())` + `resolve_memory`.
- `shell.timeout` is still `"type": "number"`: the schema lives in
  `crates/strands-agents/src/tools/vended/shell.rs` (SDK crate, owned by the other worker).
- `programmatic_tool_caller` is deliberately **not** in the subagent `tools` enum (TS lists it; the Rust
  tool is unported, so offering the name would let the model select a nonexistent tool).
