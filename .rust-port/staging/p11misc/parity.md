# Replacement PARITY rows (track p11misc)

Each row replaces the `strands-rs/docs/PARITY.md` row with the same first column.

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `strands-ts/src/agent/__tests__/agent.stateful-model.test.ts` | `strands::agent::run::tests::stateful_model` | 12/12 | the `SessionManager` cases use an `InMemoryStorage` in place of TS `MockSnapshotStorage` and read the stored latest snapshot by its key. The private-field checks read a crate-internal `Agent::conversation_manager()` |
| `strands-ts/src/agent/__tests__/agent-as-tool.interrupt.test.ts` | `strands::agent::agent_as_tool::tests::interrupt` | 15/15 | the session cases use `SessionManager` over one `InMemoryStorage` (TS `MockSnapshotStorage`); the `logger.error` assertion of `logs instead of re-raising ...` checks the error result instead |
