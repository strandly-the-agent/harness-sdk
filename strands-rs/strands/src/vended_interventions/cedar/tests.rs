//! Ported from strands-ts/src/vended-interventions/cedar/__tests__/cedar.test.node.ts.
//!
//! Most cases drive the handler's `before_tool_call` directly; the first two and the `onError`
//! cases run a whole agent as TS does. The fixtures are copies of the TS ones.

use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;

use super::*;
use crate::agent::Agent;
use crate::test_support::{mock_tool, text_result};
use crate::testing::MockMessageModel;
use crate::types::agent::{AgentResult, InvokeOptions, LocalAgent};
use crate::types::messages::{ContentBlock, StopReason, TextBlock, ToolUseBlock};
use crate::types::tools::ToolUse;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/cedar");

#[derive(Clone)]
struct UserId(&'static str);

#[derive(Clone)]
struct Environment(&'static str);

fn fixture(name: &str) -> String {
    format!("{FIXTURES}/{name}")
}

fn entities() -> CedarEntities {
    CedarEntities::Json(vec![
        json!({ "uid": { "type": "Resource", "id": "agent" }, "attrs": {}, "parents": [] }),
        json!({ "uid": { "type": "User", "id": "alice" }, "attrs": { "role": "admin" }, "parents": [] }),
        json!({ "uid": { "type": "User", "id": "bob" }, "attrs": { "role": "analyst" }, "parents": [] }),
        json!({ "uid": { "type": "User", "id": "eve" }, "attrs": { "role": "viewer" }, "parents": [] }),
    ])
}

fn user(id: &str) -> TypeAndId {
    TypeAndId::new("User", id)
}

fn state_user(state: &InvocationState) -> Result<Option<TypeAndId>> {
    Ok(state.get::<UserId>().map(|UserId(id)| user(id)))
}

fn alice(_state: &InvocationState) -> Result<Option<TypeAndId>> {
    Ok(Some(user("alice")))
}

fn state_of(user_id: &'static str) -> InvocationState {
    let state = InvocationState::new();
    state.insert(UserId(user_id));
    state
}

fn local_agent() -> LocalAgent {
    LocalAgent::new(Arc::new(MockMessageModel::new()))
}

/// The handler's verdict on one call: `None` proceeds, `Some(reason)` denies.
async fn decide(
    cedar: &CedarAuthorization,
    agent: &LocalAgent,
    tool: &str,
    input: JsonValue,
    state: InvocationState,
) -> Option<String> {
    let mut event = BeforeToolCallEvent::new(
        agent.tool_agent(),
        ToolUse::new(tool, "tool-1", input),
        None,
        state,
    );
    match cedar.before_tool_call(&mut event).await.unwrap() {
        BeforeToolCallAction::Proceed(_) => None,
        BeforeToolCallAction::Deny(deny) => Some(deny.reason),
        other => panic!("unexpected action {other:?}"),
    }
}

/// Runs an agent whose model calls `tool` once; returns the outcome and how often the tool ran.
async fn run_agent(
    cedar: CedarAuthorization,
    tool: &str,
    input: JsonValue,
    state: InvocationState,
) -> (Result<AgentResult>, usize) {
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![ContentBlock::ToolUse(ToolUseBlock::new(
        tool, "tool-1", input,
    ))]);
    model.add_turn(vec![ContentBlock::Text(TextBlock::new("Done"))]);
    let executed = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&executed);
    let tool = mock_tool(tool, move |context| {
        count.fetch_add(1, Ordering::SeqCst);
        text_result(context.tool_use().tool_use_id(), "results")
    });
    let mut agent = Agent::builder()
        .model(model)
        .tools([tool])
        .intervention(cedar)
        .printer(false)
        .build()
        .unwrap();
    let options = InvokeOptions {
        invocation_state: state,
        ..InvokeOptions::default()
    };
    let result = agent.invoke_with("Go", options).await;
    (result, executed.load(Ordering::SeqCst))
}

fn cedar(config: CedarAuthorizationConfig) -> CedarAuthorization {
    CedarAuthorization::new(config).unwrap()
}

fn error_of(config: CedarAuthorizationConfig) -> String {
    CedarAuthorization::new(config).unwrap_err().to_string()
}

fn fixture_config(policies: &str) -> CedarAuthorizationConfig {
    CedarAuthorizationConfig {
        policies: fixture(policies),
        entities: Some(entities()),
        principal_resolver: Some(Arc::new(state_user)),
        ..CedarAuthorizationConfig::default()
    }
}

fn inline(policies: &str) -> CedarAuthorizationConfig {
    CedarAuthorizationConfig {
        policies: policies.to_owned(),
        ..CedarAuthorizationConfig::default()
    }
}

mod real_cedar_evaluation {
    use super::*;

