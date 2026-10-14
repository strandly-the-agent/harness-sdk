# memory module — pass B

STATUS: **injection + extraction triggers shipped and green** on `agent-tasks/strands-rs-memory`.
748 lib tests pass (682 after pass A + 66 new/net across parallel merges); `cargo fmt` /
`clippy --all-targets --all-features -D warnings` / `cargo test --all-features` clean.

Pass B landed:
- `src/memory/injection.rs` — `MemoryInjectionConfig` (`trigger`/`max_entries`/`query`/`format`),
  `default_injection_query` (last user text on a user turn, else last assistant text),
  `default_injection_format` (`<memory><entry source="…">…</entry></memory>`),
  `escape_xml_text`/`escape_xml_attr`, `DEFAULT_MAX_ENTRIES = 5`. Reuses `ContextInjector`'s
  `is_user_turn` / `fold_into_last_user_message` / `InjectionTrigger`.
- `MemoryManager::inject(&mut [Message])` — searches with the resolved cap, truncates, folds the
  block into the last user message; fails open on no query / no hits / search error.
- `MemoryManager::init_agent` now wires hooks: `MessageAddedEvent` → `record_messages`,
  `AfterInvocationEvent` @ `hook_order::SDK_LAST` → `fire_extraction_trigger` (counts invocations,
  calls `extract_now`, logs errors), `BeforeModelCallEvent` → `inject` (our seam for TS's
  `InvokeModelStage.Input` middleware).
- `ExtractionTrigger` (`Invocation` | `Interval { turns }`, `interval()` validates `turns > 0`,
  default `Interval { turns: 5 }`) + `DEFAULT_EXTRACTION_TRIGGER_TURNS`.
- `MemoryManager::shared(config) -> Arc<Self>` + `bind_self` (a `OnceLock<Weak<Self>>` self-handle,
  also set by `AgentBuilder::memory_manager`): `Plugin::init_agent` takes `&self`, so the hooks
  need an owned handle. A manager registered via plain `.plugin(..)` logs a warning and skips
  hook wiring — pinned by `an_unbound_manager_skips_hook_wiring`.
- Flush/shutdown: `MemoryManager::flush` is public and the harness must call it explicitly;
  `Agent` has no shutdown hook/event and pass B deliberately did not add one.

Deviations from TS:
- Triggers are **manager-level**, not per-store (`MemoryManagerConfig::extraction_trigger`), because
  Rust's `extract_now` saves every extraction store in one pass.
