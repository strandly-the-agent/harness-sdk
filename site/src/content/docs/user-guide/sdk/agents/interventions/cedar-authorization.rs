//! Rust snippets for `cedar-authorization.mdx`. Enable the `cedar` feature.

use strands_agents::{tool, Result};

/// Search for information.
#[tool]
async fn search(query: String) -> String {
    format!("Results for: {query}")
}

/// Delete a record by ID.
#[tool]
async fn delete_record(record_id: String) -> String {
    format!("Deleted {record_id}")
}

/// Send an email.
#[tool]
async fn send_email(to: String, body: String) -> String {
    let _ = body;
    format!("Sent to {to}")
}

/// Deploy the service.
#[tool]
async fn deploy(version: String) -> String {
    format!("Deployed {version}")
}

// =====================
// Basic usage
// =====================

async fn basic_example() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:basic_example]
    use strands_agents::interventions::vended::CedarAuthorization;
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    let cedar = CedarAuthorization::from_policies(
        r#"permit(principal, action == Action::"search", resource);"#,
    )?;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .tool(SearchTool::new())
        .tool(DeleteRecordTool::new())
        .intervention(cedar)
        .build()?;

    agent
        .invoke("Search for quarterly reports then delete record 42")
        .await?;
    // A search call is permitted; delete_record is denied (no matching permit)
    // --8<-- [end:basic_example]
    Ok(())
}

// =====================
// Role-based access control
// =====================

async fn role_based_example() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:role_based]
    use std::sync::Arc;

    use serde_json::Map;
    use strands_agents::interventions::vended::cedar::{CedarAuthorizationConfig, EntityRef};
    use strands_agents::interventions::vended::CedarAuthorization;
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::{Agent, InvocationState, InvokeOptions};

    let cedar = CedarAuthorization::new(
        CedarAuthorizationConfig::new(
            r#"
              permit(principal, action, resource)
              when { context.session.role == "admin" };

              permit(principal, action == Action::"search", resource)
              when { context.session.role == "analyst" };
            "#,
        )
        .with_principal_resolver(Arc::new(|state: &InvocationState| {
            Ok(state
                .get::<String>("user_id")
                .map(|id| EntityRef::new("User", id.as_str())))
        }))
        .with_context_enricher(Arc::new(|ctx| {
            let role = ctx
                .invocation_state
                .get::<String>("role")
                .map_or_else(|| "none".to_owned(), |role| role.to_string());
            Ok(Map::from_iter([("role".to_owned(), role.into())]))
        })),
    )?;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .tool(SearchTool::new())
        .tool(DeleteRecordTool::new())
        .intervention(cedar)
        .build()?;

    // admin can use any tool
    let admin = InvocationState::new();
    admin.insert("user_id", "alice".to_owned());
    admin.insert("role", "admin".to_owned());
    agent
        .invoke_with(
            "Delete record 42",
            InvokeOptions::default().with_invocation_state(admin),
        )
        .await?;

    // analyst can only search
    let analyst = InvocationState::new();
    analyst.insert("user_id", "bob".to_owned());
    analyst.insert("role", "analyst".to_owned());
    agent
        .invoke_with(
            "Delete record 42",
            InvokeOptions::default().with_invocation_state(analyst),
        )
        .await?;
    // denied: no permit matches for delete_record with role "analyst"
    // --8<-- [end:role_based]
    Ok(())
}

// =====================
// Rate limiting
// =====================

async fn rate_limit_example() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:rate_limit]
    use strands_agents::interventions::vended::CedarAuthorization;
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    let cedar = CedarAuthorization::from_policies(
        r#"
          permit(principal, action == Action::"send_email", resource)
          when { context.session.call_count < 5 };

          permit(principal, action == Action::"search", resource);
        "#,
    )?;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .tool(SendEmailTool::new())
        .tool(SearchTool::new())
        .intervention(cedar)
        .build()?;

    // send_email is permitted for the first 4 calls, then denied on the 5th
    // search is unlimited
    // --8<-- [end:rate_limit]
    let _ = agent;
    Ok(())
}

// =====================
// Schema validation
// =====================

