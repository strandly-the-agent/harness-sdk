# `plugins::vended` — `AgentSkills` + `ContextInjector` (strands-rs)

Branch `agent-tasks/strands-rs-skills`. Files added:
- `crates/strands-agents/src/plugins/vended/mod.rs`
- `crates/strands-agents/src/plugins/vended/skills.rs` (impl + 33 tests)
- `crates/strands-agents/src/plugins/vended/context_injector.rs` (impl + 12 tests)
- edits: `plugins/mod.rs`, `lib.rs` (exports), `agent/mod.rs` (`mod test_fixtures` → `pub(crate) mod`, 1 line)

Gates (last line each):
```
cargo fmt                                                   -> (no output)
cargo clippy --all-targets --all-features -- -D warnings    -> Finished `dev` profile [unoptimized] target(s) in 11.89s
cargo test --all-features                                   -> test result: ok. 595 passed; 0 failed; 0 ignored (lib) + 0 failed across all suites
```
45 new tests, all passing.

## Test matrix (TS `it(...)` → Rust test)

### `skills/__tests__/agent-skills.test.node.ts`
| TS test | Rust test | Status |
| --- | --- | --- |
| resolves Skill instances directly | `resolves_skill_instances_directly` | pass |
| resolves a skill directory path | `resolves_a_skill_directory_path` | pass |
| resolves a parent directory with multiple skills | `resolves_a_parent_directory_with_multiple_skills` | pass |
| handles mixed sources | `handles_mixed_sources` | pass |
| warns on duplicate names and keeps the last | `duplicate_names_keep_the_last` | pass (log assertion dropped; no log-capture harness) |
| warns and skips non-existent paths | `skips_non_existent_paths` | pass |
| gracefully handles a path with malformed SKILL.md | `handles_a_path_with_malformed_skill_md` | pass |
| loads valid skills from a parent dir containing malformed siblings | `loads_valid_skills_from_a_parent_dir_with_malformed_siblings` | pass |
| has the correct name | `has_the_correct_plugin_name` | pass |
| exposes the `skills` tool (name/description/schema) | `exposes_one_skills_tool_with_the_expected_spec` | pass |
| `setAvailableSkills` resets the set | `set_available_skills_replaces_the_set` | pass |
| activation returns instructions + metadata + resources | `activating_a_skill_returns_instructions_metadata_and_resources` | pass |
| unknown skill returns a not-found message | `activating_an_unknown_skill_returns_a_message_listing_available_skills` | pass |
| activation order dedups / moves to end | `activation_tracking_dedups_and_moves_to_the_end` | pass |
| skill with no instructions | `a_skill_without_instructions_reports_that` | pass |
| resource listing truncation at `maxResourceFiles` | `resource_listing_is_capped` | pass |
| system-prompt injection appends | `injects_appends_to_a_string_prompt_and_replaces_on_reinjection` | pass |
| re-injection replaces the previous block | same test (second `BeforeInvocationEvent`) | pass |
| injects bare XML with no system prompt | `injects_bare_xml_when_there_is_no_system_prompt` | pass |
| re-appends with a warn when previous XML is gone | `re_appends_when_the_previous_injection_is_gone` | pass (warn not asserted) |
| `SystemContentBlock[]` prompt branch | `injects_into_a_block_system_prompt` | pass |
| empty skill set XML | `generates_xml_for_an_empty_skill_set` | pass |
| XML escaping + `<location>` | `generates_xml_with_escaping_and_location` | pass |
| non-object state value is a TypeError | `a_non_object_state_value_is_a_type_error` | pass |
| `skills` tool without a ToolContext throws | — | skipped: Rust `ToolContext` always carries an `Agent`; the error is unreachable |
| URL skill sources (`https://`, fetch/timeout/HTTP errors) | — | skipped: no HTTP client in the crate (see deviations) |
| sandbox-based discovery (`agent.sandbox`, `listFiles`) | — | skipped: no sandbox abstraction in strands-rs; discovery uses `std::fs` |

