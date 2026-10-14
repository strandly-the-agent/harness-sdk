# session module — TS → Rust test matrix

Worktree `/home/sandbox/wt/session`, branch `agent-tasks/strands-rs-session`, commit
`0a2c65ace feat(strands-rs): session (...)`.

**Scope correction (important):** the task brief described a `Session`/`SessionAgent`/`SessionMessage`
repository design (create/read/update session + agent + zero-padded message files). That design does
not exist in `strands-ts/src/session/` — the TS module is **snapshot**-based
(`SnapshotStorage`, `FileStorage`, `Snapshot`, `SnapshotManifest`, `SessionManager` with
`saveLatestOn`/`snapshotTrigger`), and `/mnt/workspace/survey/03c-1-session.md`,
`03c-2-session.md` and `03e-rust-notes.md` all describe that same snapshot design. Per COMMON.md
("TS source + its tests are the ground truth"), I ported the snapshot design. Names follow TS:
`FileStorage` (not `FileSessionStorage`), `SnapshotStorage` (not `SessionStorage`).

Files added (all under `crates/strands-agents/src/session/`):
`mod.rs`, `types.rs`, `validation.rs`, `storage.rs`, `file_storage.rs`, `session_manager.rs`,
`agent_snapshot.rs`, `test_support.rs` (`#[cfg(test)]` `MockSnapshotStorage`).
Touched outside the module: `src/lib.rs` (+`pub mod session` / `pub use`), `src/agent/mod.rs`
(appended `#[cfg(test)] test_agent_for_session`), workspace `Cargo.toml` (`uuid` gains the `v7`
feature; no new dependency).

Counts: **91 Rust tests in `session::`** (crate total 144, all passing).
TS `it(...)` in scope: 26 in `file-storage.test.node.ts`, 44 in `session-manager.test.ts`
(of which 20 are stash/multi-agent), 9 in `validation.test.ts`, 0 in a `types.test.ts` (none exists).

## validation.test.ts → `session::validation::tests`

| TS test | Rust test | Status |
| --- | --- | --- |
| validateIdentifier > returns the identifier | `returns_the_identifier` | pass |
| validateIdentifier > forward slash throws | `identifier_with_forward_slash_throws_error` | pass |
| validateIdentifier > backslash throws | `identifier_with_backslash_throws_error` | pass |
| validateScope > returns the scope | `returns_the_scope` | pass |
| validateScope > path traversal throws | `scope_with_path_traversal_throws_error` | pass |
| validateUuidV7 > valid v7 does not throw | `valid_uuid_v7_does_not_throw` | pass |
| validateUuidV7 > v4 (wrong version) throws | `uuid_v4_wrong_version_throws_error` | pass |
| validateUuidV7 > timestamp string throws | `timestamp_string_throws_error` | pass |
| validateUuidV7 > path traversal throws | `uuid_with_path_traversal_throws_error` | pass |
| validateUuidV7 > empty string throws | `empty_uuid_throws_error` | pass |
| — (added) | `rejects_uppercase_and_empty_identifiers` | pass |
| — (added: notes §"always generate lowercase UUIDs") | `uuid_v7_check_is_case_insensitive` | pass |

## file-storage.test.node.ts → `session::file_storage::tests`

