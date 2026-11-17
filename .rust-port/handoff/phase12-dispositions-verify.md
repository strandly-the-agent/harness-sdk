# Dispositions of the Phase 12 verification pass

| finding | disposition | commit |
|---|---|---|
| api-b-1 | Fixed: `HarnessError::InvalidEffort.supported` is a `Vec<String>` like the sibling lists; the message text is unchanged. Test `error::tests::an_invalid_effort_lists_the_levels_one_per_item`. | 89e0cb4db |
| api-b-2 | Fixed as proposed: the crate rustdoc and `AGENTS.md` name the public `builders` module and say every other module is private; the `AGENTS.md` layout lists `builders.rs`; its sentence on the MCP cases describes the default lookup of `node` and `node_modules` and the two overriding variables. | 97524bc3e |
| api-b-3 | Fixed for both crates: the divergence and parity rows state their reason themselves instead of citing a section of the run's internal brief (four harness rows, five SDK rows, one SDK parity row). | 97524bc3e |
| api-b-4 | The closure-only setter stays, as the first option of the finding: `.builder(..)` on `MakeSubagentOptions::builder()` takes an async closure (the TS shape `builder: (spec) => agent`), and a held `AgentBuilder` no longer goes in directly; it goes in as `move \|spec\| held(spec)`. Taking both through a marker trait loses the inference of the closure parameter, which is what api-19 asked for. The `AgentBuilder` alias rustdoc gives the wrap form and `tests/public_api.rs::make_subagent_takes_an_async_closure_or_a_held_agent_builder_wrapped_in_one` compiles both forms from outside the crate (P-1230; P-1167 has the first decision). | 8350a2ce9 |
| parity-b-1 | Fixed: the `models.test.ts` row of `PARITY.md` says the effort case is ported against `Effort`'s `FromStr`. | 97524bc3e |
