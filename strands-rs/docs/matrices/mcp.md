# MCP client (strands-rs) — Pass A

STATUS: `src/mcp/{mod,client,config,tool}.rs` written and compiling under `--all-features`
(rmcp 3.5.1 behind feature `mcp`, default ON). Unit tests in-module (config parsing/interpolation,
transport resolution incl. SSE rejection, filter regex rules, prefix sanitizing, result mapping).
NEXT: `src/bin/mcp_echo_server.rs` (rmcp server, feature `mcp-echo-server`), `tests/mcp_e2e.rs`,
`AgentBuilder::mcp_clients`, then gates (fmt/clippy/test/no-default-features).
