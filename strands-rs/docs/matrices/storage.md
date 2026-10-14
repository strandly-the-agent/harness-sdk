# storage module — LocalFileStorage + keyword search (strands-rs)

Branch `agent-tasks/strands-rs-storage`. Files added:
- `crates/strands-agents/src/storage/local_file_storage.rs`
- `crates/strands-agents/src/storage/search.rs`
Modified (additive): `storage/mod.rs` (module decls + re-exports, `InMemoryStorage.search_strategy`,
`InMemoryStorage::search`), `lib.rs` (`pub use storage::{InMemoryStorage, LocalFileStorage, Storage, StorageRef}`).

Gates (last line verbatim):
- `cargo fmt` → (no output, exit 0)
- `cargo clippy --all-targets --all-features -- -D warnings` → `    Finished \`dev\` profile [unoptimized] target(s) in 11.65s`
- `cargo test --all-features` → `test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s` (doc-tests; lib: `test result: ok. 586 passed; 0 failed; 0 ignored`)
- `cargo test --all-features storage` → `test result: ok. 71 passed; 0 failed; 0 ignored; 0 measured; 515 filtered out`

## Test matrix (TS `it(...)` → Rust test)

### `storage/__tests__/local-file-storage.test.node.ts`
| TS test | Rust test | status |
|---|---|---|
| round-trips bytes | `local_file_storage::tests::round_trips_bytes` | pass |
| creates nested directories | `creates_nested_directories` | pass |
| overwrites existing values | `overwrites_existing_values` | pass |
| returns null for missing keys | `returns_null_for_missing_keys` | pass |
| writes atomically via tmp file | `writes_atomically_via_tmp_file` | pass |
| cleans up tmp file on rename failure | `cleans_up_tmp_file_on_rename_failure` | pass (blocked parent instead of chmod) |
| rejects keys containing backslashes | `rejects_keys_containing_backslashes` | pass |
| rejects backslash path traversal | `rejects_backslash_path_traversal` | pass |
| (jail rule, from `normalizeKey`) | `rejects_dot_dot_path_traversal` | pass (extra) |
| removes an existing key | `removes_an_existing_key` | pass |
| is a no-op for missing keys | `delete_is_a_no_op_for_missing_keys` | pass |
| lists keys under a prefix | `lists_keys_under_a_prefix` | pass |
| returns all keys for empty prefix | `returns_all_keys_for_empty_prefix` | pass |
| returns empty array when base directory does not exist | `returns_empty_array_when_base_directory_does_not_exist` | pass |
| excludes scratch files | `excludes_scratch_files` | pass |
| does not exclude user .tmp files | `does_not_exclude_user_tmp_files` | pass |
| returns keys sorted lexicographically | `returns_keys_sorted_lexicographically` | pass |
| namespace returns a LocalFileStorage rooted at the subdirectory | `namespace_returns_a_local_file_storage_rooted_at_the_subdirectory` | pass |
| namespace isolates keys from the parent storage | `namespace_isolates_keys_from_the_parent_storage` | pass |
| namespace exposes baseDir pointing to the subdirectory | `namespace_exposes_base_dir_pointing_to_the_subdirectory` | pass |
| indexes writes and delegates search to a searchStrategy from config | `indexes_writes_and_delegates_search_to_a_search_strategy` | pass |
| accepts a positional sandbox and search strategy | — | skipped: no `sandbox` module in Rust (`with_search_strategy` covers the strategy half) |
| routes I/O through a sandbox from config | — | skipped: no `sandbox` module |
| carries a searchStrategy from config through forSandbox and namespace | `carries_a_search_strategy_through_namespace` | pass (namespace half only; `forSandbox` N/A) |
| marks QmdSearchStrategy as requiring host filesystem access | — | skipped: `QmdSearchStrategy` out of scope (`SearchStrategy::requires_host_fs` seam exists) |
| rejects a host filesystem strategy passed with a sandbox | — | skipped: no sandbox |
| rejects a host filesystem strategy passed with a sandbox in config | — | skipped: no sandbox |
| accepts a sandbox-safe strategy passed with a sandbox | — | skipped: no sandbox |
| rejects at runtime a strategy typed as SearchStrategy that declares requiresHostFs | — | skipped: no sandbox |
| drops a host filesystem strategy in forSandbox and falls back to keyword search | — | skipped: no sandbox |
| keeps a sandbox-safe strategy in forSandbox | — | skipped: no sandbox |
| (constructor baseDir normalization, from TS `_baseDir` regexes) | `base_dir_collapses_slash_runs_and_strips_trailing_slash` | pass (extra) |
| (default keyword scan, TS `search()` fallback) | `uses_the_default_keyword_scan_without_a_strategy` | pass (extra) |

