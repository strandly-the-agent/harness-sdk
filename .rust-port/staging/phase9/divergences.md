| MCP client name sent in the handshake | `strands-agents-ts-sdk`, version `0.0.1` | `strands-agents-rs-sdk`, the crate version | Each SDK identifies itself (P-230) |
| MCP `url` given as a `URL` object | Accepted | `url` is a string | Rust has no `URL` union here |
| MCP `requestTimeout` / `ttl` | Only in `tasksConfig` (MCP tasks) | Not available; tasks are out of scope; the SDK 60 s default applies to every request | P-237 |
| Invalid MCP header names or values in `headers` | Fail when the request is sent | `McpClient::new` fails with `header=<name> \| invalid header name: ...` | Rust builds typed headers up front |
| MCP progress token | Sent only when a progress callback is set | `rmcp` sends a `progressToken` on every request | `rmcp` assigns tokens itself (P-244) |
| MCP progress callback failures | A throw or an async rejection is caught and logged | A panic is caught and logged; callbacks are synchronous | Rust callbacks are `Fn`, not async |
| MCP tools-list change in an agent | Registry updated as the notification is handled | Applied before the agent's next model call | The agent owns its registry (P-245) |
| MCP trace context without telemetry | Injected from the active OpenTelemetry span | Injected only with the `otel` feature, from the current `tracing` span's OpenTelemetry context, with the W3C propagator | Telemetry is opt-in in Rust (P-247) |
| `loadServers` regex filters | JavaScript `RegExp` syntax; an invalid pattern reports the JS `SyntaxError` text | `regex` crate syntax (no look-around or backreferences); the message keeps the TS prefix `Server "<name>": invalid regex in toolFilters.<key>: "<pattern>": SyntaxError: ` followed by the `regex` error | No JS regex engine in Rust |
| `loadServers` entry with `tasksConfig` | Validated and used for MCP tasks | Fails that entry with `MCP tasks ("tasksConfig") are not supported in Rust yet` | MCP tasks are out of scope for v0.1 |
| `loadServers` file errors | Node `ENOENT` / `JSON.parse` messages | `std::io` / `serde_json` messages | Platform error text |
