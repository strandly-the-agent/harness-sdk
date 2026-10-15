# memory-stores module — API + test matrix (part A: `TestMemoryStore`)

TS spec: `strands-ts/src/vended-memory-stores/test-memory-store/{store.ts,__tests__/test-memory-store.test.node.ts}`.
Python tie-break: `strands-py/src/strands/vended_memory_stores/test_memory_store`.
Rust: `crates/strands-agents/src/memory/test_memory_store.rs` (public, mirroring
`file_memory_store.rs`; re-exported from `memory/mod.rs` and `lib.rs`).

Part B (`BedrockKnowledgeBaseStore`) is not ported yet — the `Bedrock Knowledge Base memory store`
row in `FEATURE_MATRIX.md` stays ❌.

## API

| TS | Rust | Notes |
|---|---|---|
| `new TestMemoryStore(config)` | `TestMemoryStore::with_config(TestMemoryStoreConfig) -> Result<Self>` | fallible: the TS constructor validates by throwing |
| `new TestMemoryStore({ name })` | `TestMemoryStore::new(name) -> Result<Self>` | convenience, default path |
| `new TestMemoryStore({ name, persist: false })` | `TestMemoryStore::ephemeral(name) -> Result<Self>` | convenience |
| `TestMemoryStoreConfig` | `TestMemoryStoreConfig { name, description, max_search_results, writable, extraction, persist, path }` | `TestMemoryStoreConfig::new(name)` + `..` update syntax |
| `store.add(content, metadata) -> { id }` | `TestMemoryStore::add_record(&self, content, metadata) -> Result<String>` | `MemoryStore::add` returns `()`, so the id receipt lives on the inherent method |
| `store.search(query, options)` | `MemoryStore::search` / inherent `search_memories` | |
| `name` / `description` / `maxSearchResults` / `writable` / `extraction` getters | `MemoryStore` trait methods | |
| `store.key` (backing-file path) | `TestMemoryStore::key(&self) -> &str` | |
| `DEFAULT_MAX_SEARCH_RESULTS` = 10, `_relevanceScore` | `DEFAULT_MAX_SEARCH_RESULTS`, `RELEVANCE_SCORE_KEY` | |

## Tests

All 36 TS `it(...)` cases are ported. The 33 store-level ones are unit tests in
`src/memory/test_memory_store.rs` (`cargo test -p strands-agents --all-features --lib
memory::test_memory_store`); the 3 `MemoryManager integration` ones become agent-loop tests in
`tests/memory_stores_loop.rs` (`--test memory_stores_loop`), driven by `MockMessageModel`.

| TS `describe` → `it` | Rust test | Status |
|---|---|---|
| construction: is writable by default | `is_writable_by_default` | pass |
| construction: honors an explicit `writable: false` | `honors_an_explicit_writable_false` | pass |
| construction: exposes name, description, and maxSearchResults | `exposes_name_description_and_max_search_results` | pass |
| construction: throws when maxSearchResults is less than 1 | `throws_when_max_search_results_is_less_than_1` | pass |
| construction: throws when name is empty | `throws_when_name_is_empty` | pass |
| construction: throws when an explicit path is empty | `throws_when_an_explicit_path_is_empty` | pass |
| construction: does no filesystem I/O on construction | `does_no_filesystem_io_on_construction` | pass |
| add: throws when the store is not writable | `throws_when_the_store_is_not_writable` | pass |
| add: throws on empty or whitespace content | `throws_on_empty_or_whitespace_content` | pass |
| add: returns an id for stored content | `returns_an_id_for_stored_content` | pass |
| add: deduplicates identical content, returning the existing id | `deduplicates_identical_content_returning_the_existing_id` | pass |
| add: persists a human-readable JSON array to disk | `persists_a_human_readable_json_array_to_disk` | pass |
| search: throws when maxSearchResults is less than 1 | `search_throws_when_max_search_results_is_less_than_1` | pass |
| search: returns no results for an empty or token-less query | `returns_no_results_for_an_empty_or_token_less_query` | pass |
| search: ranks higher token overlap first and exposes `_relevanceScore` | `ranks_higher_token_overlap_first_and_exposes_relevance_score` | pass |
| search: excludes records with no token overlap | `excludes_records_with_no_token_overlap` | pass |
| search: breaks ties by recency (most recent first) | `breaks_ties_by_recency_most_recent_first` | pass |
| search: caps results to maxSearchResults | `caps_results_to_max_search_results` | pass |
| search: tokenizes non-ASCII content as whole words | `tokenizes_non_ascii_content_as_whole_words` | pass |
| persistence: recalls entries from a fresh instance pointed at the same file | `recalls_entries_from_a_fresh_instance_pointed_at_the_same_file` | pass |
| persistence: is ephemeral when persist is false | `is_ephemeral_when_persist_is_false` | pass |
| persistence: starts empty when the backing file is missing | `starts_empty_when_the_backing_file_is_missing` | pass |
| persistence: defaults to a sanitized path under `<home>/.strands/memory` | `defaults_to_a_sanitized_path_under_home_strands_memory` | pass |
| failures: throws a clear error on a corrupt backing file | `throws_a_clear_error_on_a_corrupt_backing_file` | pass |
| failures: throws a clear error on a valid-but-wrong-shape backing file | `throws_a_clear_error_on_a_valid_but_wrong_shape_backing_file` | pass |
| failures: throws a clear error on a malformed record | `throws_a_clear_error_on_a_malformed_record` | pass |
| failures: throws a clear error on a record whose metadata is not an object | `throws_a_clear_error_on_a_record_whose_metadata_is_not_an_object` | pass |
| failures: accepts a record with null metadata | `accepts_a_record_with_null_metadata` | pass |
| failures: throws a clear error when the path is unreachable | `throws_a_clear_error_when_the_path_is_unreachable` | pass |
| concurrency: keeps all entries when many writes happen concurrently | `keeps_all_entries_when_many_writes_happen_concurrently` | pass |
| concurrency: reflects a just-added record when a search races the first add | `reflects_a_just_added_record_when_a_search_races_the_first_add` | pass |
| cross-SDK interop: loads a file written in the shared camelCase format | `loads_a_file_written_in_the_shared_camel_case_format` | pass |
| cross-SDK interop: writes the shared camelCase format with a Z-suffixed ms timestamp | `writes_the_shared_camel_case_format_with_a_z_suffixed_millisecond_timestamp` | pass |
| MemoryManager integration: stamps storeName on results returned through the manager | `memory_stores_loop::stamps_store_name_on_results_returned_through_the_manager` | pass |
| MemoryManager integration: writes through the manager add API | `memory_stores_loop::writes_through_the_manager_add_api` | pass |
| MemoryManager integration: ingests extracted facts through add when extraction fires | `memory_stores_loop::injects_stored_memories_into_the_model_call_and_extracts_facts_afterwards` | pass |

## Divergences

- `MemoryStore::add` returns `()`; the TS `{ id }` receipt is the inherent `add_record`.
- Construction is fallible (`Result`) instead of throwing.
- Backing-store resolution is eager — TS defers it to keep the module bundler-safe in a browser,
  which is not a concern in Rust. Construction still performs no filesystem I/O.
- The TS manager-integration cases drive hooks by hand against a mock agent; the Rust equivalents
  go through the real `Agent` + `MemoryManager` wiring, so the extraction case also asserts the
  injected `<memory>` block and that durable history is untouched.
- No live/integ test: the store is filesystem-only, so the unit tests cover it fully.
