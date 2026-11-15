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
| Constructing an `UnknownBlock` | `new UnknownBlock(data)` accepts any record, including a known block's data (which reloads as that block) | No public constructor; an `UnknownBlock` comes only from deserializing data no known block matches | Keeps the block type stable across a save and load (see the TS bug list) |
| A hook callback that fails | A thrown error aborts the remaining callbacks and propagates out of the invocation | Callbacks return `()` and can't fail; a hook stops work through the event's control fields (`set_cancel`, `interrupt`) | Errors aren't control flow; hooks that need to fail an invocation cancel it with a message |
| Raising an interrupt from a hook | `event.interrupt(..)` throws, ending the callback | `event.interrupt(..)` returns `None` (or the stored response on resume) and records the interrupt; the callback keeps running and decides what to do next | No exceptions in Rust; collection and the duplicate-name error text are unchanged |
| Typed tool input that fails validation | Zod's error text (`Error: [{..issues..}]`) | `Error: Invalid input at /path: <jsonschema message>` or `Error: Invalid input: <serde message>` | Zod messages can't be reproduced; the result is still an error result the model sees |
| A defaulted field of a typed tool | Zod output-mode schemas list it in `required` | Optional in the schema (schemars); the default applies when absent | Validation runs before deserialization, so a required-in-schema field would reject input Zod accepts |
| JSON nesting depth on read (saved data, model tool input) | `JSON.parse` loads any depth | More than 500 levels of arrays/objects is a parse error; model tool input that deep is the `unable to parse tool input JSON` model error | Cloning and dropping deeper values can overflow a thread stack and abort the process |
| A content block after tool input that fails to parse | Becomes a tool use with the failed block's id and name (see the TS bug list) | Is its own block (the text is kept) | TS bugs are fixed in Rust (#74) |
| Structured output mechanisms a model rejected | Remembered process-wide for every provider | Remembered per provider instance; a new instance can repeat one failed request per mechanism | No process-global mutable state |
| `updateConfig`/`merge` with a field set to `undefined`/`None` | The spread unsets the field | `None` keeps the current value; replace the config to clear a field | `Option` can't tell "unset" from "absent" |
| A tool that panics | No equivalent; a throwing tool body becomes an error result with the error message | An error result whose text is the panic message; `AfterToolCallEvent` fires as for a failure | Rust panics are caught per tool so siblings and the invocation continue |
| Two tools of a concurrent batch interrupt | `InterruptEvent` fires for the last interrupted tool only (see the TS bug list) | The batch reports every interrupt | TS bugs are fixed in Rust (#74) |
