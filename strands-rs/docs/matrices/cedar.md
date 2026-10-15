# `interventions::vended::cedar` — `CedarAuthorization` (strands-rs)

Branch `agent-tasks/strands-rs-cedar`. Source: `strands-ts/src/vended-interventions/cedar/**`
(`cedar.ts`, `__tests__/cedar.test.node.ts`, 39 `it(...)`).

Files: `crates/strands-agents/src/interventions/vended/cedar.rs` (impl + 43 unit tests),
`crates/strands-agents/tests/cedar_loop.rs` (1 agent-loop test); edits to
`interventions/vended/mod.rs` (re-export) and `Cargo.toml` (`cedar` feature).

Gates: `cargo fmt --all`, `cargo clippy -p strands-agents --all-features --tests -- -D warnings`,
`cargo test -p strands-agents --all-features --lib cedar` → 43 passed,
`--test cedar_loop` → 1 passed, `cargo check -p strands-agents --no-default-features` clean.

## API matrix (TS → Rust)

| TS | Rust |
| --- | --- |
| `new CedarAuthorization({ policies })` | `CedarAuthorization::from_policies(impl Into<CedarSource>)` |
| `new CedarAuthorization(config)` | `CedarAuthorization::new(CedarAuthorizationConfig)` |
| `policies: string` (text or `.cedar` path) | `CedarAuthorizationConfig::new(impl Into<CedarSource>)`; `CedarSource::{Text,File}` |
| `entities: Entity[] \| string` | `.with_entities(impl Into<CedarSource>)` (Cedar entity JSON array, inline or file) |
| `schema: string` | `.with_schema(impl Into<CedarSource>)` |
| `tools: McpTool[]` | `.with_tools(impl IntoIterator<Item = ToolSpec>)` |
| `namespace: string` | `.with_namespace(impl Into<String>)` |
| `principal: { type, id }` | `.with_principal(EntityRef)` / `EntityRef::new(type, id)` |
| `principalResolver: (state) => Entity \| undefined` | `.with_principal_resolver(PrincipalResolver)` = `Arc<dyn Fn(&InvocationState) -> Result<Option<EntityRef>>>` |
| `contextEnricher: ({ toolName, input, invocationState }) => object` | `.with_context_enricher(ContextEnricher)` = `Arc<dyn Fn(&EnrichmentContext) -> Result<Map<String, JsonValue>>>` |
| `onError: 'throw' \| 'deny' \| 'proceed'` (base class) | `.with_on_error(OnError::{Propagate,Deny,Proceed})` |
| `handler.name` | `InterventionHandler::name` → `"cedar-authorization"` |
| `beforeToolCall(event)` | `InterventionHandler::before_tool_call(&BeforeToolCallEvent) -> Result<ToolCallAction>` |
| `reload()` | `CedarAuthorization::reload(&self) -> Result<()>` |
| `resetCallCounts(agent)` | `CedarAuthorization::reset_call_counts(&self, &Agent)` |
| state key `'cedar-authorization'` | `CALL_COUNTS_KEY` |

## Test matrix (TS `it(...)` → Rust test)

All Rust tests live in `cedar.rs` `mod tests` unless noted.

| TS test | Rust test | Status |
| --- | --- | --- |
| allows permitted tool calls | `allows_permitted_tool_calls` + `cedar_loop.rs::permitted_tool_runs_and_denied_tool_returns_an_error_result` | pass |
| denies tools not in any permit policy (default-deny) | `denies_tools_not_in_any_permit_policy` | pass |
| enforces role-based access (admin can delete, analyst cannot) | `enforces_role_based_access_admin_can_delete_analyst_cannot` | pass |
| enforces role-based access (analyst can search, viewer cannot) | `enforces_role_based_access_analyst_can_search_viewer_cannot` | pass |
| enforces rate limits via call_count in session context | `enforces_rate_limits_via_call_count_in_session_context` | pass |
| enforces environment restrictions via contextEnricher | `enforces_environment_restrictions_via_context_enricher` | pass |
| denies when principal is missing (fail-closed) | `denies_when_principal_is_missing` | pass |
| throws on malformed policy at construction time | `throws_on_malformed_policy_at_construction_time` | pass |
| supports static principal (no invocationState needed) | `supports_a_static_principal_without_invocation_state` | pass |
| throws when both principal and principalResolver are provided | `throws_when_both_principal_and_principal_resolver_are_provided` | pass |
| defaults to `User::"anonymous"` … | `defaults_to_an_anonymous_user_principal` | pass |
| uses unconstrained resource by default | `uses_an_unconstrained_resource_by_default` | pass |
| constrains on tool arguments via context.input | `constrains_on_tool_arguments_via_context_input` | pass (also asserts the permitted argument) |
| adds custom fields usable in policies | `an_enricher_adds_custom_fields_usable_in_policies` | pass |
| denies when enricher value does not match policy | `an_enricher_adds_custom_fields_usable_in_policies` (second half) | pass |
| throws by default when handler errors | `propagates_by_default_when_a_callback_fails` | pass |
| denies when onError is "deny" and handler throws | `denies_when_on_error_is_deny_and_a_callback_fails` | pass |
| proceeds when onError is "proceed" and handler throws | `proceeds_when_on_error_is_proceed_and_a_callback_fails` | pass |
| reads .cedar file from disk | `reads_a_cedar_policy_file_from_disk` | pass |
| reads .json entity file from disk | `reads_a_json_entity_file_from_disk` | pass |
| throws when .cedar file does not exist | `throws_when_cedar_file_does_not_exist` | pass |
| resetCallCounts clears counts and re-enables rate-limited tools | `reset_call_counts_re_enables_rate_limited_tools` | pass |
| reloads policies from file | `reloads_policies_from_file` | pass |
| throws on reload if policy file was deleted | `throws_on_reload_if_policy_file_was_deleted` | pass |
| validates policies on reload | `validates_policies_on_reload` | pass |
| passes validation when policies match schema | `passes_validation_when_policies_match_schema` | pass |
| throws when policy references unknown action | `throws_when_policy_references_unknown_action` | pass |
| throws when policy references unknown attribute | `throws_when_policy_references_unknown_attribute` | pass |
| accepts inline schema string | `accepts_inline_schema_string` | pass |
| skips schema validation when schema is not provided | `skips_schema_validation_when_schema_is_not_provided` | pass |
| auto-generates schema and validates policies when tools are provided | `auto_generates_schema_and_validates_policies_when_tools_are_provided` | pass |
| catches unknown action names via auto-generated schema | `catches_unknown_action_names_via_auto_generated_schema` | pass |
| allows policies referencing context.session (handler-injected, not in schema) | `allows_policies_referencing_context_session_not_in_the_generated_schema` | pass |
| allows tool calls when tools config is provided | `auto_generates_schema_and_validates_policies_when_tools_are_provided` (construction) + `works_with_a_namespace_and_tools` (runtime decision) | pass |
| validates nested tool input schemas without breaking | `validates_nested_tool_input_schemas_without_breaking` | pass |
| surfaces schema generator errors for naming collisions in nested inputs | `surfaces_schema_generator_errors_for_naming_collisions_in_nested_inputs` | pass |
| uses namespaced action and resource types when namespace is set | `uses_namespaced_action_and_resource_types` | pass |
| denies when principal is not in the role (namespaced) | `uses_namespaced_action_and_resource_types` (second half) | pass |
| works with namespace and tools (schema generation path) | `works_with_a_namespace_and_tools` | pass |