`local-file-storage.test-d.ts` is a type-level (`expectTypeOf`) suite — not portable; the Rust
equivalent is that `LocalFileStorage` has no sandbox constructor at all.

### `storage/search/__tests__/keyword.test.ts`
| TS test | Rust test | status |
|---|---|---|
| returns matching entries scored by token overlap | `search::tests::returns_matching_entries_scored_by_token_overlap` | pass |
| returns empty array for empty query | `returns_empty_array_for_empty_query` | pass |
| returns empty array for whitespace-only query | `returns_empty_array_for_whitespace_only_query` | pass |
| matches case-insensitively | `matches_case_insensitively` | pass |
| includes the key in the scoring text | `includes_the_key_in_the_scoring_text` | pass |
| ranks results by score descending | `ranks_results_by_score_descending` | pass |
| returns empty array when no content matches | `returns_empty_array_when_no_content_matches` | pass |
| (tokenizer / STOP_WORDS exports) | `tokenize_splits_on_non_word_characters`, `stop_words_are_recognized` | pass (extra) |

`search/__tests__/qmd.test.node.ts` — skipped wholesale: `QmdSearchStrategy` (host-fs markdown
index) is out of scope for this task.

### `storage/__tests__/in-memory-storage.test.ts` (search-strategy section; the rest already covered on the branch)
| TS test | Rust test | status |
|---|---|---|
| indexes writes and delegates search to a searchStrategy from config | `storage::tests::in_memory_indexes_writes_and_delegates_search_to_a_search_strategy` | pass |
| accepts a positional search strategy | same test (`with_search_strategy`) | pass (one Rust API, no overloads) |
| uses the default keyword scan with an empty config | `in_memory_uses_the_default_keyword_scan_with_no_strategy` | pass |
| copies bytes on write/read to prevent aliasing | `in_memory_copies_bytes_on_write_and_read_to_prevent_aliasing` | pass |

Counts: 32 ported/extra passing, 9 skipped (8 sandbox/Qmd + 1 type-level suite), 0 failing.

## Deviations from TS
- **No `Sandbox` support.** TS `LocalFileStorage` can route I/O through a sandbox (`forSandbox`,
  `LocalFileStorageConfig.sandbox`, `isSandboxSafe`); there is no `sandbox` module in the Rust crate
  yet, so only the host-fs path is ported. `SearchStrategy::requires_host_fs()` is kept as the seam
  the sandbox-safety check will use once a sandbox lands.
- **Builder instead of constructor overloads**: `LocalFileStorage::new(root).with_search_strategy(s)`
  and `InMemoryStorage::new().with_search_strategy(s)` replace TS's config/positional overloads.
  `InMemoryStorage::new()` is unchanged for existing callers.
- **`namespaced(prefix)` is the inherent method name** (returns the concrete `LocalFileStorage` so
  `base_dir()` is reachable); the trait's `Storage::namespace` delegates to it. Named differently
  from TS's `namespace` only to avoid inherent/trait receiver ambiguity on `Arc<LocalFileStorage>`.
- **`Storage::search` always returns `Some(..)`** for both backends — matching TS, where an unset
  strategy falls back to `KeywordSearchStrategy` rather than disabling search.
- **Error messages** keep the TS text and append the OS cause (`Failed to write '<key>' to local
  storage: <io error>`) since Rust `Error` has no `cause` chain field.
- **Scores are `f64`** (the existing `StorageSearchResult.score` type); token overlap is integral.
- Tokenizer uses `char::is_alphanumeric() || '_'` for TS's `[^\p{L}\p{N}_]+`.
- `STOP_WORDS` is a `&[&str]` const + `is_stop_word()` (TS exports a `Set`); nothing in scope
  consumes it yet — kept for the memory/context-manager ports that reference it.

## Needed from other modules
Nothing stubbed. `QmdSearchStrategy` and sandbox-routed storage remain unported; a future
`sandbox` module should add the `forSandbox`/`requires_host_fs` rejection path.
