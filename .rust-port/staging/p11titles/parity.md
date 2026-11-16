# PARITY rows to replace (track p11titles)

Each row replaces the `strands-rs/docs/PARITY.md` row with the same first column.

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `strands-ts/src/hooks/__tests__/events.test.ts` | `strands::hooks::events::tests`, `strands::types::agent::stream_event::tests` | 68/71 | `toJSON serialization completeness` (2) reflects over TS instance keys, which Rust records don't have (their fields are fixed by type); `allows mutating toolUse fields in-place` (1) does not apply: `ToolUse` fields are private and the event hands out no `&mut ToolUse`; a hook replaces the whole value with `set_tool_use` |
| `strands-ts/src/models/__tests__/model-proxy.test.ts` | `strands::models::model_proxy::tests` | 18/19 | `toBe` identity checks compare by value; `does not change the messages it was given` (1) does not apply: a `ModelRequest` owns its messages, so the proxy has no caller copy to change |
| `strands-ts/src/registry/__tests__/tool-registry.test.ts` | `strands::tools::registry::tests` | 30/33 | `throws ToolValidationError for a non-string tool name`, `throws ToolValidationError for an invalid description` (a number), `allows a tool with a null or undefined description` (3): `Tool::name`/`description` return `&str` |
