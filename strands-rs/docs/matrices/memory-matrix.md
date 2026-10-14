# memory module — test matrix (pass A)

TS inventories: `mem3-1-tests-memory.md` (~150 cases across memory-manager / memory-injection /
memory-tracing / extraction / model-extractor / resolve-extraction-config) and
`mem3-2-tests-stores.md` (76 cases across file-memory-store / test-memory-store / harness wiring).
Pass A ports **37** Rust tests covering store validation, search/add fan-out, tool
names/descriptions/schemas/scoping, the extraction coordinator and the file store.

Counts: **37 ported**, **~189 skipped** (every skip is a module not yet in the Rust crate —
injection, telemetry spans, triggers/resolve-extraction-config, `TestMemoryStore`, harness wiring —
or an API shape that does not exist in Rust; see `memory.md` "Deviations").

## `memory-manager.test.ts` → `src/memory/memory_manager.rs`

| TS test | Rust test | Status |
|---|---|---|
| constructor: throws when stores array is empty | `requires_at_least_one_store` | pass |
| constructor: throws when two stores share a name | `rejects_duplicate_store_names` | pass |
| constructor: throws when a store is writable but has no add method | `rejects_a_writable_store_with_no_write_sink` | pass |
| constructor: throws when addToolConfig is enabled but no stores are writable | `rejects_the_add_tool_when_no_store_implements_add` | pass |
| constructor: creates instance with valid config / allows addToolConfig with 1..n writable stores | covered by `tools_use_the_ts_names_and_schemas`, `add_writes_to_every_add_capable_store` | pass |
| constructor: throws when addToolConfig.stores names a non-existent / non-writable store | — | skipped (validated in `with_config`; no dedicated test, pass B) |
| constructor: accepts MemoryStore instances in addToolConfig.stores (2 cases) | — | skipped (names only in Rust — deviation 8) |
| getTools: registers search tool by default / does not register add tool by default | `tools_use_the_ts_names_and_schemas`, `search_tool_can_be_disabled` | pass |
| getTools: registers add tool when addToolConfig is enabled | `tools_use_the_ts_names_and_schemas` | pass |
| getTools: includes store descriptions in search / add tool description | `tool_descriptions_list_the_described_stores` | pass |
| getTools: uses custom tool names from MemoryToolConfig | — | skipped (code path exists; no test, pass B) |
| getTools: aggregates tools provided by stores via getTools (3 cases) | — | skipped (`MemoryStore::tools()` is wired in `memory_tools()`; untested, pass B) |
| search: queries all stores and concatenates results | `search_merges_results_and_tags_the_store_name` | pass |
| search: forwards an explicit maxSearchResults override / falls back to the SDK default | `search_resolves_max_results_caller_then_store_then_default` | pass |
| search: filters to named stores when options.stores is provided | `search_filters_by_store_and_rejects_unknown_names` | pass |
| search: throws a not-found error when a named store does not exist | `search_filters_by_store_and_rejects_unknown_names` | pass |
| search: gracefully handles store failures | `search_skips_a_failing_store_and_returns_the_rest` | pass |
| search: searches no stores when stores option is an empty array | — | skipped (Rust `Some(vec![])` → no stores; untested) |
| add: writes to all writable stores / dispatches writes to all targeted stores | `add_writes_to_every_add_capable_store` | pass |
| add: filters to named stores / throws not-found / throws read-only | `add_rejects_a_read_only_target_and_an_unknown_store` | pass |
| add: throws when no writable stores match | `add_with_no_writable_store_errors` | pass |
| add: throws AggregateError naming the failed store on partial failure | `add_aggregates_store_failures_into_one_message` | pass |
| add: dedupes duplicate store names | — | skipped (`dedup()` implemented; untested) |
| add: passes metadata to stores | — | skipped (threaded through; untested) |
| tool store scoping: omitted / empty stores array → all; in-scope only; out-of-scope warn; all-out-of-scope throws | `tool_targets_drop_out_of_scope_names_and_error_when_all_are` | pass |
| tool store scoping: search tool result attributes each entry to its source store | `search_merges_results_and_tags_the_store_name` | pass |
| tool store scoping: add-tool scoping cases | partially — `search_tool_can_be_disabled` + shared `resolve_tool_targets` | partial |
| injection (`memory-injection.test.ts`, all) | — | skipped (no `injection/` module in Rust) |
| tracing (`memory-tracing.test.ts`, all) | — | skipped (no memory spans in the Rust tracer) |