    // No TS case: the tool results the model receives for a permitted and a forbidden call equal
    // the ones of the TS run in the conformance fixture `model_requests/cedar_run.json`.
    #[tokio::test]
    async fn the_model_receives_the_ts_results_for_a_permitted_and_a_forbidden_call() {
        let model = Arc::new(MockMessageModel::new());
        model.add_turn(vec![
            ToolUseBlock::new("search", "tooluse_cedar_1", json!({ "query": "cats" })).into(),
            ToolUseBlock::new("delete", "tooluse_cedar_2", json!({ "id": "1" })).into(),
        ]);
        model.add_turn(vec![ContentBlock::Text(TextBlock::new("Done"))]);
        let tool = |name: &'static str| {
            mock_tool(name, move |context| {
                text_result(context.tool_use().tool_use_id(), &format!("{name} ran"))
            })
        };
        let policies = concat!(
            "permit(principal, action == Action::\"search\", resource);\n",
            "permit(principal, action == Action::\"delete\", resource);\n",
            "forbid(principal, action == Action::\"delete\", resource) when { context.input.id == \"1\" };",
        );
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .tools([tool("search"), tool("delete")])
            .intervention(cedar(inline(policies)))
            .printer(false)
            .build()
            .unwrap();

        agent.invoke("Search, then delete").await.unwrap();

        let fixture: JsonValue = serde_json::from_str(include_str!(
            "../../../../conformance/fixtures/model_requests/cedar_run.json"
        ))
        .unwrap();
        assert_eq!(
            serde_json::to_value(model.requests()[1].messages[2].content()).unwrap(),
            fixture[1]["messages"][2]["content"]
        );
    }

    #[tokio::test]
    async fn allows_permitted_tool_calls() {
        let (result, executed) = run_agent(
            cedar(fixture_config("test.cedar")),
            "search",
            json!({ "query": "test" }),
            state_of("alice"),
        )
        .await;
        assert_eq!(result.unwrap().stop_reason(), StopReason::EndTurn);
        assert_eq!(executed, 1);
    }

    #[tokio::test]
    async fn denies_tools_not_in_any_permit_policy_default_deny() {
        let (result, executed) = run_agent(
            cedar(fixture_config("test.cedar")),
            "delete",
            json!({ "id": "1" }),
            state_of("alice"),
        )
        .await;
        assert_eq!(result.unwrap().stop_reason(), StopReason::EndTurn);
        assert_eq!(executed, 0);
        let handler = cedar(fixture_config("test.cedar"));
        let denied = decide(
            &handler,
            &local_agent(),
            "delete",
            json!({ "id": "1" }),
            state_of("alice"),
        )
        .await;
        assert_eq!(denied.as_deref(), Some("Access denied by Cedar policy"));
    }

    #[tokio::test]
    async fn enforces_role_based_access_admin_can_delete_analyst_cannot() {
        let handler = cedar(fixture_config("role-based.cedar"));
        let agent = local_agent();
        let input = json!({ "id": "1" });
        assert_eq!(
            decide(&handler, &agent, "delete", input.clone(), state_of("alice")).await,
            None
        );
        assert_eq!(
            decide(&handler, &agent, "delete", input, state_of("bob"))
                .await
                .as_deref(),
            Some("Access denied by Cedar policy")
        );
    }

    #[tokio::test]
    async fn enforces_role_based_access_analyst_can_search_viewer_cannot() {
        let handler = cedar(fixture_config("role-based.cedar"));
        let agent = local_agent();
        let input = json!({ "query": "x" });
        assert_eq!(
            decide(&handler, &agent, "search", input.clone(), state_of("bob")).await,
            None
        );
        assert!(
            decide(&handler, &agent, "search", input, state_of("eve"))
                .await
                .is_some()
        );
    }

    #[tokio::test]
    async fn enforces_rate_limits_via_call_count_in_session_context() {
        let handler = cedar(CedarAuthorizationConfig {
            principal_resolver: Some(Arc::new(alice)),
            ..fixture_config("rate-limited.cedar")
        });
        let agent = local_agent();
        let mut allowed = 0;
        for _ in 0..4 {
            let denied = decide(&handler, &agent, "send_email", json!({}), InvocationState::new()).await;
            allowed += usize::from(denied.is_none());
        }
        // The policy allows call_count < 3: calls 1 and 2 pass, later ones are denied.
        assert_eq!(allowed, 2);
        assert_eq!(
            agent.app_state().get_value(CedarAuthorization::STATE_KEY),
            Some(json!({ "send_email": 2 }))
        );
    }

    #[tokio::test]
    async fn enforces_environment_restrictions_via_context_enricher() {
        let handler = cedar(
            CedarAuthorizationConfig::builder()
                .policies(fixture("env-restricted.cedar"))
                .entities(entities())
                .principal_resolver(alice)
                .context_enricher(|input| {
                    let environment = input
                        .invocation_state
                        .get::<Environment>()
                        .map_or("unknown", |Environment(name)| name);
                    let mut session = JsonObject::new();
                    session.insert("environment".to_owned(), json!(environment));
                    Ok(session)
                })
                .build(),
        );
        let agent = local_agent();
        let state = |name| {
            let state = InvocationState::new();
            state.insert(Environment(name));
            state
        };
        let input = json!({ "query": "x" });
        assert_eq!(
            decide(&handler, &agent, "search", input.clone(), state("development")).await,
            None
        );
        assert!(
            decide(&handler, &agent, "search", input, state("production"))
                .await
                .is_some()
        );
    }

    #[tokio::test]
    async fn denies_when_principal_is_missing_fail_closed() {
        let handler = cedar(fixture_config("test.cedar"));
        let agent = local_agent();
        let denied = decide(
            &handler,
            &agent,
            "search",
            json!({ "query": "x" }),
            InvocationState::new(),
        )
        .await;
        assert_eq!(
            denied.as_deref(),
            Some("No principal identity found in invocation state")
        );
        assert_eq!(agent.app_state().get_value(CedarAuthorization::STATE_KEY), None);
    }

    #[test]
    fn throws_on_malformed_policy_at_construction_time() {
        let error = error_of(CedarAuthorizationConfig {
            entities: Some(entities()),
            principal_resolver: Some(Arc::new(alice)),
            ..inline("this is not valid cedar syntax at all!!!")
        });
        assert!(error.starts_with("Invalid Cedar policy: "), "{error}");
    }
}

