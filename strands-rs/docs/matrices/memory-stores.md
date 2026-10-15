# memory-stores module — API + test matrix (part A: `TestMemoryStore`)

TS spec: `strands-ts/src/vended-memory-stores/test-memory-store/{store.ts,__tests__/test-memory-store.test.node.ts}`.
Python tie-break: `strands-py/src/strands/vended_memory_stores/test_memory_store`.
Rust: `crates/strands-agents/src/memory/test_memory_store.rs` (public, mirroring
`file_memory_store.rs`; re-exported from `memory/mod.rs` and `lib.rs`).

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

# Part B — `BedrockKnowledgeBaseStore`

TS source: `strands-ts/src/vended-memory-stores/bedrock-knowledge-base/{store,types}.ts`.
Rust: `crates/strands-agents/src/memory/bedrock_knowledge_base.rs`, feature `bedrock-kb`.

## API map

| TS | Rust |
| --- | --- |
| `new BedrockKnowledgeBaseStore(config)` | `BedrockKnowledgeBaseStore::with_config(BedrockKnowledgeBaseStoreConfig) -> Result<Self>`; `BedrockKnowledgeBaseStore::new(knowledge_base_id) -> Result<Self>` for a read-only store named after the knowledge base (both async: they may build a default client) |
| `BedrockKnowledgeBaseStoreConfig` | `BedrockKnowledgeBaseStoreConfig { name, description, config, scope, filter, max_search_results, min_score, max_score, writable, access_control_list, extraction }` |
| `BedrockKnowledgeBaseConfig` | `BedrockKnowledgeBaseConfig { knowledge_base_id, data_source_id, data_source_type, s3, scope_metadata_key, knowledge_base_type, region, runtime_client, agent_client }` |
| `s3: { bucket, prefix, client }` | `BedrockKnowledgeBaseS3Config { bucket, prefix, client }` |
| `accessControlList: AccessControlEntry[]` | `Vec<AccessControlEntry { name, r#type, access: AccessControlAccess }>` (`Allow` / `Deny`, `as_str()` for the wire value) |
| `filter: RetrievalFilter` (SDK type) | owned `RetrievalFilter` enum + `FilterAttribute { key, value }` |
| `store.search(query, options)` | `MemoryStore::search(&self, query, &SearchOptions) -> Result<Vec<MemoryEntry>>` |
| `store.add(content, metadata) -> { id }` | `add_document(&self, content, metadata) -> Result<String>`; `MemoryStore::add` delegates and drops the id |
| `store.initialize()` | `MemoryStore::initialize` → `GetKnowledgeBase`, cached in a `OnceCell` |
| `store.{name,description,maxSearchResults,writable,extraction}` | same-named `MemoryStore` methods |
| `store.{scope,scopeMetadataKey,filter,accessControlList}` | inherent accessors `scope()`, `scope_metadata_key()`, `filter()`, `access_control_list()` |
| `DataSourceType`, `KnowledgeBaseType` | `DataSourceType {Custom,S3,Other}`, `KnowledgeBaseType {Vector,Managed,Kendra,Sql}` |

## Test map

43 unit tests beside the code (`cargo test -p strands-agents --features bedrock-kb --lib
memory::bedrock_knowledge_base` → `43 passed`), covering 48 of the 84 TS `it(...)` cases (several
Rust tests fold two or three closely related TS assertions together).

