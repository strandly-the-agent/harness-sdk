# p6fixb parity rows

Replaces the row `strands-ts/src/agent/__tests__/agent.context-manager.test.ts`:

| `strands-ts/src/agent/__tests__/agent.context-manager.test.ts` | `strands::experimental::context_manager::tests`, `strands::agent::run::tests::context_manager` | 16/16 | Titles that repeat under several `describe` blocks carry the block name (`when_auto_uses_null_conversation_manager`, `when_false_does_not_set_context_manager`). `when "agentic" > creates a ContextManager instance` and `registers the instance as a plugin` are in `agent::run::tests::context_manager`; the latter checks that the caller's handle was initialised, since the plugin `Arc` wraps a clone of the handle (P-625) |

New row for the in-scope table; delete the row `strands-ts/src/vended-plugins/context-offloader/__tests__/search.test.ts` from the out-of-scope table:

| `strands-ts/src/vended-plugins/context-offloader/__tests__/search.test.ts` | `strands::experimental::context_manager::search::tests` | 23/26 | the 3 `isSearchableContent` cases: `retrieve_context` only calls `isSearchableContent('text/plain')`, which is always true (`context-manager/retrieval-tool.ts:84`), so Rust has no content-type check; the other caller is the out-of-v0.1 ContextOffloader plugin |

The rows of `context-manager.test.ts` (19/20), `presets.test.ts` (16/17), `stash-integration.test.ts` (17/17), `offload-strategy.test.ts` (52/54), `summarize-strategy.test.ts` (54/55), `session-manager.test.ts` (53/71), `in-memory-storage.test.ts` (23/25) and `local-file-storage.test.node.ts` (23/31) keep their counts; their gaps are now declared in `docs/test-title-map.json` and the checker passes for them.