mod principal_config {
    use super::*;

    const PERMIT_SEARCH: &str = r#"permit(principal, action == Action::"search", resource);"#;

    #[tokio::test]
    async fn supports_static_principal_no_invocation_state_needed() {
        let handler = cedar(CedarAuthorizationConfig {
            principal: Some(user("alice")),
            entities: Some(entities()),
            ..inline(r#"permit(principal == User::"alice", action, resource);"#)
        });
        let denied = decide(
            &handler,
            &local_agent(),
            "search",
            json!({}),
            InvocationState::new(),
        )
        .await;
        assert_eq!(denied, None);
    }

    #[test]
    fn throws_when_both_principal_and_principal_resolver_are_provided() {
        let error = error_of(CedarAuthorizationConfig {
            principal: Some(user("alice")),
            principal_resolver: Some(Arc::new(alice)),
            ..inline(PERMIT_SEARCH)
        });
        assert_eq!(
            error,
            "Provide either `principal` or `principalResolver`, not both"
        );
    }

    #[tokio::test]
    async fn defaults_to_user_anonymous_when_neither_principal_nor_principal_resolver_is_provided() {
        let handler = cedar(inline(
            r#"permit(principal == User::"anonymous", action, resource);"#,
        ));
        let denied = decide(
            &handler,
            &local_agent(),
            "search",
            json!({}),
            InvocationState::new(),
        )
        .await;
        assert_eq!(denied, None);
    }
}

mod resource_handling {
    use super::*;

    #[tokio::test]
    async fn uses_unconstrained_resource_by_default() {
        let handler = cedar(CedarAuthorizationConfig {
            principal_resolver: Some(Arc::new(alice)),
            ..inline(r#"permit(principal, action == Action::"search", resource == Resource::"agent");"#)
        });
        let denied = decide(
            &handler,
            &local_agent(),
            "search",
            json!({}),
            InvocationState::new(),
        )
        .await;
        assert_eq!(denied, None);
    }

    #[tokio::test]
    async fn constrains_on_tool_arguments_via_context_input() {
        let handler = cedar(CedarAuthorizationConfig {
            principal_resolver: Some(Arc::new(alice)),
            ..inline(
                r#"permit(principal, action == Action::"delete", resource) when { context.input.record_id == "42" };"#,
            )
        });
        let agent = local_agent();
        let call = |id: &str| json!({ "record_id": id });
        assert_eq!(
            decide(&handler, &agent, "delete", call("42"), InvocationState::new()).await,
            None
        );
        assert!(
            decide(&handler, &agent, "delete", call("7"), InvocationState::new())
                .await
                .is_some()
        );
    }
}

mod context_enricher {
    use super::*;

    fn department(name: &'static str) -> CedarAuthorization {
        cedar(
            CedarAuthorizationConfig::builder()
                .policies(r#"permit(principal, action, resource) when { context.session.department == "engineering" };"#)
                .principal_resolver(alice)
                .context_enricher(move |_input| {
                    let mut session = JsonObject::new();
                    session.insert("department".to_owned(), json!(name));
                    Ok(session)
                })
                .build(),
        )
    }

    #[tokio::test]
    async fn adds_custom_fields_usable_in_policies() {
        let handler = department("engineering");
        assert_eq!(
            decide(
                &handler,
                &local_agent(),
                "search",
                json!({}),
                InvocationState::new()
            )
            .await,
            None
        );
    }

    #[tokio::test]
    async fn denies_when_enricher_value_does_not_match_policy() {
        let handler = department("marketing");
        assert!(
            decide(
                &handler,
                &local_agent(),
                "search",
                json!({}),
                InvocationState::new()
            )
            .await
            .is_some()
        );
    }
}

mod on_error_behavior {
    use super::*;

