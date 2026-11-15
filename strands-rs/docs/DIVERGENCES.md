# Divergences from the TypeScript SDK

This file lists every user-visible behaviour difference between the Rust SDK (`strands`) and the TypeScript SDK, with the reason for it. The TypeScript SDK (`strands-ts/`) is the specification, so a difference is recorded here only when Rust can't or shouldn't match it, for example where the language rules out the TypeScript mechanism. Differences in API shape that follow Rust idiom (builders, `Result`, `snake_case` names) aren't listed. A fix never changes a cross-SDK wire or persisted format: data written by either SDK keeps loading in the other.

| Area | TypeScript | Rust | Reason |
|---|---|---|---|
| JSON strings | A string can hold a lone UTF-16 surrogate, written as `\ud83d` | A lone surrogate escape fails to parse; load paths replace it with U+FFFD before parsing | Rust strings are valid UTF-8 and can't hold a lone surrogate |
| JSON nesting | No nesting limit beyond the call stack | Parsing stops at 128 nested arrays or objects | `serde_json`'s recursion limit guards against stack overflow |
| Counters | `Usage`, `Metrics` and citation positions are `number` | `u64`; a fractional `Usage` or `Metrics` value fails to load, and a citation location with a fractional position loads as `CitationLocation::Other`, written back unchanged | Integer types keep integers free of a `.0` suffix; TS never writes fractions there |
| Large JSON integers | An integer above 2^53 in loaded JSON is rounded to the nearest double (`9007199254740993` is saved back as `9007199254740992`) | Integers up to the 64-bit range keep every digit when loaded and saved | Rust parses JSON integers into `u64`/`i64`; data TS wrote never holds such an integer, so TS-written bytes round-trip unchanged |
| Unknown blocks | `new UnknownBlock(data)` accepts any object, so one built from `{"text":..}` loads back as a `TextBlock` | `UnknownBlock::new` returns `None` when the data holds a known content-block or tool-result-content key | A block that changes type on a round trip is a bug, not behaviour to keep |
| MIME lookup | `toMimeType('constructor')` returns an object-prototype function | Returns `None` | TS looks up a plain object literal, so prototype keys match |
| Concurrent invocation | A second `invoke` while one runs throws `ConcurrentInvocationError` | No such error; `invoke` borrows the agent mutably, so a concurrent call doesn't compile | The borrow checker rules the case out; callers who share an agent wrap it in `Arc<Mutex<Agent>>` |
| Tool result errors | `ToolResultBlock.error` holds the thrown value, checked with `instanceof` | `error()` returns `&(dyn std::error::Error + Send + Sync)`; finding the concrete type takes a downcast | A tool can fail with any error type, not only `strands::Error` |
