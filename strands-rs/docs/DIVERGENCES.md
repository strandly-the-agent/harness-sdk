# Divergences from TypeScript

Every user-visible behaviour difference from the TypeScript SDK, with the reason. A divergence never changes a cross-SDK wire or persisted format.

| Area | TS behaviour | Rust behaviour | Reason |
|---|---|---|---|
| Message tracking ids | `crypto.randomUUID()` (UUID v4) | UUID v7 | Time-ordered ids from the `uuid` crate; any non-empty string is still accepted |
| Lone UTF-16 surrogates in saved JSON (`"\ud83d"`) | Kept in the string | The SDK's readers replace each with U+FFFD; `serde_json::from_str` rejects them | Rust strings are UTF-8 and can't hold lone surrogates |
| `Uint8Array` fields saved as `JSON.stringify` index objects (`{"0":1}`) | Re-encoded as the base64 of `"[object Object]"` | Read as the original bytes, written as base64 | The TS behaviour corrupts the bytes (see the TS bug list) |
| Serializing a single block struct (`AudioBlock`, `ToolUseBlock`, ...) | `block.toJSON()` returns the wrapped form `{"audio": ..}` | The struct serializes its inner object; `ContentBlock` adds the wrapper | Wire JSON of messages and content blocks is identical; only standalone block serialization differs |
| Pending interrupt message saved without `trackingId` | Raw data kept as saved | Parsed as a `Message`, so a tracking id is minted and written back | Interrupt state is typed |
| Fractional `latencyMs`/`timeToFirstByteMs` | Kept as a fraction | Rounded to whole milliseconds on load | Counters are integers so they never gain `.0` |
| `latencyMs: null` in saved metrics | Kept as `null` | Loads as `0` | `latency_ms` is a required integer; the message still loads (a `null` `timeToFirstByteMs` loads as missing, as in TS) |
| Strict parsing of a malformed known block | Throws whatever the block constructor throws (often a `TypeError`) | `Error::Other` with the serde message | Only the unknown-block texts are part of the contract |
| `SandboxTimeoutError` constructor | Takes seconds as a number | Takes a `Duration`; the message text is the same | Rust-native durations |
| `StateStore.set` with a value JSON can't hold | Throws `JsonValidationError` naming the path of the function, symbol or `undefined` | Returns `JsonValidationError` with `value for key "<key>" cannot be serialized: <reason>` | Rust values can only fail inside their `Serialize` impl, which has no path |