    fn crashing(on_error: Option<OnError>) -> CedarAuthorization {
        cedar(CedarAuthorizationConfig {
            principal_resolver: Some(Arc::new(|_state| Err(Error::other("resolver crash")))),
            on_error,
            ..inline("permit(principal, action, resource);")
        })
    }

    #[tokio::test]
    async fn throws_by_default_when_handler_errors() {
        for on_error in [None, Some(OnError::Throw)] {
            let (result, executed) =
                run_agent(crashing(on_error), "search", json!({}), InvocationState::new()).await;
            let error = result.unwrap_err().to_string();
            assert!(error.contains("resolver crash"), "{error}");
            assert_eq!(executed, 0);
        }
    }

    #[tokio::test]
    async fn denies_when_on_error_is_deny_and_handler_throws() {
        let (result, executed) = run_agent(
            crashing(Some(OnError::Deny)),
            "search",
            json!({}),
            InvocationState::new(),
        )
        .await;
        assert_eq!(result.unwrap().stop_reason(), StopReason::EndTurn);
        assert_eq!(executed, 0);
    }

    #[tokio::test]
    async fn proceeds_when_on_error_is_proceed_and_handler_throws() {
        let (result, executed) = run_agent(
            crashing(Some(OnError::Proceed)),
            "search",
            json!({}),
            InvocationState::new(),
        )
        .await;
        assert_eq!(result.unwrap().stop_reason(), StopReason::EndTurn);
        assert_eq!(executed, 1);
    }
}

mod file_based_config {
    use super::*;

    #[tokio::test]
    async fn reads_cedar_file_from_disk() {
        let handler = cedar(CedarAuthorizationConfig {
            principal_resolver: Some(Arc::new(alice)),
            ..inline(&fixture("permit-all.cedar"))
        });
        let denied = decide(
            &handler,
            &local_agent(),
            "anything",
            json!({}),
            InvocationState::new(),
        )
        .await;
        assert_eq!(denied, None);
    }

    #[tokio::test]
    async fn reads_json_entity_file_from_disk() {
        let handler = cedar(CedarAuthorizationConfig {
            entities: Some(CedarEntities::File(fixture("entities.json").into())),
            principal_resolver: Some(Arc::new(alice)),
            ..inline(
                r#"permit(principal, action == Action::"search", resource) when { principal.role == "admin" };"#,
            )
        });
        let denied = decide(
            &handler,
            &local_agent(),
            "search",
            json!({}),
            InvocationState::new(),
        )
        .await;
        assert_eq!(denied, None);
    }

    #[test]
    fn throws_when_cedar_file_does_not_exist() {
        let error = error_of(inline("/nonexistent/path/policies.cedar"));
        assert_eq!(
            error,
            "Cedar policy file not found: /nonexistent/path/policies.cedar"
        );
    }
}

mod session_management {
    use super::*;

    #[tokio::test]
    async fn reset_call_counts_clears_counts_and_re_enables_rate_limited_tools() {
        let handler = cedar(CedarAuthorizationConfig {
            principal_resolver: Some(Arc::new(alice)),
            ..inline("permit(principal, action, resource) when { context.session.call_count < 2 };")
        });
        let agent = local_agent();
        let call = || decide(&handler, &agent, "send", json!({}), InvocationState::new());
        assert_eq!(call().await, None);
        assert!(call().await.is_some());
        handler.reset_call_counts(agent.app_state()).unwrap();
        assert_eq!(
            agent.app_state().get_value(CedarAuthorization::STATE_KEY),
            Some(json!({}))
        );
        assert_eq!(call().await, None);
    }

    // No TS case: TS doesn't test that a new handler picks the counts up from the app state.
    #[tokio::test]
    async fn restores_call_counts_from_app_state() {
        let config = || CedarAuthorizationConfig {
            principal_resolver: Some(Arc::new(alice)),
            ..inline("permit(principal, action, resource) when { context.session.call_count < 2 };")
        };
        let agent = local_agent();
        assert_eq!(
            decide(
                &cedar(config()),
                &agent,
                "send",
                json!({}),
                InvocationState::new()
            )
            .await,
            None
        );
        assert!(
            decide(
                &cedar(config()),
                &agent,
                "send",
                json!({}),
                InvocationState::new()
            )
            .await
            .is_some()
        );
    }
}

mod reload {
    use super::*;