| TS test | Rust test | Status |
| --- | --- | --- |
| saveSnapshot > saves snapshot to history file | `saves_snapshot_to_history_file` | pass |
| saveSnapshot > saves as latest when isLatest | `saves_snapshot_as_latest_when_is_latest_is_true` | pass |
| saveSnapshot > creates directories recursively | `creates_directories_recursively` | pass |
| saveSnapshot > throws SessionError when write fails | `throws_session_error_when_write_fails` | pass (base dir is a regular file instead of `vi.spyOn(writeFile)`) |
| saveSnapshot > rejects a scope containing path traversal | `rejects_a_scope_id_containing_path_traversal` | pass (adapted: `Scope` is a Rust enum, so an invalid scope is unrepresentable; the traversal case is exercised through `scope_id` + `validate_scope` in `validation::tests`) |
| saveSnapshot > saves multi-agent snapshot to correct path | `saves_multi_agent_snapshot_to_correct_path` | pass |
| loadSnapshot > loads latest when snapshotId undefined | `loads_latest_snapshot_when_snapshot_id_is_none` | pass |
| loadSnapshot > loads specific snapshot by ID | `loads_specific_snapshot_by_id` | pass |
| loadSnapshot > returns null when file missing | `returns_null_when_snapshot_file_does_not_exist` | pass |
| loadSnapshot > throws SessionError on invalid JSON | `throws_session_error_when_snapshot_json_is_invalid` | pass |
| loadSnapshot > throws SessionError when read fails | `throws_session_error_when_file_read_fails` | pass (snapshot path is a directory instead of `vi.spyOn(readFile)`) |
| listSnapshotIds > returns sorted snapshot IDs | `returns_sorted_snapshot_ids` | pass |
| listSnapshotIds > ignores non-snapshot files | `ignores_non_snapshot_files` | pass |
| listSnapshotIds > filters by startAfter | `filters_by_start_after_for_pagination` | pass |
| listSnapshotIds > limits results | `limits_results_when_limit_is_provided` | pass |
| listSnapshotIds > combines startAfter and limit | `combines_start_after_and_limit` | pass |
| listSnapshotIds > empty when directory missing | `returns_empty_array_when_directory_does_not_exist` | pass |
| listSnapshotIds > throws SessionError when readdir fails | `throws_session_error_when_readdir_fails_with_non_enoent_error` | pass (`immutable_history` is a regular file ⇒ ENOTDIR) |
| deleteSession > removes the session directory | `removes_the_entire_session_directory` | pass |
| deleteSession > no-ops when missing | `no_ops_when_session_directory_does_not_exist` | pass |
| deleteSession > throws SessionError when rm fails | `throws_session_error_when_rm_fails` | pass (session dir is a regular file ⇒ ENOTDIR) |
| saveManifest > saves manifest to correct path | `saves_manifest_to_correct_path` | pass |
| saveManifest > throws SessionError when write fails | `throws_session_error_when_manifest_write_fails` | pass |
| loadManifest > loads manifest from file | `loads_manifest_from_file` | pass |
| loadManifest > returns default when missing | `returns_default_manifest_when_manifest_file_does_not_exist` | pass |
| loadManifest > throws SessionError on invalid JSON | `throws_session_error_when_manifest_json_is_invalid` | pass |
| — (added: `limit <= 0`) | `returns_empty_array_when_limit_is_zero` | pass |
| — (added: `startAfter` must be a UUID v7) | `rejects_a_start_after_cursor_that_is_not_a_uuid_v7` | pass |
| — (added: `SNAPSHOT_REGEX` semantics) | `snapshot_regex_matches_the_js_pattern` | pass |
| — (added: 2-space pretty JSON, no trailing newline, `.tmp` removed) | `writes_pretty_json_without_a_trailing_newline` | pass |

## session-manager.test.ts → `session::session_manager::tests`

