# Conformance fixtures

JSON produced by the TypeScript SDK, which the Rust tests use to check that both SDKs read and write the same data. Never edit the fixtures by hand.

## Regenerating

From the repository root:

```bash
npm ci
npm run build
npx tsx strands-rs/conformance/generate.ts
```

CI runs the same command and fails if `git diff --exit-code strands-rs/conformance/fixtures` shows a change or the run leaves an untracked fixture. Rerun it after any change to the TypeScript wire formats and commit the result.

The generator imports the built SDK from `strands-ts/dist`. Before importing it, it fixes `Date`, `performance.now`, `crypto.randomUUID` (counter UUIDs such as `00000000-0000-4000-8000-000000000001`), `crypto.getRandomValues` (a seeded xorshift, which also fixes uuid v7 ids) and `Math.random`, and removes `OTEL_*`, `LANGFUSE_*` and `STRANDS_*` variables from its environment. Each scenario resets those sources, so reruns are byte-identical. The clock reads `2025-01-02T03:04:05.678Z` unless a case says otherwise. Agent runs use an in-process scripted model; nothing touches the network.

## Layout

Each file is a JSON object of named cases. A case has a `description` and some of:

- `expected`: the value TypeScript produced, in TypeScript key order. Rust output must match it.
- `input`: what was fed to TypeScript to get `expected` (for round trips, deserialize `input`, serialize, compare with `expected`).
- `expectedText`: the exact compact text `JSON.stringify` wrote, for cases where the bytes matter (number formatting, escapes).
- `expectedError`: the error message TypeScript threw.
- `shapeOnly: true`: TypeScript declares this value only as a type, so the case proves the JSON shape and nothing about TypeScript runtime behaviour.

| File                   | Contents                                                                                                                                                    |
| ---------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `messages.json`        | Messages and metadata, system prompts, stop reasons, limit stop messages, `Usage` and `Metrics`.                                                            |
| `content-blocks.json`  | Every content block type, media and citation variants, tool result content including `JsonBlock`, unknown-block passthrough.                                |
| `tools.json`           | `ToolSpec` from function and zod tools, `ToolChoice`, `ToolStreamEvent`.                                                                                    |
| `stream-events.json`   | Every `ModelStreamEvent`, and every `AgentStreamEvent` `toJSON` from real agent runs (one with a tool, one with an interrupt).                              |
| `hook-events.json`     | Every hook event `toJSON`, from a run with a retried model error and a failing tool, plus events built directly.                                            |
| `snapshots.json`       | Agent snapshots (presets, include and exclude, app data, millisecond `createdAt`), multi-agent snapshots, error texts.                                      |
| `interrupts.json`      | Interrupt state from tool, hook and agent-as-tool interrupts, a `null` response, resume from a restored snapshot.                                           |
| `session-layout.json`  | Storage keys and values written by `SessionManager` (latest, immutable history, manifest, multi-agent scope, the `LocalFileStorage` layout), id validation. |
| `stash.json`           | Stash keys and records, `storageType` names, inline and external stash data in session snapshots.                                                           |
| `telemetry-names.json` | Spans for each semantic-convention mode and for a graph, metric instruments with units, `AgentMetrics`.                                                     |
| `edge-cases.json`      | Nulls that differ from missing fields, number formatting, surrogates, string escapes, non-canonical base64, redacted content, every media format.           |

## Gaps and quirks

- `ToolSpec` (beyond what tools generate), `ToolChoice`, `Usage` and `Metrics` are TypeScript types with no runtime constructor, so those cases are `shapeOnly`.
- `lone_surrogates` has only `expectedText`: serde_json and Rust strings cannot hold lone surrogates, so an `expected` value would make the file unparseable.
- `model_content_block_delta_reasoning_redacted` records that `JSON.stringify` writes a `Uint8Array` delta as an object keyed by index.
- Span `exception.stacktrace` attributes are cut to their first line, because stack frames name local paths.
- `InterruptState`, `Stash`, `SnapshotStorageAdapter`, the validators and other internals are not exported from the package entry points; the generator imports them from their `dist` modules.
