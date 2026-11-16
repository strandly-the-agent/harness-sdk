| finding | disposition | commit |
|---|---|---|
| correctness-1 | Fixed. `session_storage` is `LocalFileStorage::new(dir).with_namespace("")`, so every `dir` gives `<dir>/<id>/scopes/...` and a backslash in the directory name is accepted. Tests: `agent::tests::sessions::stores_a_session_directly_under_a_dir_with_no_final_component`, `stores_a_session_under_a_dir_whose_name_has_a_backslash`. | f9b1f12ba |
| correctness-7 | Fixed. A whole `maxDepth` up to `Number.MAX_SAFE_INTEGER` is accepted and held as `u32::MAX` when larger; above the safe range it is rejected, as zod's `int()` does (checked with zod at the pin). Test: `config::tests::holds_a_subagent_max_depth_above_u32_max_as_u32_max`. Replacement `DIVERGENCES.md` row staged. | 536f6c770 |
