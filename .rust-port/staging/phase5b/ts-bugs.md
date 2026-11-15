# TS bugs from Phase 5 item 2 (`AnthropicModel`)

None found. The TS behaviours that look odd and were kept: a thinking-binding or structured-output retry restarts the stream (TS `yield* this.stream(..)`), so events of the failed first attempt are not withdrawn; the failure happens before the first event in practice (a 400 at request time).