fn schema_validation_example() -> Result<()> {
    // --8<-- [start:schema_validation]
    use std::sync::Arc;

    use serde_json::Map;
    use strands_agents::interventions::vended::cedar::CedarAuthorizationConfig;
    use strands_agents::interventions::vended::CedarAuthorization;
    use strands_agents::tools::Tool;

    // Valid policies pass schema validation
    let cedar = CedarAuthorization::new(
        CedarAuthorizationConfig::new(
            r#"
              permit(principal, action == Action::"search", resource);
              permit(principal, action == Action::"delete_record", resource)
              when { context.session.role == "admin" };
            "#,
        )
        .with_tools([
            SearchTool::new().spec().clone(),
            DeleteRecordTool::new().spec().clone(),
        ])
        .with_context_enricher(Arc::new(|ctx| {
            let role = ctx
                .invocation_state
                .get::<String>("role")
                .map_or_else(|| "none".to_owned(), |role| role.to_string());
            Ok(Map::from_iter([("role".to_owned(), role.into())]))
        })),
    )?;

    // A typo in the action name fails at construction:
    // CedarAuthorization::new(
    //     CedarAuthorizationConfig::new(
    //         r#"permit(principal, action == Action::"deleet_record", resource);"#,
    //     )
    //     .with_tools([SearchTool::new().spec().clone()]),
    // )
    // → Err("Cedar policy validation failed: unrecognized action")
    // --8<-- [end:schema_validation]
    let _ = cedar;
    Ok(())
}

// =====================
// Namespaced policies
// =====================

async fn namespace_example() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:namespace]
    use std::sync::Arc;

    use serde_json::json;
    use strands_agents::interventions::vended::cedar::{CedarAuthorizationConfig, EntityRef};
    use strands_agents::interventions::vended::CedarAuthorization;
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::tools::Tool;
    use strands_agents::{Agent, InvocationState, InvokeOptions};

    let entities = json!([
        { "uid": { "type": "Agent::Resource", "id": "default" }, "attrs": {}, "parents": [] }
    ]);

    let cedar = CedarAuthorization::new(
        CedarAuthorizationConfig::new(
            r#"permit(principal, action == Agent::Action::"search", resource);"#,
        )
        .with_namespace("Agent")
        .with_tools([SearchTool::new().spec().clone()])
        .with_entities(entities.to_string())
        .with_principal_resolver(Arc::new(|state: &InvocationState| {
            Ok(state
                .get::<String>("user_id")
                .map(|id| EntityRef::new("Agent::User", id.as_str())))
        })),
    )?;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .tool(SearchTool::new())
        .intervention(cedar)
        .build()?;

    let state = InvocationState::new();
    state.insert("user_id", "alice".to_owned());
    agent
        .invoke_with(
            "Search for reports",
            InvokeOptions::default().with_invocation_state(state),
        )
        .await?;
    // --8<-- [end:namespace]
    Ok(())
}

// =====================
// Environment gating
// =====================

async fn env_gating_example() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:env_gating]
    use std::sync::Arc;

    use serde_json::Map;
    use strands_agents::interventions::vended::cedar::CedarAuthorizationConfig;
    use strands_agents::interventions::vended::CedarAuthorization;
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::{Agent, InvocationState, InvokeOptions};

    let cedar = CedarAuthorization::new(
        CedarAuthorizationConfig::new(
            r#"
              permit(principal, action == Action::"deploy", resource)
              when { context.session.environment in ["staging", "development"] };
            "#,
        )
        .with_context_enricher(Arc::new(|ctx| {
            let environment = ctx
                .invocation_state
                .get::<String>("environment")
                .map_or_else(|| "unknown".to_owned(), |env| env.to_string());
            Ok(Map::from_iter([(
                "environment".to_owned(),
                environment.into(),
            )]))
        })),
    )?;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .tool(DeployTool::new())
        .intervention(cedar)
        .build()?;

    // works in staging
    let staging = InvocationState::new();
    staging.insert("environment", "staging".to_owned());
    agent
        .invoke_with(
            "Deploy the service",
            InvokeOptions::default().with_invocation_state(staging),
        )
        .await?;

    // denied when environment is absent: the enricher falls back to
    // "unknown", which is outside the permitted set
    agent.invoke("Deploy the service").await?;
    // --8<-- [end:env_gating]
    Ok(())
}

// =====================
// File-based policies
// =====================

fn file_policies_example() -> Result<()> {
    // --8<-- [start:file_policies]
    use std::path::Path;

    use strands_agents::interventions::vended::cedar::CedarAuthorizationConfig;
    use strands_agents::interventions::vended::CedarAuthorization;

    let cedar = CedarAuthorization::new(
        CedarAuthorizationConfig::new(Path::new("./policies/agent.cedar"))
            .with_entities(Path::new("./policies/entities.json")),
    )?;
    // --8<-- [end:file_policies]
    let _ = cedar;
    Ok(())
}

// =====================
// Hot reload
// =====================

fn hot_reload_example() -> Result<()> {
    // --8<-- [start:hot_reload]
    use std::path::Path;

    use strands_agents::interventions::vended::CedarAuthorization;

    let cedar = CedarAuthorization::from_policies(Path::new("./policies/agent.cedar"))?;

    // After editing agent.cedar on disk:
    cedar.reload()?;
    // Validates new policies before applying. Returns Err if invalid, and the
    // previous policy set stays active.
    // --8<-- [end:hot_reload]
    Ok(())
}