    fn file_config(path: &std::path::Path) -> CedarAuthorizationConfig {
        CedarAuthorizationConfig {
            principal_resolver: Some(Arc::new(alice)),
            ..inline(path.to_str().unwrap())
        }
    }

    #[test]
    fn reloads_policies_from_file() {
        let handler = cedar(CedarAuthorizationConfig {
            principal_resolver: Some(Arc::new(alice)),
            ..inline(&fixture("test.cedar"))
        });
        handler.reload().unwrap();
    }

    #[test]
    fn throws_on_reload_if_policy_file_was_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reload.cedar");
        std::fs::write(&path, "permit(principal, action, resource);").unwrap();
        let handler = cedar(file_config(&path));
        std::fs::remove_file(&path).unwrap();
        let error = handler.reload().unwrap_err().to_string();
        assert_eq!(error, format!("Cedar policy file not found: {}", path.display()));
    }

    #[tokio::test]
    async fn validates_policies_on_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reload.cedar");
        std::fs::write(&path, "permit(principal, action, resource);").unwrap();
        let handler = cedar(file_config(&path));
        std::fs::write(&path, "this is broken!!!").unwrap();
        let error = handler.reload().unwrap_err().to_string();
        assert!(error.starts_with("Invalid Cedar policy: "), "{error}");
        // The handler keeps the policies it had.
        assert_eq!(
            decide(
                &handler,
                &local_agent(),
                "search",
                json!({}),
                InvocationState::new()
            )
            .await,
            None
        );
    }
}

mod schema_validation {
    use super::*;

    fn with_schema(policies: &str, schema: String) -> CedarAuthorizationConfig {
        CedarAuthorizationConfig {
            schema: Some(schema),
            principal_resolver: Some(Arc::new(alice)),
            ..inline(policies)
        }
    }

    #[test]
    fn passes_validation_when_policies_match_schema() {
        cedar(with_schema(
            r#"permit(principal is User, action == Action::"search", resource is Resource);"#,
            fixture("test.cedarschema"),
        ));
    }

    #[test]
    fn throws_when_policy_references_unknown_action() {
        let error = error_of(with_schema(
            r#"permit(principal, action == Action::"nonexistent_tool", resource);"#,
            fixture("test.cedarschema"),
        ));
        assert!(
            error.starts_with("Cedar policy validation failed: policy0: "),
            "{error}"
        );
    }

    #[test]
    fn throws_when_policy_references_unknown_attribute() {
        let error = error_of(with_schema(
            r#"permit(principal, action == Action::"search", resource) when { principal.nonexistent == "x" };"#,
            fixture("test.cedarschema"),
        ));
        assert!(
            error.starts_with("Cedar policy validation failed: policy0: "),
            "{error}"
        );
    }

    #[test]
    fn accepts_inline_schema_string() {
        let schema = "
  entity User = { role: String };
  entity Resource;
  action \"search\" appliesTo { principal: [User], resource: [Resource] };
";
        cedar(with_schema(
            r#"permit(principal is User, action == Action::"search", resource is Resource);"#,
            schema.to_owned(),
        ));
    }

    #[test]
    fn skips_schema_validation_when_schema_is_not_provided() {
        cedar(inline(
            r#"permit(principal, action == Action::"anything", resource);"#,
        ));
    }

    // No TS case: TS tests the missing policy file only; the schema file has its own text.
    #[test]
    fn throws_when_cedarschema_file_does_not_exist() {
        let error = error_of(with_schema(
            "permit(principal, action, resource);",
            "/nonexistent/schema.cedarschema".to_owned(),
        ));
        assert_eq!(
            error,
            "Cedar schema file not found: /nonexistent/schema.cedarschema"
        );
    }
}

mod tools_config_schema_generator_integration {
    use super::*;

    fn tool(name: &str, input_schema: JsonValue) -> ToolDefinition {
        ToolDefinition {
            name: name.to_owned(),
            input_schema: Some(input_schema),
            description: None,
        }
    }