| TS case (abridged) | Rust test | status |
| --- | --- | --- |
| constructor: name/scope/description/maxSearchResults carried through, writable defaults false, extraction defaults undefined | `carries_through_the_config_surface` | pass |
| extraction config: exposes a configured extraction config verbatim | `exposes_a_configured_extraction_config` | pass |
| constructor: throws when writable but dataSourceType is omitted / `'OTHER'` | `rejects_a_writable_store_without_a_writable_data_source` | pass |
| add — CUSTOM: throws when dataSourceId is missing | `rejects_a_writable_store_without_a_data_source_id` | pass |
| add — S3: throws when the s3 config is missing | `rejects_a_writable_s3_store_without_an_s3_config` | pass |
| constructor: throws when minScore and maxScore are both set | `rejects_min_and_max_score_together` | pass |
| constructor: throws when a zero minScore is paired with maxScore | `rejects_a_zero_min_score_paired_with_a_max_score` | pass |
| constructor: throws when maxSearchResults is less than 1 | `rejects_a_store_limit_below_one`, `rejects_a_per_search_limit_below_one` | pass |
| (Rust-only) empty `knowledgeBaseId` is rejected | `rejects_an_empty_knowledge_base_id` | pass |
| search: RetrieveCommand with query + default limit of 10 | `defaults_the_result_limit` | pass |
| search: store maxSearchResults / per-call override | `per_search_limit_overrides_the_store_default` | pass |
| search: derives a scope filter (default key `namespace`) | `derives_a_scope_filter` | pass |
| search: honors a custom scopeMetadataKey (search) + drops colliding keys (write) | `uses_a_custom_scope_metadata_key_for_search_and_writes` | pass |
| search: prefers an explicit filter over a scope-derived one | `an_explicit_filter_overrides_the_scope_filter` | pass |
| scope/filter resolution: applies no filter when there is no scope | `applies_no_filter_when_the_store_has_no_scope` | pass |
| scope/filter resolution: writes are scoped even with an explicit search filter | `scopes_writes_even_when_an_explicit_search_filter_is_set` | pass |
| config reuse: one connection config serves several scoped stores | `reuses_one_connection_config_across_scopes` | pass |
| search: maps content, metadata, location, score onto each entry; defaults missing content to `""` | `maps_results_to_memory_entries` | pass |
| score bounds: min/max kept, zero not treated as unset, unscored kept, nothing clears the bound | `drops_results_below_min_score_and_keeps_unscored_ones`, `drops_results_above_max_score` | pass |
| search: logs and rethrows when retrieve fails | `wraps_retrieve_failures` | pass |
| kind detection: detects from GetKnowledgeBase, managedSearchConfiguration, detects once and reuses, initialize is idempotent | `detects_a_managed_knowledge_base_and_caches_the_lookup`, `uses_managed_search_configuration_for_a_managed_knowledge_base` | pass |
| kind detection: treats non-managed types as vector | `treats_an_unrecognized_knowledge_base_type_as_vector` | pass |
| kind detection: throws when detection fails | `wraps_get_knowledge_base_failures`, `rejects_a_knowledge_base_with_no_reported_type` | pass |
| add — non-writable store throws | `rejects_add_on_a_non_writable_store`, `exposes_add_support_from_writable` | pass |
| add — CUSTOM: throws when content is empty/whitespace | `rejects_blank_content` | pass |
| add — CUSTOM: returns the generated id, same id as the document identifier, inline document with no metadata field | `ingests_an_inline_custom_document_and_returns_its_id` | pass |
| add — CUSTOM: scope as a leading inline attribute | `attaches_the_scope_as_a_leading_inline_attribute` | pass |
| add — CUSTOM: drops metadata keys colliding with scopeMetadataKey | `drops_metadata_keys_that_collide_with_the_scope_key` | pass |
| add — CUSTOM: maps supported metadata types, skips unsupported | `maps_supported_metadata_types_and_skips_unsupported_ones` | pass |
| add — CUSTOM: ACL verbatim in inline metadata, with scope attributes | `attaches_the_access_control_list_in_inline_metadata` | pass |
| add — CUSTOM: raises when ACL is set with no inline attributes | `rejects_an_inline_acl_with_no_attributes` | pass |
| add — CUSTOM: rewrites a missing-ACL ValidationException / does not rewrite when ACL configured | `rewrites_a_missing_acl_validation_exception`, `does_not_rewrite_when_an_acl_is_configured` | pass |
| add — CUSTOM: logs and rethrows when ingestion fails | `wraps_a_generic_ingestion_failure` | pass |
| add — S3: uploads the content object, ingests an S3 document, returns the object URI | `uploads_the_content_object_and_ingests_an_s3_document` | pass |
| add — S3: does not double the prefix slash | `does_not_double_the_prefix_slash` | pass |
| add — S3: sidecar from scope + caller metadata, document points at it | `writes_a_sidecar_carrying_scope_and_metadata_and_points_the_document_at_it` | pass |
| add — S3: ACL-only sidecar with capitalized keys | `writes_an_acl_only_sidecar_with_capitalized_keys` | pass |
| add — S3: logs and rethrows when the upload fails, before any ingestion | `wraps_an_s3_upload_failure_before_any_ingestion` | pass |
| (live) write + search round trip | `integ_memory_kb::ingests_a_document_and_searches_the_knowledge_base` | skipped (no KB) |

### TS cases deliberately **not** ported

- `requires name at the type level` — a `@ts-expect-error` compile-time case; `name: String` is
  non-optional in Rust, so there is nothing to assert at runtime.
- `throws when maxSearchResults is less than 1` for *negative* values — `usize` makes a negative
  limit unrepresentable; only the `0` case exists.
