Track `p7router`: Phase 7 item 4 (`ModelRouter`). Read `.rust-port/briefs/run3-common.md` first and follow it.

Worktree `/mnt/workspace/wt-p7router` (branch `wip/p7router`), `CARGO_TARGET_DIR=/tmp/ct-p7router`, gate output `/mnt/workspace/gates-p7router`, staging dir `.rust-port/staging/p7router/`, decisions from P-450.

Scope (PROMPT.md section 12, Phase 7, the `ModelRouter` bullet; section 10 for the retry and plugin contracts): the `ModelRouter` plugin (TS `src/models/routing/`): `FallbackStrategy` and `ClassifierStrategy` (with `routingPolicy`; the deprecated `systemPrompt` is out), rejection of stateful models and duplicates, `max_switches`, the retry `attempt_count` reset when the router switches models, and `aux_model` rejecting routers. Agent wiring: the router is the first slot of the plugin registration order (router, conversation manager, retry strategies, user plugins, ...) and the model config accepts a router where TS does (`agent/agent.ts`); keep it small and in its own commit. All error texts and log texts match TS.

Specs: `models/routing/__tests__/{router,fallback-strategy,classifier-strategy}.test.ts` (67 cases) and `agent/__tests__/agent.model-routing.test.ts`.

Your paths: `strands-rs/strands/src/models/routing*`, tests beside the code and `strands-rs/strands/tests/model_routing*.rs`. Order: strategy trait and `FallbackStrategy`, `ClassifierStrategy`, the router plugin, agent wiring with the `agent.model-routing` cases, staging logs. Commit after each.

When that is done and time remains: port the `InterruptState` unit cases of `interrupt.test.ts` (14, listed as carried over in its `strands-rs/docs/PARITY.md` row) if the type exists in Rust, else make the gap reason exact.