    fn tools() -> Vec<ToolDefinition> {
        vec![
            tool(
                "search",
                json!({ "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] }),
            ),
            tool(
                "delete",
                json!({ "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] }),
            ),
        ]
    }

    fn with_tools(policies: &str, tools: Vec<ToolDefinition>) -> CedarAuthorizationConfig {
        CedarAuthorizationConfig {
            tools: Some(tools),
            ..inline(policies)
        }
    }

    #[test]
    fn auto_generates_schema_and_validates_policies_when_tools_are_provided() {
        let handler = cedar(with_tools(
            r#"permit(principal, action == Action::"search", resource);"#,
            tools(),
        ));
        assert_eq!(handler.name(), "cedar-authorization");
    }

    #[test]
    fn catches_unknown_action_names_via_auto_generated_schema() {
        let error = error_of(with_tools(
            r#"permit(principal, action == Action::"nonexistent", resource);"#,
            tools(),
        ));
        assert!(
            error.starts_with("Cedar policy validation failed: policy0: "),
            "{error}"
        );
    }

    #[test]
    fn allows_policies_referencing_context_session_handler_injected_not_in_schema() {
        let handler = cedar(with_tools(
            r#"permit(principal, action == Action::"search", resource) when { context.session.role == "admin" };"#,
            tools(),
        ));
        assert_eq!(handler.name(), "cedar-authorization");
    }

    #[tokio::test]
    async fn allows_tool_calls_when_tools_config_is_provided() {
        let handler = cedar(CedarAuthorizationConfig {
            principal: Some(user("alice")),
            ..with_tools(
                r#"permit(principal, action == Action::"search", resource);"#,
                tools(),
            )
        });
        let (result, executed) =
            run_agent(handler, "search", json!({ "query": "x" }), InvocationState::new()).await;
        result.unwrap();
        assert_eq!(executed, 1);
    }

    #[tokio::test]
    async fn validates_nested_tool_input_schemas_without_breaking() {
        let mut nested = tools();
        nested.push(tool(
            "create_user",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "address": {
                        "type": "object",
                        "properties": { "street": { "type": "string" }, "city": { "type": "string" } },
                    },
                },
            }),
        ));
        let handler = cedar(CedarAuthorizationConfig {
            principal: Some(user("alice")),
            ..with_tools(
                r#"permit(principal, action == Action::"create_user", resource);"#,
                nested,
            )
        });
        let input = json!({ "name": "Bob", "address": { "street": "123", "city": "NYC" } });
        let (result, executed) = run_agent(handler, "create_user", input, InvocationState::new()).await;
        result.unwrap();
        assert_eq!(executed, 1);
    }

    #[test]
    fn surfaces_schema_generator_errors_for_naming_collisions_in_nested_inputs() {
        let colliding = vec![tool(
            "create_user",
            json!({
                "type": "object",
                "properties": {
                    "address": {
                        "type": "object",
                        "properties": {
                            "street": { "type": "string" },
                            "geo": {
                                "type": "object",
                                "properties": { "lat": { "type": "number" }, "lng": { "type": "number" } },
                            },
                        },
                    },
                    "address_geo": {
                        "type": "object",
                        "properties": { "lat": { "type": "string" }, "lng": { "type": "string" } },
                    },
                },
            }),
        )];
        let error = error_of(with_tools("permit(principal, action, resource);", colliding));
        assert!(error.starts_with("Schema generation failed: "), "{error}");
    }
}

mod namespace_option {
    use super::*;

    fn namespaced() -> CedarAuthorization {
        cedar(CedarAuthorizationConfig {
            namespace: Some("Agent".to_owned()),
            entities: Some(CedarEntities::Json(vec![
                json!({ "uid": { "type": "Agent::Role", "id": "admin" }, "attrs": {}, "parents": [] }),
                json!({
                    "uid": { "type": "Agent::User", "id": "alice" },
                    "attrs": {},
                    "parents": [{ "type": "Agent::Role", "id": "admin" }],
                }),
                json!({ "uid": { "type": "Agent::User", "id": "bob" }, "attrs": {}, "parents": [] }),
                json!({ "uid": { "type": "Agent::Resource", "id": "default" }, "attrs": {}, "parents": [] }),
            ])),
            principal_resolver: Some(Arc::new(|state| {
                Ok(state
                    .get::<UserId>()
                    .map(|UserId(id)| TypeAndId::new("Agent::User", id)))
            })),
            ..inline(
                r#"permit(principal in Agent::Role::"admin", action == Agent::Action::"search", resource);"#,
            )
        })
    }

    #[tokio::test]
    async fn uses_namespaced_action_and_resource_types_when_namespace_is_set() {
        let denied = decide(
            &namespaced(),
            &local_agent(),
            "search",
            json!({}),
            state_of("alice"),
        )
        .await;
        assert_eq!(denied, None);
    }

    #[tokio::test]
    async fn denies_when_principal_is_not_in_the_role_namespaced() {
        let denied = decide(
            &namespaced(),
            &local_agent(),
            "search",
            json!({}),
            state_of("bob"),
        )
        .await;
        assert_eq!(denied.as_deref(), Some("Access denied by Cedar policy"));
    }

