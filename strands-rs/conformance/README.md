# Conformance fixtures

`generate.ts` imports the built TS SDK (`strands-ts/dist`) and writes JSON that the Rust SDK must read and write identically. Each fixture file holds the exact bytes TS produced: `JSON.stringify` output (compact, no trailing newline), or the raw bytes TS wrote to storage. `fixtures/INDEX.txt` lists every file.

Regenerate from the repository root:

```bash
npm ci && npm run build
npm run generate --prefix strands-rs/conformance
npm run type-check --prefix strands-rs/conformance   # optional: checks the generator against the SDK types
```

The generator is deterministic: before importing the SDK it fixes the clock at `2026-11-11T12:00:00.000Z`, makes `performance.now()` return 0, replaces `crypto.randomUUID` with counter UUIDs and `crypto.getRandomValues` with a seeded xorshift (which also fixes UUID v7 ids).

| Directory | Contents |
|---|---|
| `messages/` | Every content block type and source variant, every media format, unknown blocks and formats (saved-data path), legacy data without `trackingId`, text edge cases (lone surrogates, control characters), JSON number formatting, non-canonical base64 input with TS's canonical output |
| `tools/` | `ToolChoice` variants, a `ToolSpec` |
| `stream/` | `ModelStreamEvent` JSON (including `Uint8Array` fields, which `JSON.stringify` writes as index-keyed objects, and null `additionalModelResponseFields`/`trace`), and every `AgentStreamEvent` from a mock-model tool loop |
| `hooks/` | `toJSON` of every hook event fired during that loop |
| `agent/` | Messages after the loop, snapshots with all fields and with a pending interrupt |
| `interrupt/` | Interrupt state with a null response, resume responses, a pending tool execution and `context`; a null interrupt response. `Interrupt.fromJSON` defaults a missing `source` to `hook` |
| `storage/` | The exact keys and bytes `SessionManager` wrote: `snapshot_latest.json`, `immutable_history/`, `manifest.json` (written through `SnapshotStorageAdapter.saveManifest`, since `SessionManager` doesn't write it), an inline stash, and a multi-agent-scope snapshot |
| `stash/` | Stash records as the context manager writes them, and the `storageType` name |
| `telemetry/` | Span names, kinds, attribute keys and event names; metric names, units and descriptions, recorded with in-memory OpenTelemetry providers during the loop |

The Rust tests that consume each fixture land with the code that implements its types.
