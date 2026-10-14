# telemetry (strands-rs) — Pass C

STATUS: merge of agent-tasks/strands-rs resolved + committed (6 conflicts, kept both sides;
restored the missing `src/bin/mcp_echo_server.rs` that the MCP pass never committed — it broke
`--all-features` on the integration branch). `cargo build --all-targets --all-features` clean.
NEXT: run full gates (clippy -D warnings, test --all-features, --no-default-features build,
strands-harness tests), then verify telemetry tests from pass A/B cover the span hierarchy.