    #[tokio::test]
    async fn works_with_namespace_and_tools_schema_generation_path() {
        let handler = cedar(CedarAuthorizationConfig {
            namespace: Some("MyApp".to_owned()),
            tools: Some(vec![ToolDefinition {
                name: "search".to_owned(),
                input_schema: Some(json!({
                    "type": "object",
                    "properties": { "query": { "type": "string" } },
                    "required": ["query"],
                })),
                description: None,
            }]),
            entities: Some(CedarEntities::Json(vec![
                json!({ "uid": { "type": "MyApp::Resource", "id": "default" }, "attrs": {}, "parents": [] }),
            ])),
            ..inline(r#"permit(principal, action == MyApp::Action::"search", resource);"#)
        });
        let denied = decide(
            &handler,
            &local_agent(),
            "search",
            json!({ "query": "x" }),
            InvocationState::new(),
        )
        .await;
        assert_eq!(denied, None);
    }
}

/// Rust-only cases for what TS leaves to the WASM bindings.
mod rust_only {
    use super::*;

    // No TS case: TS has no test for its `Invalid entity` check.
    #[test]
    fn entities_must_carry_a_uid_with_a_type_and_an_id() {
        for entity in [json!({ "attrs": {} }), json!({ "uid": { "type": "User" } })] {
            let error = error_of(CedarAuthorizationConfig {
                entities: Some(CedarEntities::Json(vec![entity])),
                ..inline("permit(principal, action, resource);")
            });
            assert_eq!(
                error,
                "Invalid entity: each entity must have a uid with type and id"
            );
        }
    }

    // No TS case: TS has no test for an evaluation failure.
    #[tokio::test]
    async fn a_tool_input_cedar_cannot_represent_fails_evaluation_and_restores_the_count() {
        let handler = cedar(inline("permit(principal, action, resource);"));
        let agent = local_agent();
        let denied = decide(
            &handler,
            &agent,
            "search",
            json!({ "ratio": 0.5 }),
            InvocationState::new(),
        )
        .await
        .unwrap();
        assert_eq!(
            denied,
            "Cedar evaluation failed: data did not match any variant of untagged enum RawCedarValueJson"
        );
        assert_eq!(
            agent.app_state().get_value(CedarAuthorization::STATE_KEY),
            Some(json!({ "search": 0 }))
        );
    }

    /// The tool input as a model streams it.
    fn streamed(text: &str) -> JsonValue {
        serde_json::from_str(text).unwrap()
    }

    // No TS case: `JSON.parse` already holds `1.0` and `1e0` as the integer 1 in TS.
    #[tokio::test]
    async fn a_whole_number_written_as_a_float_reaches_cedar_as_an_integer() {
        let handler = cedar(inline(
            "permit(principal, action, resource) when { context.input.n == 1 };",
        ));
        let agent = local_agent();
        for input in [r#"{"n":1.0}"#, r#"{"n":1e0}"#, r#"{"n":1}"#] {
            let denied = decide(
                &handler,
                &agent,
                "search",
                streamed(input),
                InvocationState::new(),
            )
            .await;
            assert_eq!(denied, None, "{input}");
        }
    }

    // No TS case: as above, for entity attributes and the enricher's values.
    #[tokio::test]
    async fn whole_number_floats_in_entities_and_session_fields_are_integers() {
        let handler = cedar(CedarAuthorizationConfig {
            entities: Some(CedarEntities::Json(vec![streamed(
                r#"{"uid":{"type":"User","id":"anonymous"},"attrs":{"level":1.0},"parents":[]}"#,
            )])),
            context_enricher: Some(Arc::new(|_input| {
                let mut session = JsonObject::new();
                session.insert("budget".to_owned(), json!(2.0));
                Ok(session)
            })),
            ..inline(
                "permit(principal, action, resource) when { principal.level == 1 && context.session.budget == 2 };",
            )
        });
        let denied = decide(
            &handler,
            &local_agent(),
            "search",
            json!({}),
            InvocationState::new(),
        )
        .await;
        assert_eq!(denied, None);
    }

    // No TS case: TS doesn't test the engine's error texts; these are the ones the WASM build gives.
    #[test]
    fn construction_errors_carry_the_engine_text() {
        assert_eq!(
            error_of(inline("permit(principal, action, resource")),
            "Invalid Cedar policy: failed to parse policies from string: unexpected end of input"
        );
        assert_eq!(
            error_of(inline(
                "permit(principal, action, resource) when { 1 + };\nforbid(principal, action resource);"
            )),
            "Invalid Cedar policy: failed to parse policies from string: unexpected token `}`"
        );
        assert_eq!(
            error_of(CedarAuthorizationConfig {
                schema: Some("entity User".to_owned()),
                ..inline("permit(principal, action, resource);")
            }),
            "Cedar policy validation failed: failed to parse schema from string: error parsing schema: unexpected end of input"
        );
        assert_eq!(
            error_of(CedarAuthorizationConfig {
                schema: Some("entity User; entity Resource; action \"search\" appliesTo { principal: [User], resource: [Resource] };".to_owned()),
                ..inline("permit(principal, action == Action::\"nope\", resource);")
            }),
            "Cedar policy validation failed: policy0: for policy `policy0`, unrecognized action `Action::\"nope\"`"
        );
    }