- `resolve_extraction_config` (`extraction: true` → defaults from the agent's aux model) and the
  memory telemetry spans are still unported; no `agent.aux_model` in Rust yet.

New tests (10): `injection.rs` — escaping, default format with/without source, default query
(user turn / off-turn / empty), trigger gating, custom query+format, `max_entries` floor;
`memory_manager.rs` — `inject_folds_a_memory_block_into_the_last_user_message`,
`inject_is_a_no_op_when_disabled_off_turn_or_without_hits`,
`inject_fails_open_when_every_store_search_fails`,
`inject_caps_entries_and_passes_the_cap_to_the_store`,
`interval_trigger_rejects_zero_turns_and_fires_every_n`,
`init_agent_registers_the_trigger_and_injection_hooks`, `an_unbound_manager_skips_hook_wiring`,
`injection_reaches_the_model_and_extraction_runs_after_an_invocation` (loop-level, `MockMessageModel`
`calls()` sees the `<entry source="notes">` block, history untouched, extractor wrote one entry).

NEXT (pass C, in priority order):
1. Remaining `mem3-1` / `mem3-2` rows: file-store search ranking/limits, manager edge cases,
   `AggregateError` message shape (Rust flattens to one `Error::Other` with newline-joined causes).
2. `FileMemoryStore` key-aware extractor (`createKeyAwareExtractor`, `EXTRACTION_CONTRACT`) and
   `resolve_extraction_config` once `agent.aux_model` exists.
3. `TestMemoryStore` vended fake; `to_read_only`; memory telemetry spans.
4. Per-store triggers if/when `extract_now` becomes per-store.

Gate output (verbatim last lines, pass B):
- `cargo fmt` — no output.
- `cargo clippy --all-targets --all-features -- -D warnings` — `    Finished `dev` profile [unoptimized] target(s) in 22.72s`
- `cargo test --all-features` — `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` (last suite; `-p strands-agents --lib` line: `test result: ok. 748 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 15.39s`). Two transient failures (a doctest `cc` link error, 17 `strands-harness` lib failures) were shared-target-dir races with parallel workers; green on re-run.

---

# memory module — pass A

STATUS: **core shipped and green** on `agent-tasks/strands-rs-memory` (commit `9f387eab7`).
682 lib tests pass (645 pre-existing + 37 new); `cargo fmt` / `clippy -D warnings` / `cargo test --all-features` clean.

NEXT (pass B, in priority order):
1. **Injection** — `MemoryInjectionConfig`, `createInjectionMiddleware`, `isUserTurn`,
   `<memory><entry source="…">` default format, `escapeXmlText/Attr`. Needs the `injection/`
   module, which does not exist in the Rust crate yet; nothing of it is ported.
2. **Extraction triggers** — `InvocationTrigger` / `IntervalTrigger` + the `MessageAddedEvent` /
   `AfterInvocationEvent` hook wiring (`HookOrder::SDK_LAST`) that calls `record_messages` /
   `extract_now` automatically. Today the manager exposes those as public methods and the agent
   does not call them, so extraction is opt-in/manual.
3. **`FileMemoryStore` key-aware extractor** (`createKeyAwareExtractor`: injects existing topic
   headings into the prompt, `EXTRACTION_CONTRACT`) and `resolve_extraction_config`
   (`extraction: true` → defaults from the agent's aux model).
4. **`TestMemoryStore`** (vended fake, single-JSON-blob, `_relevanceScore` metadata).
5. **Telemetry spans** (`startMemory{Search,Add,Inject,Extract}Span`) — the tracer has no memory
   spans in Rust yet.
6. `to_read_only` wrapper (TS `toReadOnly` drops `getTools` on purpose), harness defaults
   (`memory: true` → `FileMemoryStore` under `./.agent/memory` + `ModelExtractor`,
   `injection.trigger = everyTurn`) — that lives in harness-rs, not the SDK.

## Files added

| File | Contents |
|---|---|
| `src/memory/types.rs` | `MemoryEntry`, `SearchOptions`, `AddMessagesContext`, `MemoryStoreConfig`, `MemorySearchOptions`, `MemoryAddOptions`, `MemoryToolConfig`, `MemoryAddToolConfig`, `ExtractionConfig`, `ExtractionResult`, `Extractor`/`ExtractorRef`, `MemoryStore`/`MemoryStoreRef`, `Metadata` |
| `src/memory/memory_manager.rs` | `MemoryManager` (+`MemoryManagerConfig`), validation, `search`/`add`, `memory_tools()`, `Plugin` impl, extraction coordinator (`record_messages`/`extract_now`/`flush`) |
| `src/memory/extraction/{mod.rs,model_extractor.rs}` | `ModelExtractor`, verbatim `DEFAULT_SYSTEM_PROMPT`, output parsing |
| `src/memory/file_memory_store.rs` | `FileMemoryStore` (+`FileMemoryStoreConfig`) over a `StorageRef` |
| `src/memory/mod.rs` | module + re-exports |
| `src/lib.rs`, `src/agent/builder.rs` | `pub mod memory`, top-level re-exports, `AgentBuilder::memory_manager(Arc<MemoryManager>)` (additive: pushes the manager as a plugin and extends `tools` with `memory_tools()`) |

## Optional capabilities (the "five code paths branch on has-add" warning)

Rust has no `typeof store.add === 'function'`, so the trait makes it explicit:
`supports_add()` / `supports_add_messages()` default to `false` and the default `add` /
`add_messages` bodies return `Error::Other("MemoryStore '<name>': add is not supported by this
store")`. `types::has_write_sink` is the `_hasWriteSink` equivalent. The branch points ported:
writable-store validation, extraction-config validation (extractor→`add`, none→`add_messages`),
`add_stores` (programmatic `add`), add-tool availability, and the extraction save path.
`memory::types::tests::optional_capabilities_default_to_unsupported_errors` pins the messages.

## Deviations from TS (deliberate, pass-A scope)

1. **No injection, no triggers, no telemetry spans** — see NEXT. `MemoryManager` is a `Plugin`
   whose `init_agent` only calls `store.initialize()`.
2. **Coordinator concurrency is simplified**: one `tokio::sync::Mutex<ExtractionState>` holding
   per-store watermarks (`marks`), failure/attempt counters and a single shared pending buffer,
   with saves performed sequentially while the lock is held. TS keeps a per-store promise chain
   and saves concurrently. Observable semantics preserved: per-store watermark (a store only sees
   `seq > mark`), sequence numbers passed through `AddMessagesContext`, buffer trimmed to the
   slowest store's mark, backoff after `SAVE_FAILURES_BEFORE_BACKOFF = 10` consecutive failures
   with a probe every `BACKOFF_PROBE_INTERVAL = 3` attempts, success clearing the streak.
   `flush()` is a bounded stability loop (`extract_now` until a pass attempts nothing) instead of
   TS's "await the chain set until stable". Not preserved: parallel store saves.
3. **`AggregateError` → `Error::Other`** with `"<summary>\n<inner 1>\n<inner 2>"`. Summaries are
   verbatim TS (`MemoryManager: store writes failed: a, b`, `failed to write 2 of 2 extracted
   entries`, `MemoryManager: failed to add 1 of 2 entries`); the inner messages are appended
   because Rust has no aggregate-error type. Asserted in tests.
4. **Tool config shape**: `search_tool: Option<MemoryToolConfig>` / `add_tool:
   Option<MemoryAddToolConfig>` replace TS's `false | object | undefined` union. Defaults match
   TS: search tool on, add tool off (`MemoryManager::new`); `with_add_tool` turns both on.
5. **Tools are built from `Arc<Self>`** (`memory_tools()`), not from `Plugin::tools()`, because the
   tool closures need to own the manager handle. `AgentBuilder::memory_manager` does both steps;
   `Plugin::tools()` returns empty. A caller using `.plugin(manager)` directly gets no tools —
   documented on the builder method.
6. **`add_memory` with `wait_for_writes: false` is not implemented** — the fire-and-forget branch
   needs a spawn + detach policy; pass A always awaits (TS's default is `true`).
7. **`FileMemoryStore` write locking is store-wide** (one `Mutex`), not per-key as in TS. Same
   lost-update guarantee, less concurrency.
8. **`_resolveAddToolStores` accepts names only** (`Option<Vec<String>>`), not
   `(string | MemoryStore)[]` — a store instance has no stable identity to compare in Rust, and TS
   resolves instances by name anyway.
9. `MemoryEntry.metadata` is a `BTreeMap<String, JsonValue>` (deterministic ordering for tests)
   rather than TS's insertion-ordered record.

## Namespacing

`FileMemoryStore::with_config` wraps the backend with
`resolve_namespace(storage, "memory/<name>")`, so a plain `LocalFileStorage("./.strands/")` and
store `facts` writes `./.strands/memory/facts/<slug>.md`, while an already-NAMESPACED handle
(`namespace(storage, "")`, i.e. the harness's `new LocalFileStorage(dir).namespace('')`) is left
untouched and files land at `<dir>/<slug>.md` — pinned by
`an_already_namespaced_storage_is_not_rescoped`.

## Needed from other modules

- `injection/` (middleware + `isUserTurn` + XML escaping) — blocks memory injection entirely.
- Hook events `MessageAddedEvent` / `AfterInvocationEvent` ordering (`SDK_LAST`) for triggers.
- Tracer memory spans.
- `agent.aux_model` (for `extraction: true` defaulting and `FileMemoryStore`'s built-in extractor).

## Gate output (verbatim last lines)

- `cargo fmt` — no output.
- `cargo clippy --all-targets --all-features -- -D warnings` — `    Finished \`dev\` profile [unoptimized] target(s) in 13.59s`
- `cargo test --all-features` — `test result: ok. 1 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 1.65s` (doctests; the lib line is `test result: ok. 682 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`). One earlier doctest run failed with `extern location for strands_agents does not exist` — a shared-target-dir race with a parallel worker, green on re-run.