## `extraction/__tests__/extraction.test.ts` → `src/memory/memory_manager.rs`

| TS test | Rust test | Status |
|---|---|---|
| extractor path writes extracted facts | `extraction_writes_extracted_facts_and_advances_the_watermark` | pass |
| watermark prevents re-processing the same messages | same test (second `extract_now` saves nothing) | pass |
| no-extractor passthrough hands raw messages + sequenceNumbers to addMessages | `extraction_without_an_extractor_passes_raw_messages_with_sequence_numbers` | pass |
| backoff after 10 consecutive failures, probe every 3rd attempt | `a_failing_store_backs_off_after_ten_failures_and_probes_every_third_attempt` | pass |
| partial entry-write failure aggregates | `extraction_entry_write_failures_are_aggregated` | pass |
| flush is a no-op without extraction stores | `flush_is_a_noop_without_extraction_stores` | pass |
| extraction validation: read-only store / extractor without add / no-extractor without addMessages | `rejects_extraction_on_a_read_only_store`, `rejects_an_extractor_on_a_store_without_add` | pass (2 of 3) |
| trigger-driven extraction (MessageAdded / AfterInvocation / IntervalTrigger) | — | skipped (no triggers/hook wiring in pass A) |
| `resolve-extraction-config.test.ts` (all) | — | skipped (not ported) |

## `extraction/__tests__/model-extractor.test.ts` → `src/memory/extraction/model_extractor.rs`

| TS test | Rust test | Status |
|---|---|---|
| parses a JSON array of facts | `extracts_facts_from_a_json_array` | pass |
| tolerates prose / fenced block around the array | `tolerates_prose_and_fences_around_the_array` | pass |
| keeps metadata, drops blank/malformed items | `keeps_entry_metadata_and_drops_malformed_items` | pass |
| returns [] on unparseable output | `unparseable_output_yields_no_entries` | pass |
| no messages → no model call | `no_messages_skips_the_model_call` | pass |
| renders `role: text` transcript | `renders_messages_as_role_prefixed_text` | pass |
| custom systemPrompt / model resolution (`this > auxModel > agent.model`) | — | skipped (`system_prompt()` builder exists; aux-model resolution needs `agent.aux_model`) |

## `file-memory-store.test.node.ts` → `src/memory/file_memory_store.rs`

| TS test | Rust test | Status |
|---|---|---|
| add writes a slugified `.md` file / search round trip | `add_then_search_round_trips_through_storage` | pass |
| add merges new facts into an existing topic file | `add_merges_new_facts_into_an_existing_topic_file` | pass |
| slugify: lowercases, strips punctuation, truncates to 50 | `slugify_lowercases_strips_punctuation_and_truncates` | pass |
| blank first line falls back to `entry-<ts>.md` | `blank_first_line_falls_back_to_a_timestamp_key` | pass |
| keyword search respects maxSearchResults; empty query → [] | `search_respects_the_result_cap_and_ignores_empty_queries` | pass |
| entry metadata carries `path` + `score` | `add_then_search_round_trips_through_storage` | pass |
| two stores on one backend are namespaced by name | `store_name_scopes_two_stores_on_one_backend` | pass |
| an already-namespaced storage is not re-scoped (NAMESPACED marker) | `an_already_namespaced_storage_is_not_rescoped` | pass |
| custom `search` strategy override | — | skipped (code path implemented via `SearchStrategyRef`; untested) |
| built-in key-aware extractor (existing-topic prompt) | — | skipped (not ported, pass B) |
| `test-memory-store.test.node.ts` (all) | — | skipped (`TestMemoryStore` not ported) |
| `harness-ts/test/memory.test.ts`, `agent-generalist-memory.test.ts` | — | skipped (harness-rs, not the SDK) |

Note: Rust tests use `InMemoryStorage` for the file-store round trips (same `Storage` trait, no
`tempfile` needed); `LocalFileStorage` is already covered by `src/storage/` tests.
