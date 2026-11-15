| MCP client name sent in the handshake | `strands-agents-ts-sdk`, version `0.0.1` | `strands-agents-rs-sdk`, the crate version | Each SDK identifies itself (P-230) |
| MCP `url` given as a `URL` object | Accepted | `url` is a string | Rust has no `URL` union here |
| MCP `requestTimeout` / `ttl` | Only in `tasksConfig` (MCP tasks) | Not available; tasks are out of scope; the SDK 60 s default applies to every request | P-237 |
| Invalid MCP header names or values in `headers` | Fail when the request is sent | `McpClient::new` fails with `header=<name> \| invalid header name: ...` | Rust builds typed headers up front |