Rust-only tests (no TS counterpart): `passes_validation_when_the_schema_comes_from_a_file`,
`throws_when_schema_file_does_not_exist`,
`the_generated_schema_is_namespaced_only_when_a_namespace_is_configured`,
`a_tool_spec_without_an_input_schema_still_generates_an_action`,
`entities_must_carry_a_uid_with_a_type_and_an_id`, `entity_sources_accept_the_escaped_uid_form`,
`text_and_path_convert_to_the_matching_source`, `the_utc_hour_is_a_legal_cedar_long`.

TS drives every decision test through a whole `Agent.invoke`; Rust drives
`before_tool_call` with a `BeforeToolCallEvent` fixture (`fn event`/`fn gate` in `mod tests`) and
keeps one end-to-end loop test, `tests/cedar_loop.rs`, for the wiring.

## Divergences

- Engine is the first-party `cedar_policy` crate plus `cedar_policy_mcp_schema_generator` (the
  crate the TS/Python WASM bundles are built from), not `@cedar-policy/cedar-wasm`. Policies,
  entities and the schema are parsed once at construction / `reload()`, not per tool call.
  `cedar-policy` is pinned `=4.11.0` because the generator pins `cedar-policy-core =4.11.0`.
- `CedarSource::{Text,File}` replaces TS's single string whose `.cedar`/`.cedarschema` suffix
  decides whether it is text or a path.
- Tools are described by this crate's `ToolSpec`, not a separate MCP-shaped struct.
- TS's `onError` is base-class `InterventionHandler` behaviour; Rust handlers return `Result`, so
  `OnError` is a Cedar config option covering this handler's own callbacks. A failing principal
  resolver or context enricher restores the call count it consumed (TS leaks it).
- Entity type/id are parsed into Cedar's `EntityUid`, so Python's double-quote-injection guards are
  unnecessary.
- Everything is behind the `cedar` cargo feature (heavy optional deps).

## Open question

`strip_namespace` is string surgery on generator output, kept because it reproduces the TS
generated-schema shape; it would break if the generator's formatting changes. The alternative is to
validate un-namespaced policy sets against a namespaced schema with the namespace prepended.

## Live test

`crates/strands-agents/tests/integ_cedar.rs` (`#[ignore = "live Bedrock"]`,
`us.anthropic.claude-haiku-4-5-20251001-v1:0`, `AWS_REGION=us-west-2`): a permitted tool runs and a
Cedar-denied tool never runs inside a real tool-use loop. 2 passed.

## Review disposition (round 1)

| Finding | Disposition |
| --- | --- |
| 🟠 `OnError::Deny` must prefix `Handler threw: ` | fixed — `Deny(format!("Handler threw: {error}"))`; asserted in `denies_when_on_error_is_deny_and_a_callback_fails` |
| 🟠 float/`null` tool input is denied, undocumented + untested | fixed — module `//!` note + `denies_tool_input_cedar_cannot_represent` |
| 🟠 silent fail-open on `OnError::Proceed` | fixed — `tracing::warn!(handler = "cedar-authorization", %error, ..)` |
| 🟡 `increment → decide → decrement` not atomic | fixed — the `call_counts` guard is taken once in `authorize` and passed `&mut` into increment/decrement, so the decision happens inside the lock |
| 🟡 only `CedarAuthorization` re-exported from `vended` | fixed — `CedarAuthorizationConfig`, `CedarSource`, `EntityRef`, `OnError` re-exported too |
| 🟡 enricher-cannot-override guarantee untested | fixed — `an_enricher_cannot_overwrite_hour_utc_or_call_count` |
| 🟡 `strip_namespace` falls through on `None` | fixed — the `None` arm returns `Err(generation_failed(..))`; `stripping_a_namespace_from_an_unnamespaced_schema_is_an_error` |