    // No TS case: as above, for evaluation failures and denials.
    #[tokio::test]
    async fn evaluation_failures_carry_the_engine_text() {
        let permit = "permit(principal, action, resource);";
        let agent = local_agent();
        let denied = |config: CedarAuthorizationConfig, input: JsonValue| {
            let agent = &agent;
            async move {
                decide(&cedar(config), agent, "search", input, InvocationState::new())
                    .await
                    .unwrap()
            }
        };
        assert_eq!(
            denied(
                CedarAuthorizationConfig {
                    principal: Some(TypeAndId::new("not a type", "a")),
                    ..inline(permit)
                },
                json!({})
            )
            .await,
            "Cedar evaluation failed: failed to parse principal: failed to parse escape `__entity`: {\n  \"type\": \"not a type\",\n  \"id\": \"a\"\n}, errors: unexpected token `a`"
        );
        assert_eq!(
            denied(
                CedarAuthorizationConfig {
                    entities: Some(CedarEntities::Json(vec![json!({
                        "uid": { "type": "User", "id": "anonymous" },
                        "attrs": { "level": 1.5 },
                        "parents": [],
                    })])),
                    ..inline(permit)
                },
                json!({})
            )
            .await,
            "Cedar evaluation failed: error during entity deserialization: data did not match any variant of untagged enum RawCedarValueJson"
        );
        assert_eq!(
            denied(inline(permit), json!({ "q": null })).await,
            "Cedar evaluation failed: while parsing context, found a `null`; JSON `null`s are not allowed in Cedar"
        );
        assert_eq!(
            denied(inline("forbid(principal, action, resource);"), json!({})).await,
            "Access denied by Cedar policy: policy0"
        );
        assert_eq!(
            denied(
                inline("permit(principal, action, resource) when { context.input.missing == 1 };"),
                json!({})
            )
            .await,
            "Access denied by Cedar policy: record does not have the attribute `missing`"
        );
    }

    // No TS case: TS reports what the WASM generator returns; these are its texts.
    #[test]
    fn schema_generation_failures_name_the_step() {
        let object = json!({ "type": "object", "properties": {} });
        let tools = |tool: ToolDefinition| CedarAuthorizationConfig {
            tools: Some(vec![tool]),
            ..inline("permit(principal, action, resource);")
        };
        let mut no_schema = ToolDefinition::new("search", JsonValue::Null);
        no_schema.input_schema = None;
        assert_eq!(
            error_of(tools(no_schema)),
            "Schema generation failed: Invalid tool descriptions: failed to parse JSON"
        );
        assert_eq!(
            error_of(CedarAuthorizationConfig {
                namespace: Some("not valid".to_owned()),
                ..tools(ToolDefinition::new("search", object.clone()))
            }),
            "Schema generation failed: Schema error: failed to parse schema stub"
        );
        assert_eq!(
            error_of(tools(ToolDefinition::new(
                "web-fetch",
                json!({ "type": "object", "properties": { "url": { "type": "string" } } })
            ))),
            "Schema generation failed: Error adding tools: unexpected token `-`"
        );
        let described = ToolDefinition::new("search", object).with_description("Searches");
        assert_eq!(
            serde_json::to_value(&described).unwrap(),
            json!({ "name": "search", "inputSchema": { "type": "object", "properties": {} }, "description": "Searches" })
        );
        CedarAuthorization::new(tools(described)).unwrap();
    }

    // No TS case: the Rust constructor that lets a downstream test call its enricher.
    #[test]
    fn a_context_enricher_can_be_called_with_a_built_input() {
        let enricher: ContextEnricher = Arc::new(|input| {
            let mut session = JsonObject::new();
            session.insert("tool".to_owned(), json!(input.tool_name));
            session.insert(
                "env".to_owned(),
                json!(input.invocation_state.get::<Environment>().map(|env| env.0)),
            );
            session.insert("q".to_owned(), input.tool_input["q"].clone());
            Ok(session)
        });
        let state = InvocationState::new();
        state.insert(Environment("prod"));
        let input = json!({ "q": "cats" });
        let session = enricher(&ContextEnricherInput::new("search", &input, &state)).unwrap();
        assert_eq!(
            JsonValue::Object(session),
            json!({ "tool": "search", "env": "prod", "q": "cats" })
        );
    }

    // No TS case: TS reads `Date.getUTCHours()`.
    #[test]
    fn the_utc_hour_is_the_hour_of_the_day() {
        let at = |seconds| UNIX_EPOCH + std::time::Duration::from_secs(seconds);
        assert_eq!(utc_hour(at(0)), 0);
        assert_eq!(utc_hour(at(23 * 3600 + 3599)), 23);
        assert_eq!(utc_hour(at(86_400 + 5 * 3600)), 5);
    }
}
