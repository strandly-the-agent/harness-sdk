| `builtin-tools.ts` `mcpClientsByClientName` | The unnamed-client warning says `application_name` where TS says `applicationName` (P-953). |
| `builtin-tools.ts` `webSearchMode` | `web_search: 'exa'` without the `mcp` feature is `HarnessError::FeatureDisabled` (P-951). |
| `agent.ts` `createHarness` | Built-in plugins and an existing `AgentSkills` are recognised by plugin name, not type (P-958). |
| `agent.ts` `createHarness` | `caching: 'auto'` passed explicitly behaves as not passed (warns, never errors) (P-957). |
| `agent.ts` `createHarness` | `mcp_servers` without the `mcp` feature is an error (P-961). |
