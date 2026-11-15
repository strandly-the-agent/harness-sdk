## MCP

No live model tests were added; the MCP tests run against local servers (in-process `rmcp` servers and a Node stdio fixture), which need no credentials.

Stdio fixture run: 2026-11-15, `STRANDS_TEST_NODE=<node> STRANDS_TEST_NODE_MODULES=<repo node_modules> cargo test -p strands --features mcp --test mcp_stdio`: 5 passed, 0 failed.
