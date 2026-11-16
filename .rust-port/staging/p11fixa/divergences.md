# p11fixa staged DIVERGENCES.md rows

Replace row 137 (`Per-agent skills of one AgentSkills shared by several agents`, P-383) with:

| `AgentSkills::get_available_skills` argument | The agent object | The agent's id; when several live agents share the id, the skills loaded last are returned. The plugin's hooks and its `skills` tool read the agent's own entry | The caller holds an `Agent`, a `LocalAgent` or neither (P-383, P-741) |

Add:

| A call dropped inside its own `AfterInvocationEvent` hooks | Can't happen: a broken `for await` drains the generator's `finally` | The next `invoke`, `stream`, direct tool call or `shutdown()` runs every `AfterInvocationEvent` hook again from the first, with a cancelled signal | A dropped future stops at any `await` (P-98, P-740) |