- `constructs a default runtime client when none is injected` / `uses the injected runtime client
  without constructing one` / `lazily constructs a default agent client` / `constructs a separate
  default runtime client per store` (4 cases) — TS asserts on a mocked SDK constructor. Rust has no
  seam to observe `aws_config::defaults(..).load()`, and exercising it would read ambient AWS
  config in a unit test.
- `exposes the boolean shorthand verbatim` — `ExtractionConfig` is a struct in Rust; the
  `true` shorthand is `ExtractionSetting`, covered by the extraction module's own tests.
- `unwraps NumericValue metadata back into the number that was stored` and the two related cases
  (3) — the Rust SDK returns `Document`, already typed, so there is no `NumericValue` wrapper to
  unwrap. See divergences.
- `config reuse across namespaces` — the two `MemoryManager` registration cases are `MemoryManager`'s
  behaviour and are covered in `memory_manager.rs`; the per-store scoped-filter case is covered by
  `reuses_one_connection_config_across_scopes`.
- `metadata logging` (1) and `extraction via MemoryManager` (1) — the debug line is observable only
  with a tracing subscriber, and the extraction path needs an agent-loop test. **Not yet written.**
- The remaining per-case duplicates inside the `search`, `score bounds`, and `add — S3` describes
  (e.g. `keeps a later result after dropping an earlier one`, `writes a sidecar built from caller
  metadata`, `omits unsupported metadata values from the sidecar`) assert a subset of what the
  folded Rust tests above already assert.

## Divergences

- `RetrievalFilter` / `FilterAttribute` are owned Rust enums converted to the SDK type at call
  time; TS passes the SDK type straight through. This keeps the public API free of an
  `aws-sdk-bedrockagentruntime` type.
- Metadata attribute values use a private owned `AttributeValue`, so one resolved list serves both
  the inline `CUSTOM` attributes and the JSON S3 sidecar (the Rust SDK type has no serde impl).
- No `NumericValue` unwrapping: the Rust SDK hands back a typed `Document`, mapped straight to
  `serde_json::Value`.
- `_sourceLocation` is emitted as `{ "type", "uri" }`; TS stores the raw SDK location object (the
  SDK struct has no serde impl).
- `add_document` returns the document id (uuid for `CUSTOM`, the `s3://` URI for `S3`);
  `MemoryStore::add` drops it, matching the trait.
- Agent and S3 clients are built lazily through a `OnceCell` on first use, so a read-only store
  never constructs them.
- Construction is fallible (`Result`) and `async`, not throwing.
- `BedrockKnowledgeBaseS3Config::prefix` is `Option<String>` where TS requires `prefix: string`:
  `None` uploads to the bucket root, and a set prefix gets the same trailing slash TS adds.
- `initialize` is the only `GetKnowledgeBase` caller a user needs; `search` resolves the type
  itself when `initialize` was not run.

## Review disposition (round 1)

| Finding | Disposition |
| --- | --- |
| 🔴 constructor shape inconsistent with siblings | fixed — `with_config(config)` + `new(knowledge_base_id)`; call sites and the API map updated |
| 🟠 incomplete root re-exports | fixed — `lib.rs` exports the same ten names as `memory/mod.rs` (the eight plus `AccessControlAccess`, `AccessControlEntry`) |
| 🟠 relocated `passes_score_bound` doc comment | fixed — moved back above the function |
| 🟠 `AccessControlEntry.access: String` | fixed — `AccessControlAccess { Allow, Deny }` with `as_str()` for the wire value |
| 🟠 duplicated `document_to_json` / `json_to_document` | fixed — one `pub(crate)` copy in `src/aws_document.rs` (gated `bedrock` or `bedrock-kb`), keeping `format.rs`'s fallible/sorted semantics; used by the model provider and the KB store |
| 🟠 missing `config reuse across scopes` test | fixed — `reuses_one_connection_config_across_scopes` asserts both `Retrieve` filters through a repeating mock rule |
| 🟠 stale "Part B not ported" line in this matrix | fixed — removed |
| 🟡 undocumented `prefix: Option` divergence | fixed — module `//!` doc and the divergence list |
| 🟡 `region` + a pre-built client both set should error | deferred — the `S3StorageBuilder` sibling it should mirror is not on this branch, and TS ignores `region` when a client is supplied; wants its own test alongside the mirrored message |
| 🟡 `#[non_exhaustive]` on the new public enums | deferred — no other public enum in `strands-agents` is `#[non_exhaustive]` except `interrupt`'s; a crate-wide call, not a memory-stores one |
