# Conformance fixtures

`generate.ts` drives the built TypeScript SDK (`strands-ts/dist/`, imported by relative path, not from npm) and writes JSON fixtures to `fixtures/`. Rust tests parse each fixture, serialize it back, and compare against the TS-written bytes, so the Rust wire and persisted formats stay identical to TypeScript. `fixtures/INDEX.md` lists every fixture and the TS API that produced it.

## Regenerate

From the repo root, after `npm ci` and `npm run build`:

```bash
npx tsx strands-rs/conformance/generate.ts
```

or `npm run generate` from this folder (it changes to the repo root and runs the same command). The generator deletes and rewrites `fixtures/`, makes no network calls, and leaves nothing else on disk. CI regenerates and runs `git diff --exit-code` on `fixtures/`, so commit regenerated fixtures together with any TS change that alters them.

## Determinism

`src/determinism.ts` is evaluated before strands-ts or `uuid` is imported and patches:

- `Date` and `Date.now` to a fixed instant, `2026-01-01T00:00:00.000Z` (`createdAt`, `updatedAt`, `startTime` and durations are constant).
- `performance.now` to return `0`.
- `crypto.randomUUID` to counter UUIDs `00000000-0000-4000-8000-<counter>` (tracking ids follow call order).
- `crypto.getRandomValues` to a seeded xorshift, which also makes `uuid` v7 snapshot ids deterministic.
- `Math.random` to the same xorshift (retry jitter, sandbox sentinels).
- Every `OTEL_*` environment variable is removed, so telemetry runs with the defaults unless a run sets one explicitly.

Two consecutive runs produce byte-identical output, including under a different `TZ`.

## Layout and byte conventions

One convention per fixture kind:

- **pretty** (everything except `persisted/` and `types/compact_message.json`): `JSON.stringify(value, null, 2)` plus a trailing newline. Values are the TS `toJSON()` output, so this is the wire form with TS key order. A file with named cases is an object of `case name -> value`; parse it into an ordered map and re-serialize pretty to compare bytes.
- **compact** (`persisted/**`, `types/compact_message.json`): the exact bytes TS handed to storage (`JSON.stringify(value)`, no trailing newline). Files under `persisted/<scenario>/` mirror the storage keys, for example `session/<sessionId>/scopes/agent/<agentId>/snapshots/snapshot_latest.json`.

Directories:

| Directory | Contents |
|---|---|
| `types/` | Messages, every content block, media formats, MIME maps, base64 leniency, JSON edge cases, strict-parser error texts, `ToolChoice`, stop reasons, `Usage` and `Metrics` |
| `events/` | `ModelStreamEvent` classes as `JSON.stringify` writes them |
| `agent/` | `Agent.stream()` events for scripted runs (text, tools, interrupts, structured output, redaction, model error) and the `StreamOptions` the model received |
| `hooks/` | Hook-event `toJSON()` examples and firing order |
| `interrupts/` | Interrupt state in snapshots and built directly (null response, `resume()`, `context`) |
| `snapshots/` | `takeSnapshot` presets and field selection, error texts, a multi-agent result |
| `persisted/` | Session snapshot, immutable history, manifest, multi-agent scope and stash bytes as written to storage |
| `stash/` | Stash record keys, bodies and `storageTypeName` |
| `schemas/` | `z.toJSONSchema` output, the stripped tool input schema and `ToolSpec` shapes |
| `telemetry/` | Metric instruments and spans captured with recording OpenTelemetry providers |

Fixtures marked "validity only" in the index are transcribed from a TS type rather than produced by TS code, so they pin JSON shape only.

## Sources

- `generate.ts`: entry point; applies the patches, then runs the sections in order.
- `src/scripted-model.ts`: a `Model` that replays scripted `ModelStreamEvent` turns and records `StreamOptions`.
- `src/types.ts`, `src/events.ts`, `src/persistence.ts`, `src/schemas.ts`, `src/telemetry.ts`: one section each.
