Replace the row `Bedrock user agent` (`DIVERGENCES.md:55`):

| Bedrock user agent | `<customUserAgent> strands-agents-ts-sdk` | App name `strands-agents-rs-sdk`, or the `SdkConfig` app name when one is set | `AppName` can't hold a space; each SDK identifies itself, with the name the MCP handshake uses (P-158, P-803) |

Replace the row `` `Ask::Stdio` prompts of several `HumanInTheLoop` handlers `` (`DIVERGENCES.md:182`):

| `Ask::Stdio` prompts of several `HumanInTheLoop` handlers | Each handler queues its own prompts; two handlers open two `readline` interfaces on one stdin | One queue for the process; the line is read on a detached thread. A prompt dropped while waiting leaves its read for the next prompt, which adopts it only while no line has arrived: a line typed for the dropped question is discarded | Stdin is per process, and a Rust caller can drop an invocation mid-prompt, which TS can't (P-642, P-798) |

New rows:

| Cancelling an invocation while the model stream is silent (time to first token, a provider retry sleep) | The invocation ends when the provider's stream honours the abort signal; a model that ignores it keeps the invocation waiting | The invocation ends at once with `Cancelled`; the model stream is dropped, which drops its request | Dropping a future is how Rust cancels it, and a cancel must not wait on a provider (P-799) |
| An MCP tool call whose invocation is dropped (a timeout around `invoke`, a dropped `AgentStream`) | No analog: a TS call always runs to its result or its abort signal | The client sends `notifications/cancelled` for the open request and removes its progress route | Every drop is a cancel (P-91, P-797) |
| A memory search, add or injection whose invocation is dropped mid store call | No analog: the awaited store call finishes and the span ends with its result | The `memory.search`, `memory.add` and `memory.inject` spans end with status OK and no result attributes | A dropped future can't report a result; the span still gets its end bookkeeping (P-801) |
| Text of a Bedrock error that has no modeled message | The AWS SDK error's `message` | The error and its sources joined with `: ` (for example `dispatch failure: io error: ...`) | The Rust SDK error has no single message; its `Debug` form, which holds the raw response, is left out (P-802) |
| Key order inside nested objects of MCP JSON sent to the model (`annotations`, `icons` of a `resource_link`) | The MCP TypeScript SDK's schema order | `rmcp`'s struct order; top-level keys of tool schemas and of `resource_link` items match TS | Only the top-level orders are reproduced (P-804) |