### `skills/__tests__/skill.test.*` (frontmatter / validation)
| TS test | Rust test | Status |
| --- | --- | --- |
| must start with `---` | `frontmatter_requires_an_opening_delimiter` | pass |
| missing closing `---` | `frontmatter_requires_a_closing_delimiter` | pass |
| requires `name` / `description` | `requires_name_and_description` | pass |
| parses `allowed-tools` (string form), `license`, `compatibility`, `metadata`, body | `parses_optional_frontmatter_fields` | pass |
| parses `allowed-tools` as a list | `parses_allowed_tools_as_a_yaml_list` | pass |
| invalid name warns (non-strict) / throws (strict) | `invalid_names_warn_by_default_and_fail_in_strict_mode` | pass |
| consecutive hyphens / >64 chars | `strict_mode_rejects_consecutive_hyphens_and_long_names` | pass |
| name must match parent directory | `strict_mode_requires_the_name_to_match_the_directory` | pass |
| `fromFile` on a SKILL.md path / invalid path error | `from_file_accepts_a_skill_md_path_and_rejects_other_paths` | pass |
| `fromDirectory` missing dir error | `from_directory_rejects_a_missing_directory` | pass |
| YAML colon-quoting fallback (`fixYamlColons`) | — | skipped: the hand-rolled parser treats a scalar's `:` as literal, so the fallback has no analogue |

### `context-injector/__tests__/plugin.test.ts`
| TS test | Rust test | Status |
| --- | --- | --- |
| has the default / configured name | `has_a_default_name_that_can_be_overridden` | pass |
| registers an `InvokeModelStage` input middleware on `initAgent` | folded into every hook test (`run_hook` fires `BeforeModelCallEvent`) | adapted |
| folds `renderContent()` text into the latest user message | `folds_render_content_text_into_the_latest_user_message` | pass |
| skips on a non-user turn by default | `skips_on_a_non_user_turn_by_default`, `skips_a_tool_result_user_turn_by_default` | pass |
| `'everyTurn'` injects regardless of the latest role | `every_turn_injects_regardless_of_the_latest_role` | pass |
| exposes `appState` and the agent to `renderContent` | `exposes_app_state_and_the_agent_to_render_content` | pass |
| fails open when `renderContent` throws | `fails_open_when_render_content_errors` | pass |
| empty/undefined text is a no-op | `empty_or_absent_text_is_a_no_op` | pass |
| predicate trigger | `a_custom_trigger_predicate_decides` | pass |
| no user message → no-op | `no_user_message_means_no_injection` | pass |
| leading-newline text skips the separator | `text_starting_with_a_newline_is_appended_without_a_separator` | pass |
| reaches the model when an injector is active | `injected_text_reaches_the_model_and_history_is_untouched` (`MockMessageModel::calls()`) | pass |
| `dynamicTrailingBlocks` handoff / absent without an injector | — | skipped: no per-call `dynamicTrailingBlocks` field on `BeforeModelCallEvent`; the same test asserts durable history is unchanged instead |

Counts: 45 Rust tests, 45 pass, 0 fail; 6 TS cases skipped (reasons above).

## Deviations from TS
- **No URL sources.** `SkillSource` is `Path | Instance`; `Skill::fromUrl` is not ported (the crate has no HTTP client and adding one is a dependency decision). Add a `SkillSource::Url` + `reqwest`-backed loader when the crate takes an HTTP dep.
- **No sandbox indirection.** TS defers fs discovery to `initAgent` so it reads `agent.sandbox`. Rust has no sandbox abstraction yet, so sources resolve eagerly in the constructor via `std::fs`, and the per-agent `WeakMap` collapses to one shared `Arc<RwLock<IndexMap>>`. Per-agent state (`activatedSkills`, `lastInjectedXml`) still lives in `agent.state()[state_key]`, so one plugin instance remains safe across agents.
- **Hand-rolled frontmatter parser** (`parse_yaml_mapping`): `key: value`, quoted scalars, `[a, b]` flow sequences, `- item` block sequences, one level of nested mapping. No YAML crate in the workspace; this covers every field the skill format uses. The `fixYamlColons` fallback is unnecessary as a result.
- **`ContextInjector` uses `BeforeModelCallEvent.messages`** instead of `InvokeModelStage.Input` middleware (per task), and so does not carry TS's `dynamicTrailingBlocks` accumulator — nothing in the Rust loop consumes it. Ordering across multiple injectors is hook-registration (= plugin-registration) order, matching TS.
- `renderContent` is `Fn(&InjectionContext) -> BoxFuture<Result<Option<String>>>`; `Err` and `Ok(None)` both fail open. `ContextInjector::inject(text)` is the fixed-string shortcut.
- `AgentSkills`/`ContextInjector` are `Clone` (cheap `Arc` handles) so one instance can be registered and still queried by the caller.

## Needed from other modules / stubs
- Nothing stubbed. One shared-file edit: `agent/mod.rs:267` `mod test_fixtures` → `pub(crate) mod test_fixtures` (additive, needed to build hook events in tests outside `agent/`).