| TS test | Rust test | Status |
| --- | --- | --- |
| constructor > defaults saveLatestOn to invocation | `defaults_save_latest_on_to_invocation` | pass |
| saveSnapshot > saves snapshot_latest when isLatest | `saves_snapshot_latest_when_is_latest_is_true` | pass |
| saveSnapshot > saves immutable when !isLatest | `saves_immutable_snapshot_when_is_latest_is_false` | pass |
| saveSnapshot > allocates unique snapshot IDs | `allocates_unique_snapshot_ids` | pass |
| listSnapshotIds > empty when none exist | `returns_empty_array_when_no_snapshots_exist` | pass |
| listSnapshotIds > IDs for the target agent | `returns_snapshot_ids_for_the_target_agent` | pass |
| listSnapshotIds > does not return latest | `does_not_return_latest_snapshot_id` | pass |
| listSnapshotIds > forwards limit | `forwards_limit_parameter` | pass |
| listSnapshotIds > forwards startAfter | `forwards_start_after_parameter` | pass |
| restoreSnapshot > restores latest when no id | `restores_snapshot_latest_when_no_snapshot_id_provided` | pass |
| restoreSnapshot > restores specific snapshot by ID | `restores_specific_snapshot_by_id` | pass |
| restoreSnapshot > returns false when not found | `returns_false_when_snapshot_not_found` | pass |
| InitializedEvent > loads snapshot_latest on init | `loads_snapshot_latest_on_initialization` | pass |
| InitializedEvent > handles missing snapshot gracefully | `handles_missing_snapshot_gracefully` | pass |
| InitializedEvent > warns when restore overwrites messages | `warns_when_snapshot_restore_overwrites_existing_messages` | pass (asserts the overwrite; the `tracing::warn!` itself is not captured — no log-capture fixture in the crate yet) |
| InitializedEvent > does not warn with no messages | `does_not_warn_when_restoring_into_agent_with_no_messages` | pass (same caveat) |
| MessageAddedEvent > saves latest when saveLatestOn=message | `saves_snapshot_latest_when_save_latest_on_is_message` | pass |
| MessageAddedEvent > does not save when saveLatestOn=invocation | `does_not_save_on_message_added_when_save_latest_on_is_invocation` | pass (uses `hooks().count::<MessageAddedEvent>() == 0`) |
| AfterInvocationEvent > saves latest when saveLatestOn=invocation | `saves_snapshot_latest_on_after_invocation_when_save_latest_on_is_invocation` | pass |
| AfterInvocationEvent > saves latest when saveLatestOn=message | `saves_snapshot_latest_on_after_invocation_when_save_latest_on_is_message` | pass |
| AfterInvocationEvent > does not save when saveLatestOn=trigger | `does_not_save_snapshot_latest_when_save_latest_on_is_trigger` | pass |
| snapshotTrigger > immutable snapshot when trigger true | `creates_immutable_snapshot_when_trigger_returns_true` | pass |
| snapshotTrigger > no snapshot when trigger false | `does_not_create_immutable_snapshot_when_trigger_returns_false` | pass |
| snapshotTrigger > provides agentData to trigger | `provides_agent_data_to_trigger` | pass |
| snapshotTrigger > saves both immutable and latest | `saves_both_immutable_and_latest_when_trigger_fires` | pass (also asserts exactly 2 writes) |
| snapshotTrigger > trigger on message count | `trigger_based_on_message_count_via_agent_data` | pass |
| snapshotTrigger > trigger on agent state | `trigger_based_on_agent_state_via_agent_data` | pass |
| integration > handles complete session lifecycle | `handles_complete_session_lifecycle` | pass |
| integration > supports resuming from immutable snapshot | `supports_resuming_from_immutable_snapshot` | pass |
| AfterModelCall > saves latest on redaction (saveLatestOn=message) | `saves_snapshot_latest_when_save_latest_on_is_message_and_redaction_occurred` | pass |
| AfterModelCall > no save without redaction (message) | `does_not_save_when_save_latest_on_is_message_but_no_redaction_occurred` | pass |
| AfterModelCall > `it.each(['invocation','message'])` saves on redaction | `saves_snapshot_latest_on_redaction_for_each_auto_save_strategy` | pass |
| AfterModelCall > no hook registered when saveLatestOn=trigger | `does_not_register_after_model_call_hook_when_save_latest_on_is_trigger` | pass |
| AfterModelCall > no save without redaction (invocation) | `does_not_save_on_after_model_call_without_redaction_under_invocation` | pass |
| stash > includes stash data in the snapshot | — | **skipped**: `context-manager`/`Stash` not ported |
| stash > omits stash key when no context manager | — | **skipped**: same |
| stash > omits stash key when stash is empty | — | **skipped**: same |
| stash > writes external ref when storage is durable | — | **skipped**: same |
| stash > restores stash data from snapshot | — | **skipped**: same |
| stash > skips restore when location is external | — | **skipped**: same |
| stash > warns when stash storage type changed | — | **skipped**: same |
| stash > handles snapshot without stash data | — | **skipped**: same |
| stash > deleteSession × 6 (`deletes stash data via stash reference`, `… from custom stash storage`, `… when storage is pre-namespaced`, `does not over-delete when rootStorage is pre-namespaced`, `clearSession cleans up other agents`, `succeeds when no stash is available`) | — | **skipped**: `Stash` + generic `Storage`/`namespace` not ported |
| stash > round-trips stash data with real Agent | — | **skipped**: same |
| stash > deleteSession cleans up stash data with real Agent | — | **skipped**: same |
| stash > deleteSession keeps a stash rooted at a scoped view | — | **skipped**: same |
| stash > deleteSession with scoped agent storage removes only this session | — | **skipped**: same |
| initMultiAgent × 3, saveSnapshot — multi-agent × 3, restoreSnapshot — multi-agent, AfterNodeCall/AfterMultiAgentInvocation × 3, scope isolation, BeforeMultiAgentInvocation state restore × 6 | — | **skipped**: `multiagent/` is out of scope (DESIGN.md "Out") — `Scope::MultiAgent` and the `multiAgent` path segment *are* implemented and covered in `file_storage` tests |
| — (added) | `exposes_name_and_session_id` | pass |
| — (added) | `rejects_an_invalid_session_id` | pass |
| — (added: `model.stateful` purge, §4.3 step 5) | `discards_restored_messages_for_a_stateful_model` | pass |
| — (added) | `delete_session_removes_this_session_only` | pass |
| — (added) | `storage_errors_propagate_from_hooks` | pass |

