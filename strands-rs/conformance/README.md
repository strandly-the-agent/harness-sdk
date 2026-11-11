# Conformance fixtures

`generate.ts` imports the built TS SDK (`strands-ts/dist`) and writes JSON that the Rust SDK must read and write identically. Each fixture file holds the exact bytes TS produced: `JSON.stringify` output (compact, no trailing newline), or the raw bytes TS wrote to storage. `fixtures/INDEX.txt` lists every file.

Regenerate from the repository root:

```bash
npm ci && npm run build
npm run generate --prefix strands-rs/conformance
npm run type-check --prefix strands-rs/conformance   # CI runs this too; a media format list that drifts from TS fails here
```

The generator is deterministic: before importing the SDK it fixes the clock at `2026-11-11T12:00:00.000Z`, makes `performance.now()` return 0, replaces `crypto.randomUUID` with counter UUIDs and `crypto.getRandomValues` with a seeded xorshift (which also fixes UUID v7 ids).

| Directory | Contents |
|---|---|
| `messages/` | Every content block type and source variant, every media format, unknown blocks and formats (saved-data path), legacy data without `trackingId`, text edge cases (lone surrogates, control characters), JSON number formatting (including `-0` and `1e20`), non-canonical base64 input with TS's canonical output |
| `tools/` | `ToolChoice` variants and a `ToolSpec`. These are `JSON.stringify` of typed literals (no SDK code runs on them), so they check the wire shape, not SDK behaviour |
| `stream/` | `ModelStreamEvent` JSON built through the SDK event classes (including `Uint8Array` fields, which `JSON.stringify` writes as index-keyed objects, and null `additionalModelResponseFields`/`trace`); every `AgentStreamEvent` from a mock-model tool loop, and from a run with a streaming tool and a tool interrupt (`toolStreamUpdateEvent`, `interruptEvent`) |
| `hooks/` | `toJSON` of every hook event fired during those two runs |
| `agent/` | Messages after the loop, snapshots with all fields and with a pending interrupt |
| `interrupt/` | Restored interrupt state with a pending tool execution and `context`, answered through `resume()` (one null response); a null interrupt response. `Interrupt.fromJSON` defaults a missing `source` to `hook` |
| `storage/` | The exact keys and bytes `SessionManager` wrote: `snapshot_latest.json`, `immutable_history/`, `manifest.json` (written through `SnapshotStorageAdapter.saveManifest`, since `SessionManager` doesn't write it), an inline stash, and a multi-agent-scope snapshot written by the TS multi-agent `takeSnapshot` |
| `stash/` | Stash records as the context manager writes them, and the `storageType` name |
| `telemetry/` | Span names, kinds, attribute keys and event names from the loop (`spans.json`) and from memory injection, extraction, search and add (`spans-memory.json`); metric names, units and descriptions of every instrument, including tool errors, model latency and time to first token. Recorded with in-memory OpenTelemetry providers |

The Rust tests that consume each fixture land with the code that implements its types.
