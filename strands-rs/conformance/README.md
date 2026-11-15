# Conformance fixtures

`generate.ts` builds real strands-ts objects from the compiled SDK (`strands-ts/dist`) and writes their exact `JSON.stringify` output to `fixtures/`. Rust tests parse a fixture, serialize it again and compare the bytes.

Regenerate from the repo root:

```bash
npm ci && npm run build
(cd strands-rs/conformance && npm run generate)
```

CI regenerates the fixtures and runs `git diff --exit-code` on `fixtures/`, so a TS wire change shows up as a diff.

The generator is deterministic: before strands-ts loads it fixes `Date`, `Date.now` and `performance.now` to a scripted clock starting at `2026-11-11T12:00:00.123Z`, replaces `crypto.randomUUID` with counter UUIDs, `crypto.getRandomValues` and `Math.random` with a seeded xorshift (which also fixes uuid v7), drops stack traces, and clears `OTEL_*` and `LANGFUSE_*` variables. It needs no network and no credentials.

## Layout

- `fixtures/manifest.json` lists every fixture with a description. An entry with `_note` says which part of the shape the generator chose rather than TS (for example a list the generator assembled, or a record wrapping OpenTelemetry data).
- `<topic>/<case>.json` holds one object. `<case>.input.json` next to it is the input TS parsed to produce `<case>.json`.
- `sessions/<case>/` mirrors the storage keys a `SessionManager` wrote, byte for byte (`session/<sessionId>/scopes/<scope>/<scopeId>/snapshots/...` and the stash under `context/...`).

## Shapes TS doesn't produce at runtime

- `ModelStreamEvent` classes have no `toJSON`; their fixtures are `JSON.stringify` of the instance.
- `ToolChoice` `auto` and `any` are only built inside provider request formatting, so `tools/tool_choice_{auto,any}.json` are literals that prove the JSON shape. The `tool` variant also appears in `model_requests/structured_output_run.json` from a real run.
- The `StopReason` values and the audio, video and document format lists are TS type unions with no runtime list; the generator reads them from the TS source with the TypeScript compiler API and builds a real TS object for each value. Images use the runtime `IMAGE_FORMATS`.
- `ToolResultBlock.error` is never serialized, so `content_blocks/tool_result_error.json` has no error field.
- Checkpoints (experimental, `checkpointing`) and the deprecated `FileStorage` session layout are not covered.
