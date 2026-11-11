# TS bugs found while porting

Facts only: `file:line`, what is wrong, minimal repro, how Rust handles it.

**`Uint8Array` fields serialize two ways.** `strands-ts/src/models/streaming.ts:446` (`ReasoningContentDelta.redactedContent: Uint8Array`) has no `toJSON`, so `JSON.stringify` writes it as an index-keyed object (`{"0":1,"1":2,"2":255}`), while the persisted `ReasoningBlock.toJSON` (`strands-ts/src/types/messages.ts:559`) writes base64 (`"AAEC/v8="`). Repro: `JSON.stringify({ type: 'modelContentBlockDeltaEvent', delta: { type: 'reasoningContentDelta', redactedContent: new Uint8Array([1, 2, 255]) } })`. Fixture: `strands-rs/conformance/fixtures/stream/model-stream-events.json`. Rust handling: accept both forms on read; persisted blocks write base64 and stream events write the index-keyed object TS writes (P-25).
