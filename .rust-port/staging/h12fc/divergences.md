# Divergence rows, track h12fc

Rows for `strands-rs/docs/DIVERGENCES.md` (SDK):

| Area | TS behaviour | Rust behaviour | Reason |
|---|---|---|---|
| Several agents on one `McpClient` (`tools/list_changed`) | `Agent.initialize` assigns the client's single `onToolsChanged`, so only the last agent to initialize follows a tools-list change, and a consumer callback set earlier is replaced | Every live agent that listed the client's tools applies the change before its next model call; a dropped agent is removed from the client; a callback set with `set_on_tools_changed` is left in place and called too | TS bug (see the TS bug list): a delegate on the parent's client stopped the parent's refresh (P-1140) |
| Background task that asks for input after the model ended its turn, once the caller answers (`waitForCompletion` on) | The model is called with a history ending in its own assistant message; its reply is a second assistant message in a row, and the task result follows on a later call | No model call until the resumed task settles (or asks again); its result is delivered first, so the request ends in the delivered tool result. With `wait_for_completion: false` the sequence is the TS one | TS bug (see the TS bug list); the request sequence is the one an uninterrupted run produces (P-1141) |
