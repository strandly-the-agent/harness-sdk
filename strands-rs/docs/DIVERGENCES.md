# Divergences from strands-ts

User-visible behaviour that differs from strands-ts at the tracked spec commit (`7296caba7`, see [README.md](../README.md)), and why. A divergence never changes a cross-SDK wire or persisted format.

| Area | TS behaviour | Rust behaviour | Reason |
|---|---|---|---|
| Persisted JSON | Strings may hold lone UTF-16 surrogates; `JSON.stringify` writes them as `\ud83d` escapes | When the SDK reads persisted data, a lone surrogate escape becomes U+FFFD; the rest of the document loads | A Rust `String` is UTF-8 and can't hold a lone surrogate, and `serde_json` would reject the whole document |
| Persisted JSON and model tool input | `JSON.parse` nests until the stack runs out | Nesting deeper than 128 levels fails to parse (tool input fails with `unable to parse tool input JSON`) | `serde_json`'s recursion limit keeps a hostile document from overflowing the stack, which aborts the process rather than returning an error |
| JSON numbers | `JSON.parse` reads a number literal beyond the `f64` range (`1e400`) as `Infinity`, which `JSON.stringify` writes as `null` | The SDK reads such a literal as `null` | `serde_json` values can't hold `Infinity`; `null` is what TS persists for it |
| Interrupts | Raising an unanswered interrupt throws `InterruptError` (`Interrupt raised: <name>`) | No error type: the interrupt comes back as a value and becomes a tool outcome or an agent result | Interrupts are control flow, not failures; callers can't swallow one by catching errors |
| `ToolResultBlock` | Two blocks are compared field by field, including `error` | `PartialEq` ignores `error` | `error` is a runtime-only handle that is never serialized |
