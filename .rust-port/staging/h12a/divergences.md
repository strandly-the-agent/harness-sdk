Row for `strands-rs/docs/DIVERGENCES.md`:

| `Agent` intervention handlers | No public accessor; the registry is the private `_interventionRegistry` | `Agent::interventions()` lists the handlers in registration order | Callers and the harness tests can check which handlers an agent runs without reaching into private state (P-1050) |
