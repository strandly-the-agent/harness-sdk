# Parity rows, track h12plug (replacement rows for `harness-rs/docs/PARITY.md`)

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `harness-ts/test/plugins/todos.test.ts` | `harness-rs/strands-harness/src/plugins/todos/tests.rs` | 6/9 | `is enabled by default and contributes the todo_write tool`, `is disabled when not selected`, `throws on an unknown built-in plugin name`: need `create_harness` (the `create_harness` track ports them) |
