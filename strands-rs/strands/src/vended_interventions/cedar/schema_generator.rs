//! Generates a Cedar schema from tool definitions (TS
//! `vended-interventions/cedar/schema-generator.ts`).

use cedar_policy_mcp_schema_generator::{SchemaGenerator, SchemaGeneratorConfig};
use mcp_tools_sdk::description::ServerDescription;

use super::ToolDefinition;
use crate::error::{Error, Result};

fn failed(error: impl std::fmt::Display) -> Error {
    Error::other(format!("Schema generation failed: {error}"))
}

/// The schema for `tools`: one action per tool, with its input as the context. Without a
/// `namespace` the generated `namespace Agent { .. }` wrapper is removed, so un-namespaced
/// policies (`Action::"search"`) validate against it.
pub(super) fn generate_schema(tools: &[ToolDefinition], namespace: Option<&str>) -> Result<String> {
    let ns = namespace.unwrap_or("Agent");
    let stub = format!(
        "
namespace {ns} {{
  @mcp_principal
  entity User;
  @mcp_resource
  entity Resource;
  @mcp_context(\"session\")
  type SessionContext = {{
    hour_utc: Long,
    call_count: Long
  }};
}}
"
    );
    let config = SchemaGeneratorConfig::default().flatten_namespaces(true);
    let mut generator = SchemaGenerator::from_cedarschema_str_with_config(&stub, config).map_err(failed)?;
    let tools_json = serde_json::to_string(tools).map_err(failed)?;
    let description = ServerDescription::from_json_str(&tools_json).map_err(failed)?;
    generator
        .add_actions_from_server_description(&description)
        .map_err(failed)?;
    let schema = generator.get_schema_as_str();
    if namespace.is_some() {
        return Ok(schema);
    }
    Ok(strip_namespace(&schema))
}

/// TS: drop the leading `namespace <name> {`, the trailing `}` and every `<name>::` prefix
/// (`Agent::` when the schema doesn't start with a namespace).
fn strip_namespace(schema: &str) -> String {
    let (name, body) = namespace_header(schema).unwrap_or(("Agent", schema));
    let body = body.trim_end().strip_suffix('}').unwrap_or(body);
    body.replace(&format!("{name}::"), "")
}

/// The name and the rest of a schema starting with `namespace <name> {`.
fn namespace_header(schema: &str) -> Option<(&str, &str)> {
    let rest = schema.strip_prefix("namespace")?;
    let trimmed = rest.trim_start();
    if trimmed.len() == rest.len() {
        return None;
    }
    let end = trimmed.find(|c: char| !(c.is_alphanumeric() || c == '_'))?;
    let (name, after) = trimmed.split_at(end);
    let body = after.trim_start().strip_prefix('{')?;
    (!name.is_empty()).then_some((name, body))
}