## agent_snapshot.rs (`agent/snapshot.ts`, session preset only)

`captures_messages_state_and_system_prompt`, `round_trips_through_load`,
`null_system_prompt_clears_it`, `absent_keys_leave_fields_unchanged`,
`rejects_wrong_scope_and_schema_version` — all pass.

## types.rs

`snapshot_serializes_with_ts_field_names`, `manifest_serializes_with_ts_field_names`,
`multi_agent_scope_round_trips`, `iso8601_matches_date_to_iso_string`, `now_is_well_formed` — pass.

## Deviations from TS

1. **Names follow TS, not the task brief**: `FileStorage`/`SnapshotStorage`/`Snapshot`; no
   `Session`/`SessionAgent`/`SessionMessage` types exist in the TS source to port.
2. **Params objects → explicit args** (`03e-rust-notes.md`): `save_snapshot(&location, id,
   is_latest, &snapshot)`, `Option<usize>`/`Option<&str>` for `limit`/`start_after`.
   `SessionStorage = { snapshot }` is flattened away.
3. **`storage` is required at construction.** TS can fall back to `agent.storage` and wrap it in
   `SnapshotStorageAdapter`; the generic `Storage` layer and the adapter are out of scope, so
   `SessionManager::new(session_id, storage)` takes `Arc<dyn SnapshotStorage>` and the
   "SessionManager requires a storage backend" error paths do not exist.
4. **`Scope` is an enum**, so `validateScope` only runs on strings (`validate_scope(&str)`), and the
   file-storage "invalid scope" test moved to `validation`.
5. **Snapshot `data` keys**: only `messages`, `state`, `systemPrompt` are written. `modelState` and
   `interrupts` have no Rust equivalent yet; the keys are *omitted* rather than written as
   `{}`/`null` so a TS reader leaves its own values untouched (writing `modelState: {}` would clear
   them). `takeSnapshot` presets/`include`/`exclude`/`appData` are not ported — the session manager
   only ever uses the `session` preset.
6. **No stash integration** (`data.stash`, `_includeStashData`, `_restoreStashData`,
   `_deleteStashData`) and **no multi-agent** (`initMultiAgent`, `AfterNodeCallEvent`,
   `_multiAgentRestoredIds`): both depend on unported modules. The `multiAgent` scope/path layout is
   implemented.
7. **Timestamps**: no new dependency. `types::now_iso8601()` formats `SystemTime` as
   `YYYY-MM-DDTHH:MM:SS.mmmZ`, byte-identical to `Date.toISOString()` (tested against known epochs).
8. **`uuid` gains the `v7` feature** in the workspace `Cargo.toml` + already-present crate
   dependency. `Uuid::now_v7()` is lowercase, so it satisfies both `validate_uuid_v7` and
   `validate_identifier` (asserted in a test).
9. **Logging**: TS `logger.warn/debug` → `tracing::warn!/debug!` with the same message text; the two
   TS tests that spy on the logger assert the observable effect instead.
10. `validation` errors are `Error::Other` (TS throws plain `Error`); storage errors are
    `Error::Session` with the TS message text and the cause attached as `#[source]`.

## Stubbed / needed from other modules

- `agent_snapshot.rs` is a **temporary home** for `agent/snapshot.ts`
  (`take_agent_snapshot`/`load_agent_snapshot`, `pub(crate)`, not re-exported). When `agent/`
  grows `takeSnapshot`/`loadSnapshot` (with presets, `modelState`, `interrupts`), these should move
  there and `SessionManager` should call the `Agent` methods.
- `InitializedEvent` is never fired (no event loop yet). The callback is registered and
  `SessionManager::initialize(&agent)` is public so the loop — or a test — can drive the restore.
- Wave 2 should wire `AgentMetadata { session_id }` from `SessionManager::session_id()` into
  `StreamOptions`; nothing in this module does it.
