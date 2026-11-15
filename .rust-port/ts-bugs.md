# TS bugs found while porting

Format: `**B-N. Title.** file:line. What is wrong. Minimal repro. How Rust handles it.`

**B-1. `toMimeType` and `toMediaFormat` resolve `Object.prototype` keys.** `strands-ts/src/mime.ts` (`toMimeType`, `toMediaFormat`). Both index a plain object literal, so inherited keys match. Minimal repro: `toMimeType('constructor')` returns the `Object` function and `toMediaFormat('tostring')` returns `undefined` but `toMediaFormat('__proto__')` returns `Object.prototype`, not `undefined`. Rust matches on known strings and returns `None` (tested in `mime.rs`).

**B-2. Stream events and persisted blocks encode the same bytes differently.** `strands-ts/src/models/streaming.ts` (`ReasoningContentDelta.redactedContent: Uint8Array`) versus `src/types/messages.ts` (`ReasoningBlock.toJSON`). `JSON.stringify` of a stream event writes the `Uint8Array` as an index-keyed object (`{"0":0,"1":127}`), while the aggregated block writes base64. Minimal repro: `JSON.stringify(new ModelContentBlockDeltaEvent({ type: 'modelContentBlockDeltaEvent', delta: { type: 'reasoningContentDelta', redactedContent: new Uint8Array([1]) } }))` gives `{"0":1}` for `redactedContent`. Rust writes each form where TS does and reads both (P-16).

**B-3. `ModelContentBlockDeltaEvent.contentBlockIndex` is never set.** `strands-ts/src/models/streaming.ts` (`ModelContentBlockDeltaEvent`). The class declares `readonly contentBlockIndex?: number`, but the constructor ignores it and the data interface has no such field, so it is always `undefined`. Rust leaves it out (P-24).
