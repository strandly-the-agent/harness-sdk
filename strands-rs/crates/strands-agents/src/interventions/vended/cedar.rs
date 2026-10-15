//! Cedar policy authorization (`strands-ts/src/vended-interventions/cedar/**`).
//!
//! Gates every tool call on a [Cedar](https://cedarpolicy.com) policy set. A tool call maps to the
//! request `principal`, `Action::"<tool name>"`, `Resource::"agent"` with
//! `context = { input: <tool args>, session: { hour_utc, call_count, ..enricher } }`; `permit`
//! proceeds, anything else denies. Behind the `cedar` cargo feature.
//!
//! Divergences from TS, which uses `@cedar-policy/cedar-wasm`:
//! - The engine is the first-party [`cedar_policy`] crate and the schema generator is
//!   [`cedar_policy_mcp_schema_generator`], the Rust crate the TS/Python WASM bundles are built
//!   from. Policies, entities and the schema are parsed once (at construction and on
//!   [`reload`](CedarAuthorization::reload)) instead of on every tool call.
//! - Inline text and file paths are distinct ([`CedarSource`]) rather than one string whose
//!   `.cedar`/`.cedarschema` suffix decides; tools are described by this crate's [`ToolSpec`]
//!   rather than a separate MCP-shaped struct.
//! - TS's handler-level `onError` lives on the TS `InterventionHandler` base class; Rust handlers
//!   return `Result`, so [`OnError`] is a Cedar config option covering this handler's own
//!   callbacks. A principal resolver or context enricher that fails restores the call count it
//!   consumed (TS leaks it).
//! - Entity type and id are parsed into Cedar's own `EntityUid`, so Python's explicit
//!   double-quote-injection guards are unnecessary.
//!
//! Cedar has no type for JSON floats or `null`, so a tool input containing either (anywhere in
//! `context.input`) cannot be evaluated and the call is denied with `Cedar evaluation failed: ..`
//! — as in TS.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use cedar_policy::{
    Authorizer, Context, Decision, Entities, EntityId, EntityTypeName, EntityUid, PolicySet, Request, Schema,
    ValidationMode, Validator,
};
use cedar_policy_mcp_schema_generator::{SchemaGenerator, SchemaGeneratorConfig};
use mcp_tools_sdk::description::ServerDescription;
use parking_lot::{Mutex, RwLock};
use serde_json::Map;

use crate::agent::{Agent, InvocationState};
use crate::error::{Error, Result};
use crate::hooks::events::BeforeToolCallEvent;
use crate::interventions::{InterventionHandler, ToolCallAction};
use crate::types::{JsonValue, ToolSpec};

/// Agent-state key holding this handler's per-tool call counts (shared with TS and Python).
pub const CALL_COUNTS_KEY: &str = "cedar-authorization";

/// Denial reason when no principal could be resolved.
const NO_PRINCIPAL: &str = "No principal identity found in invocation state";
/// Rejection reason for an entity array element without a usable `uid`.
const INVALID_ENTITY: &str = "Invalid entity: each entity must have a uid with type and id";
/// Validation errors an auto-generated schema is allowed to report (tool-name typos).
const ACTION_ERRORS: [&str; 2] = ["unrecognized action", "unable to find an applicable action"];

/// Where a Cedar artifact comes from.
///
/// [`CedarSource::File`] is re-read by [`CedarAuthorization::reload`]; `&str`/`String` convert to
/// [`CedarSource::Text`] and `&Path`/`PathBuf` to [`CedarSource::File`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CedarSource {
    /// Inline text: Cedar policy text, `.cedarschema` text, or a Cedar entity JSON array.
    Text(String),
    /// A path on disk holding that text.
    File(PathBuf),
}

impl From<String> for CedarSource {
    fn from(text: String) -> Self {
        CedarSource::Text(text)
    }
}

impl From<&str> for CedarSource {
    fn from(text: &str) -> Self {
        CedarSource::Text(text.to_owned())
    }
}

impl From<PathBuf> for CedarSource {
    fn from(path: PathBuf) -> Self {
        CedarSource::File(path)
    }
}

impl From<&Path> for CedarSource {
    fn from(path: &Path) -> Self {
        CedarSource::File(path.to_owned())
    }
}

/// A Cedar entity reference: `entity_type::"id"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntityRef {
    /// Entity type name, namespaced where the policies are (`User`, `Agent::User`).
    pub entity_type: String,
    /// Entity id.
    pub id: String,
}

impl EntityRef {
    /// `entity_type::"id"`.
    pub fn new(entity_type: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            entity_type: entity_type.into(),
            id: id.into(),
        }
    }

    fn to_uid(&self) -> Result<EntityUid> {
        let name = EntityTypeName::from_str(&self.entity_type)
            .map_err(|e| Error::Plugin(format!("invalid entity type '{}': {e}", self.entity_type)))?;
        Ok(EntityUid::from_type_name_and_id(name, EntityId::new(&self.id)))
    }
}

/// What a [`CedarAuthorization`] gate does when one of its own callbacks fails.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum OnError {
    /// Propagate the error and fail the invocation (TS `'throw'`).
    #[default]
    Propagate,
    /// Deny the tool call (fail-closed).
    Deny,
    /// Run the tool anyway (fail-open; dangerous).
    Proceed,
}

/// Resolves the Cedar principal from the invocation state. `Ok(None)` denies (fail-closed).
pub type PrincipalResolver = Arc<dyn Fn(&InvocationState) -> Result<Option<EntityRef>> + Send + Sync>;

/// The tool call a [`ContextEnricher`] is enriching.
#[non_exhaustive]
pub struct EnrichmentContext<'a> {
    /// Name of the tool being gated.
    pub tool_name: &'a str,
    /// The tool's input arguments.
    pub tool_input: &'a JsonValue,
    /// Shared per-invocation state.
    pub invocation_state: &'a InvocationState,
}

/// Adds fields to `context.session`. Cannot overwrite `hour_utc` or `call_count`.
pub type ContextEnricher =
    Arc<dyn Fn(&EnrichmentContext<'_>) -> Result<Map<String, JsonValue>> + Send + Sync>;

/// Configuration for [`CedarAuthorization`]; build from [`CedarAuthorizationConfig::new`] and the
/// `with_*` setters.
#[derive(Clone)]
#[non_exhaustive]
pub struct CedarAuthorizationConfig {
    /// The policy set to evaluate.
    pub policies: CedarSource,
    /// Tool specs to auto-generate a validation schema from, when `schema` is unset.
    pub tools: Vec<ToolSpec>,
    /// Cedar entity JSON array, for policies that need an entity hierarchy or attributes.
    pub entities: Option<CedarSource>,
    /// Cedar schema to validate the policies against; takes precedence over `tools`.
    pub schema: Option<CedarSource>,
    /// Namespace for the generated action/resource types (`Agent::Action::"search"`).
    pub namespace: Option<String>,
    /// Static principal. Defaults to `User::"anonymous"`; conflicts with `principal_resolver`.
    pub principal: Option<EntityRef>,
    /// Per-invocation principal, for multi-tenant agents; conflicts with `principal`.
    pub principal_resolver: Option<PrincipalResolver>,
    /// Extra `context.session` fields.
    pub context_enricher: Option<ContextEnricher>,
    /// What to do when a callback fails.
    pub on_error: OnError,
}

impl CedarAuthorizationConfig {
    /// Configuration over `policies` (inline Cedar text or a `.cedar` file), everything defaulted.
    pub fn new(policies: impl Into<CedarSource>) -> Self {
        Self {
            policies: policies.into(),
            tools: Vec::new(),
            entities: None,
            schema: None,
            namespace: None,
            principal: None,
            principal_resolver: None,
            context_enricher: None,
            on_error: OnError::default(),
        }
    }

    /// Auto-generate a validation schema from these tool specs.
    #[must_use]
    pub fn with_tools(mut self, tools: impl IntoIterator<Item = ToolSpec>) -> Self {
        self.tools = tools.into_iter().collect();
        self
    }

    /// Supply the Cedar entity JSON array.
    #[must_use]
    pub fn with_entities(mut self, entities: impl Into<CedarSource>) -> Self {
        self.entities = Some(entities.into());
        self
    }

    /// Validate the policies against this schema instead of a generated one.
    #[must_use]
    pub fn with_schema(mut self, schema: impl Into<CedarSource>) -> Self {
        self.schema = Some(schema.into());
        self
    }

    /// Namespace the action and resource types.
    #[must_use]
    pub fn with_namespace(mut self, namespace: impl Into<String>) -> Self {
        self.namespace = Some(namespace.into());
        self
    }

    /// Use a fixed principal for every request.
    #[must_use]
    pub fn with_principal(mut self, principal: EntityRef) -> Self {
        self.principal = Some(principal);
        self
    }

    /// Resolve the principal from the invocation state on every request.
    #[must_use]
    pub fn with_principal_resolver(mut self, resolver: PrincipalResolver) -> Self {
        self.principal_resolver = Some(resolver);
        self
    }

    /// Add fields to `context.session`.
    #[must_use]
    pub fn with_context_enricher(mut self, enricher: ContextEnricher) -> Self {
        self.context_enricher = Some(enricher);
        self
    }

    /// Set the callback-failure policy.
    #[must_use]
    pub fn with_on_error(mut self, on_error: OnError) -> Self {
        self.on_error = on_error;
        self
    }
}

/// Where the policies, entities and schema are read from; [`CedarAuthorization::reload`] re-reads
/// exactly these.
#[derive(Clone)]
struct Sources {
    policies: CedarSource,
    entities: Option<CedarSource>,
    schema: Option<CedarSource>,
    tools: Vec<ToolSpec>,
    namespace: Option<String>,
}

/// The parsed, validated artifacts a request is evaluated against.
struct Compiled {
    policies: PolicySet,
    entities: Entities,
}

/// Authorizes each tool call against a Cedar policy set.
///
/// ```no_run
/// use strands_agents::interventions::vended::cedar::CedarAuthorization;
///
/// let cedar = CedarAuthorization::from_policies(
///     r#"permit(principal, action == Action::"search", resource);"#,
/// )?;
/// # Ok::<_, strands_agents::Error>(())
/// ```
pub struct CedarAuthorization {
    sources: Sources,
    compiled: RwLock<Compiled>,
    principal: Option<EntityRef>,
    principal_resolver: Option<PrincipalResolver>,
    context_enricher: Option<ContextEnricher>,
    on_error: OnError,
    action_type: String,
    resource: EntityRef,
    call_counts: Mutex<HashMap<String, i64>>,
    authorizer: Authorizer,
}

impl CedarAuthorization {
    /// Handler over `policies` (inline Cedar text or a `.cedar` file) with everything defaulted.
    pub fn from_policies(policies: impl Into<CedarSource>) -> Result<Self> {
        Self::new(CedarAuthorizationConfig::new(policies))
    }

    /// Handler over `config`. Fails when the policies cannot be parsed, a source file is missing,
    /// or the policies do not validate against the (supplied or generated) schema.
    pub fn new(config: CedarAuthorizationConfig) -> Result<Self> {
        if config.principal.is_some() && config.principal_resolver.is_some() {
            return Err(Error::Plugin(
                "Provide either `principal` or `principal_resolver`, not both".to_owned(),
            ));
        }
        let sources = Sources {
            policies: config.policies,
            entities: config.entities,
            schema: config.schema,
            tools: config.tools,
            namespace: config.namespace,
        };
        let compiled = compile(&sources)?;
        let namespace = sources.namespace.clone();
        let principal = match (&config.principal_resolver, config.principal) {
            (Some(_), _) => None,
            (None, Some(principal)) => Some(principal),
            (None, None) => Some(EntityRef::new(qualify(&namespace, "User"), "anonymous")),
        };
        let resource_id = if namespace.is_some() { "default" } else { "agent" };
        Ok(Self {
            sources,
            compiled: RwLock::new(compiled),
            principal,
            principal_resolver: config.principal_resolver,
            context_enricher: config.context_enricher,
            on_error: config.on_error,
            action_type: qualify(&namespace, "Action"),
            resource: EntityRef::new(qualify(&namespace, "Resource"), resource_id),
            call_counts: Mutex::new(HashMap::new()),
            authorizer: Authorizer::new(),
        })
    }

    /// Re-reads the policies, entities and schema from their sources. Validates before committing,
    /// so a broken reload leaves the live policy set untouched.
    pub fn reload(&self) -> Result<()> {
        let compiled = compile(&self.sources)?;
        *self.compiled.write() = compiled;
        Ok(())
    }

    /// Clears the rate-limit call counters, in memory and in `agent`'s state.
    pub fn reset_call_counts(&self, agent: &Agent) {
        self.call_counts.lock().clear();
        agent.state().set(CALL_COUNTS_KEY, JsonValue::Object(Map::new()));
    }

    /// `Ok(Proceed)`/`Ok(Deny)` for a decision, `Err` only for a failing user callback.
    fn authorize(&self, event: &BeforeToolCallEvent) -> Result<ToolCallAction> {
        let principal = match (&self.principal, &self.principal_resolver) {
            (Some(principal), _) => Some(principal.clone()),
            (None, Some(resolve)) => resolve(&event.invocation_state)?,
            (None, None) => None,
        };
        let Some(principal) = principal.filter(|p| !p.entity_type.is_empty() && !p.id.is_empty()) else {
            return Ok(ToolCallAction::Deny(NO_PRINCIPAL.to_owned()));
        };

        let tool_name = event.tool_use.name.clone();
        // Held across the decision so a denial's refund cannot hand budget to a concurrent call.
        let mut counts = self.call_counts.lock();
        let call_count = self.increment_call_count(&mut counts, event, &tool_name);
        match self.decide(event, &principal, &tool_name, call_count) {
            Ok(None) => Ok(ToolCallAction::Proceed),
            Ok(Some(reason)) => {
                self.decrement_call_count(&mut counts, event, &tool_name);
                Ok(ToolCallAction::Deny(reason))
            }
            Err(error) => {
                self.decrement_call_count(&mut counts, event, &tool_name);
                Err(error)
            }
        }
    }

    /// `None` permits; `Some(reason)` denies.
    fn decide(
        &self,
        event: &BeforeToolCallEvent,
        principal: &EntityRef,
        tool_name: &str,
        call_count: i64,
    ) -> Result<Option<String>> {
        let tool_input = match &event.tool_use.input {
            JsonValue::Null => JsonValue::Object(Map::new()),
            input => input.clone(),
        };
        let mut session = match &self.context_enricher {
            Some(enrich) => enrich(&EnrichmentContext {
                tool_name,
                tool_input: &tool_input,
                invocation_state: &event.invocation_state,
            })?,
            None => Map::new(),
        };
        session.insert("hour_utc".to_owned(), current_utc_hour().into());
        session.insert("call_count".to_owned(), call_count.into());
        let context = JsonValue::Object(Map::from_iter([
            ("input".to_owned(), tool_input),
            ("session".to_owned(), JsonValue::Object(session)),
        ]));

        let request = match self.build_request(principal, tool_name, context) {
            Ok(request) => request,
            Err(error) => return Ok(Some(format!("Cedar evaluation failed: {error}"))),
        };
        let compiled = self.compiled.read();
        let response = self
            .authorizer
            .is_authorized(&request, &compiled.policies, &compiled.entities);
        if response.decision() == Decision::Allow {
            return Ok(None);
        }
        let details = response
            .diagnostics()
            .reason()
            .map(ToString::to_string)
            .chain(response.diagnostics().errors().map(ToString::to_string))
            .filter(|detail| !detail.is_empty())
            .collect::<Vec<_>>();
        let suffix = if details.is_empty() {
            String::new()
        } else {
            format!(": {}", details.join(", "))
        };
        Ok(Some(format!("Access denied by Cedar policy{suffix}")))
    }

    fn build_request(&self, principal: &EntityRef, tool_name: &str, context: JsonValue) -> Result<Request> {
        let context = Context::from_json_value(context, None).map_err(|e| Error::Plugin(e.to_string()))?;
        Request::new(
            principal.to_uid()?,
            EntityRef::new(&self.action_type, tool_name).to_uid()?,
            self.resource.to_uid()?,
            context,
            None,
        )
        .map_err(|e| Error::Plugin(e.to_string()))
    }

    fn increment_call_count(
        &self,
        counts: &mut HashMap<String, i64>,
        event: &BeforeToolCallEvent,
        tool_name: &str,
    ) -> i64 {
        if counts.is_empty() {
            if let Some(stored) = event
                .agent
                .state()
                .get_as::<HashMap<String, i64>>(CALL_COUNTS_KEY)
            {
                counts.extend(stored);
            }
        }
        let next = counts.get(tool_name).copied().unwrap_or(0) + 1;
        counts.insert(tool_name.to_owned(), next);
        persist_call_counts(event, counts);
        next
    }

    fn decrement_call_count(
        &self,
        counts: &mut HashMap<String, i64>,
        event: &BeforeToolCallEvent,
        tool_name: &str,
    ) {
        match counts.get_mut(tool_name) {
            Some(current) if *current > 0 => *current -= 1,
            _ => return,
        }
        persist_call_counts(event, counts);
    }
}

#[async_trait]
impl InterventionHandler for CedarAuthorization {
    fn name(&self) -> &str {
        "cedar-authorization"
    }

    async fn before_tool_call(&self, event: &BeforeToolCallEvent) -> Result<ToolCallAction> {
        match self.authorize(event) {
            Ok(action) => Ok(action),
            Err(error) if error.is_control_flow() => Err(error),
            Err(error) => match self.on_error {
                OnError::Propagate => Err(error),
                OnError::Deny => Ok(ToolCallAction::Deny(format!("Handler threw: {error}"))),
                OnError::Proceed => {
                    tracing::warn!(
                        handler = "cedar-authorization",
                        %error,
                        "proceeding despite authorization failure"
                    );
                    Ok(ToolCallAction::Proceed)
                }
            },
        }
    }
}

fn persist_call_counts(event: &BeforeToolCallEvent, counts: &HashMap<String, i64>) {
    event.agent.state().set(
        CALL_COUNTS_KEY,
        serde_json::to_value(counts).expect("call counts are serializable"),
    );
}

fn qualify(namespace: &Option<String>, name: &str) -> String {
    match namespace {
        Some(namespace) => format!("{namespace}::{name}"),
        None => name.to_owned(),
    }
}

fn current_utc_hour() -> i64 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    ((secs / 3600) % 24) as i64
}

fn compile(sources: &Sources) -> Result<Compiled> {
    let policy_text = load(&sources.policies, "policy")?;
    let policies =
        PolicySet::from_str(&policy_text).map_err(|e| Error::Plugin(format!("Invalid Cedar policy: {e}")))?;
    let entities = load_entities(sources.entities.as_ref())?;

    let (schema, auto_generated) = match (&sources.schema, sources.tools.is_empty()) {
        (Some(schema), _) => (Some(load(schema, "schema")?), false),
        (None, false) => (
            Some(generate_schema(&sources.tools, sources.namespace.as_deref())?),
            true,
        ),
        (None, true) => (None, false),
    };
    if let Some(schema) = schema {
        validate(&policies, &schema, auto_generated)?;
    }
    Ok(Compiled { policies, entities })
}

fn load(source: &CedarSource, what: &str) -> Result<String> {
    match source {
        CedarSource::Text(text) => Ok(text.clone()),
        CedarSource::File(path) => std::fs::read_to_string(path)
            .map_err(|e| Error::Plugin(format!("Cedar {what} file not found: {} ({e})", path.display()))),
    }
}

fn load_entities(source: Option<&CedarSource>) -> Result<Entities> {
    let Some(source) = source else {
        return Ok(Entities::empty());
    };
    let json: JsonValue = serde_json::from_str(&load(source, "entity")?)
        .map_err(|e| Error::Plugin(format!("Invalid Cedar entities: {e}")))?;
    check_entity_uids(&json)?;
    Entities::from_json_value(json, None).map_err(|e| Error::Plugin(format!("Invalid Cedar entities: {e}")))
}

fn check_entity_uids(json: &JsonValue) -> Result<()> {
    let entities = json
        .as_array()
        .ok_or_else(|| Error::Plugin(INVALID_ENTITY.to_owned()))?;
    for entity in entities {
        let uid = entity
            .get("uid")
            .ok_or_else(|| Error::Plugin(INVALID_ENTITY.to_owned()))?;
        let uid = uid.get("__entity").unwrap_or(uid);
        let field = |name| uid.get(name).and_then(JsonValue::as_str).unwrap_or_default();
        if field("type").is_empty() || field("id").is_empty() {
            return Err(Error::Plugin(INVALID_ENTITY.to_owned()));
        }
    }
    Ok(())
}

fn validate(policies: &PolicySet, schema: &str, auto_generated: bool) -> Result<()> {
    let schema = Schema::from_str(schema)
        .map_err(|e| Error::Plugin(format!("Cedar policy validation failed: invalid schema: {e}")))?;
    let errors = Validator::new(schema)
        .validate(policies, ValidationMode::Strict)
        .validation_errors()
        .map(|error| (error.policy_id().to_string(), error.to_string()))
        // An auto-generated schema only describes tool inputs, not the handler-injected session
        // context, so only surface typos in tool names.
        .filter(|(_, message)| !auto_generated || ACTION_ERRORS.iter().any(|needle| message.contains(needle)))
        .map(|(policy_id, message)| format!("{policy_id}: {message}"))
        .collect::<Vec<_>>();
    if errors.is_empty() {
        return Ok(());
    }
    Err(Error::Plugin(format!(
        "Cedar policy validation failed: {}",
        errors.join(", ")
    )))
}

/// Generates a validation schema from `tools`, mirroring the stub and `flattenNamespaces` config
/// the TS and Python ports pass to the same generator.
fn generate_schema(tools: &[ToolSpec], namespace: Option<&str>) -> Result<String> {
    let ns = namespace.unwrap_or("Agent");
    let stub = format!(
        "namespace {ns} {{
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
    let mut generator =
        SchemaGenerator::from_cedarschema_str_with_config(&stub, config).map_err(generation_failed)?;
    let tools_json = JsonValue::Array(tools.iter().map(mcp_tool_json).collect());
    let description = ServerDescription::from_json_str(&tools_json.to_string()).map_err(generation_failed)?;
    generator
        .add_actions_from_server_description(&description)
        .map_err(generation_failed)?;

    let schema = generator.get_schema_as_str();
    match namespace {
        Some(_) => Ok(schema),
        // Policies in the empty namespace (`Action::"search"`) only validate against an
        // un-namespaced schema, so unwrap the generated `namespace Agent { .. }`.
        None => strip_namespace(&schema, ns),
    }
}

fn generation_failed(error: impl std::fmt::Display) -> Error {
    Error::Plugin(format!("Schema generation failed: {error}"))
}

/// The MCP tool description the generator expects. Output schemas are dropped: the generator is
/// configured for inputs only, matching TS.
fn mcp_tool_json(spec: &ToolSpec) -> JsonValue {
    serde_json::json!({
        "name": spec.name,
        "description": spec.description,
        "inputSchema": spec
            .input_schema
            .clone()
            .unwrap_or_else(|| serde_json::json!({ "type": "object", "properties": {} })),
    })
}

fn strip_namespace(schema: &str, namespace: &str) -> Result<String> {
    let body = schema
        .trim()
        .strip_prefix("namespace")
        .map(str::trim_start)
        .and_then(|rest| rest.strip_prefix(namespace))
        .map(str::trim_start)
        .and_then(|rest| rest.strip_prefix('{'))
        .map(str::trim_end)
        .and_then(|rest| rest.strip_suffix('}'));
    match body {
        Some(body) => Ok(body.replace(&format!("{namespace}::"), "")),
        None => Err(generation_failed(format!(
            "generated schema is not a `namespace {namespace} {{ .. }}` block"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `test.cedarschema` fixture from the TS suite.
    const TEST_SCHEMA: &str = r#"
entity User = {
  role: String,
};
entity Resource;

action "search" appliesTo {
  principal: [User],
  resource: [Resource],
  context: {
    input: { query: String },
    session: { hour_utc: Long, call_count: Long, environment: String },
  },
};
"#;

    fn tools() -> Vec<ToolSpec> {
        vec![
            ToolSpec::new("search", "search things").with_input_schema(serde_json::json!({
                "type": "object",
                "properties": { "query": { "type": "string" } },
                "required": ["query"],
            })),
            ToolSpec::new("delete", "delete things").with_input_schema(serde_json::json!({
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"],
            })),
        ]
    }

    fn error_of(result: Result<CedarAuthorization>) -> String {
        result.err().expect("expected a construction error").to_string()
    }

    #[test]
    fn throws_on_malformed_policy_at_construction_time() {
        let error = error_of(CedarAuthorization::from_policies(
            "this is not valid cedar syntax at all!!!",
        ));
        assert!(error.starts_with("Invalid Cedar policy: "), "{error}");
    }

    #[test]
    fn throws_when_both_principal_and_principal_resolver_are_provided() {
        let config = CedarAuthorizationConfig::new("permit(principal, action, resource);")
            .with_principal(EntityRef::new("User", "alice"))
            .with_principal_resolver(Arc::new(|_| Ok(Some(EntityRef::new("User", "alice")))));
        assert_eq!(
            error_of(CedarAuthorization::new(config)),
            "Provide either `principal` or `principal_resolver`, not both"
        );
    }

    #[test]
    fn throws_when_cedar_file_does_not_exist() {
        let error = error_of(CedarAuthorization::from_policies(Path::new(
            "/nonexistent/path.cedar",
        )));
        assert!(
            error.starts_with("Cedar policy file not found: /nonexistent/path.cedar"),
            "{error}"
        );
    }

    #[test]
    fn reloads_policies_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.cedar");
        std::fs::write(
            &path,
            r#"permit(principal, action == Action::"search", resource);"#,
        )
        .unwrap();
        let cedar = CedarAuthorization::from_policies(path).unwrap();
        cedar.reload().expect("reload of an unchanged valid file");
    }

    #[test]
    fn throws_on_reload_if_policy_file_was_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policies.cedar");
        std::fs::write(&path, "permit(principal, action, resource);").unwrap();
        let cedar = CedarAuthorization::from_policies(path.clone()).unwrap();

        std::fs::remove_file(&path).unwrap();
        let error = cedar.reload().expect_err("reload must fail").to_string();
        assert!(error.starts_with("Cedar policy file not found: "), "{error}");
    }

    #[test]
    fn validates_policies_on_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policies.cedar");
        std::fs::write(&path, "permit(principal, action, resource);").unwrap();
        let cedar = CedarAuthorization::from_policies(path.clone()).unwrap();

        std::fs::write(&path, "this is broken!!!").unwrap();
        let error = cedar.reload().expect_err("reload must fail").to_string();
        assert!(error.starts_with("Invalid Cedar policy: "), "{error}");
        // The live policy set is untouched: a second reload still reports the same failure.
        assert!(cedar.reload().is_err());
    }

    #[test]
    fn passes_validation_when_policies_match_schema() {
        let config = CedarAuthorizationConfig::new(
            r#"permit(principal is User, action == Action::"search", resource is Resource);"#,
        )
        .with_schema(TEST_SCHEMA);
        assert!(CedarAuthorization::new(config).is_ok());
    }

    #[test]
    fn passes_validation_when_the_schema_comes_from_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.cedarschema");
        std::fs::write(&path, TEST_SCHEMA).unwrap();
        let config = CedarAuthorizationConfig::new(
            r#"permit(principal is User, action == Action::"search", resource is Resource);"#,
        )
        .with_schema(path);
        assert!(CedarAuthorization::new(config).is_ok());
    }

    #[test]
    fn throws_when_schema_file_does_not_exist() {
        let config = CedarAuthorizationConfig::new("permit(principal, action, resource);")
            .with_schema(Path::new("/nonexistent/path.cedarschema"));
        let error = error_of(CedarAuthorization::new(config));
        assert!(
            error.starts_with("Cedar schema file not found: /nonexistent/path.cedarschema"),
            "{error}"
        );
    }

    #[test]
    fn throws_when_policy_references_unknown_action() {
        let config = CedarAuthorizationConfig::new(
            r#"permit(principal, action == Action::"nonexistent_tool", resource);"#,
        )
        .with_schema(TEST_SCHEMA);
        let error = error_of(CedarAuthorization::new(config));
        assert!(error.starts_with("Cedar policy validation failed: "), "{error}");
    }

    #[test]
    fn throws_when_policy_references_unknown_attribute() {
        let config = CedarAuthorizationConfig::new(
            r#"permit(principal, action == Action::"search", resource) when { principal.nonexistent == "x" };"#,
        )
        .with_schema(TEST_SCHEMA);
        let error = error_of(CedarAuthorization::new(config));
        assert!(error.starts_with("Cedar policy validation failed: "), "{error}");
    }

    #[test]
    fn accepts_inline_schema_string() {
        let config = CedarAuthorizationConfig::new(
            r#"permit(principal is User, action == Action::"search", resource is Resource);"#,
        )
        .with_schema(
            r#"
            entity User = { role: String };
            entity Resource;
            action "search" appliesTo { principal: [User], resource: [Resource] };
            "#,
        );
        assert!(CedarAuthorization::new(config).is_ok());
    }

    #[test]
    fn skips_schema_validation_when_schema_is_not_provided() {
        // References an action no schema declares; without a schema only the parse check runs.
        assert!(CedarAuthorization::from_policies(
            r#"permit(principal, action == Action::"anything", resource);"#
        )
        .is_ok());
    }

    #[test]
    fn auto_generates_schema_and_validates_policies_when_tools_are_provided() {
        let config =
            CedarAuthorizationConfig::new(r#"permit(principal, action == Action::"search", resource);"#)
                .with_tools(tools());
        let cedar = CedarAuthorization::new(config).unwrap();
        assert_eq!(cedar.name(), "cedar-authorization");
    }

    #[test]
    fn catches_unknown_action_names_via_auto_generated_schema() {
        let config =
            CedarAuthorizationConfig::new(r#"permit(principal, action == Action::"nonexistent", resource);"#)
                .with_tools(tools());
        let error = error_of(CedarAuthorization::new(config));
        assert!(error.starts_with("Cedar policy validation failed: "), "{error}");
        assert!(error.contains("unrecognized action"), "{error}");
    }

    #[test]
    fn allows_policies_referencing_context_session_not_in_the_generated_schema() {
        let config = CedarAuthorizationConfig::new(
            r#"permit(principal, action == Action::"search", resource) when { context.session.role == "admin" };"#,
        )
        .with_tools(tools());
        assert!(CedarAuthorization::new(config).is_ok());
    }

    #[test]
    fn surfaces_schema_generator_errors_for_naming_collisions_in_nested_inputs() {
        let colliding = vec![ToolSpec::new("create_user", "create a user").with_input_schema(
            serde_json::json!({
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
        let config =
            CedarAuthorizationConfig::new("permit(principal, action, resource);").with_tools(colliding);
        let error = error_of(CedarAuthorization::new(config));
        assert!(error.starts_with("Schema generation failed: "), "{error}");
    }

    #[test]
    fn the_generated_schema_is_namespaced_only_when_a_namespace_is_configured() {
        let flat = generate_schema(&tools(), None).unwrap();
        assert!(!flat.contains("namespace"), "{flat}");
        assert!(!flat.contains("Agent::"), "{flat}");
        assert!(flat.contains(r#"action "search""#), "{flat}");

        let namespaced = generate_schema(&tools(), Some("MyApp")).unwrap();
        assert!(namespaced.starts_with("namespace MyApp {"), "{namespaced}");
    }

    #[test]
    fn a_tool_spec_without_an_input_schema_still_generates_an_action() {
        let spec = ToolSpec {
            input_schema: None,
            ..ToolSpec::new("ping", "ping")
        };
        let schema = generate_schema(&[spec], None).unwrap();
        assert!(schema.contains(r#"action "ping""#), "{schema}");
    }

    #[test]
    fn entities_must_carry_a_uid_with_a_type_and_an_id() {
        let config = CedarAuthorizationConfig::new("permit(principal, action, resource);")
            .with_entities(r#"[{ "attrs": {}, "parents": [] }]"#);
        assert_eq!(error_of(CedarAuthorization::new(config)), INVALID_ENTITY);

        let config = CedarAuthorizationConfig::new("permit(principal, action, resource);")
            .with_entities(r#"[{ "uid": { "type": "User" }, "attrs": {}, "parents": [] }]"#);
        assert_eq!(error_of(CedarAuthorization::new(config)), INVALID_ENTITY);
    }

    #[test]
    fn entity_sources_accept_the_escaped_uid_form() {
        let config = CedarAuthorizationConfig::new("permit(principal, action, resource);").with_entities(
            r#"[{ "uid": { "__entity": { "type": "User", "id": "alice" } }, "attrs": {}, "parents": [] }]"#,
        );
        assert!(CedarAuthorization::new(config).is_ok());
    }

    #[test]
    fn text_and_path_convert_to_the_matching_source() {
        assert_eq!(
            CedarSource::from("permit(principal, action, resource);"),
            CedarSource::Text("permit(principal, action, resource);".to_owned())
        );
        assert_eq!(
            CedarSource::from(Path::new("/tmp/p.cedar")),
            CedarSource::File(PathBuf::from("/tmp/p.cedar"))
        );
    }

    #[test]
    fn the_utc_hour_is_a_legal_cedar_long() {
        let hour = current_utc_hour();
        assert!((0..24).contains(&hour), "{hour}");
    }

    // --- runtime decisions (the TS `real Cedar evaluation` / resource / enricher / onError /
    // session / namespace suites, driven through the handler instead of a whole agent loop) ---

    /// The `entities.json` fixture from the TS suite.
    const ENTITIES: &str = r#"[
      { "uid": { "type": "Resource", "id": "agent" }, "attrs": {}, "parents": [] },
      { "uid": { "type": "User", "id": "alice" }, "attrs": { "role": "admin" }, "parents": [] },
      { "uid": { "type": "User", "id": "bob" }, "attrs": { "role": "analyst" }, "parents": [] },
      { "uid": { "type": "User", "id": "eve" }, "attrs": { "role": "viewer" }, "parents": [] }
    ]"#;
    /// The `test.cedar` fixture.
    const PERMIT_SEARCH: &str = r#"permit(principal, action == Action::"search", resource);"#;
    /// The `role-based.cedar` fixture.
    const ROLE_BASED: &str = r#"
permit(principal is User, action, resource) when { principal.role == "admin" };
permit(principal is User, action == Action::"search", resource) when { principal.role == "analyst" };
permit(principal is User, action == Action::"query_database", resource) when { principal.role == "analyst" };
"#;
    /// The `rate-limited.cedar` fixture.
    const RATE_LIMITED: &str = r#"permit(principal, action == Action::"send_email", resource) when { context.session.call_count < 3 };"#;
    /// The `env-restricted.cedar` fixture.
    const ENV_RESTRICTED: &str =
        r#"permit(principal, action, resource) when { context.session.environment != "production" };"#;

    fn agent() -> Agent {
        Agent::builder()
            .model(crate::testing::MockMessageModel::shared(vec![]))
            .build()
            .expect("agent over a mock model")
    }

    fn event(agent: &Agent, tool: &str, input: JsonValue, state: InvocationState) -> BeforeToolCallEvent {
        BeforeToolCallEvent {
            agent: agent.clone(),
            tool_use: crate::types::ToolUse {
                name: tool.to_owned(),
                tool_use_id: "call-1".to_owned(),
                input,
            },
            tool: None,
            invocation_state: state,
            cancel: crate::hooks::events::Cancel::No,
            selected_tool: None,
        }
    }

    /// Gates one tool call: `Ok(None)` permits, `Ok(Some(reason))` denies.
    async fn gate(
        cedar: &CedarAuthorization,
        agent: &Agent,
        tool: &str,
        input: JsonValue,
        state: InvocationState,
    ) -> Result<Option<String>> {
        match cedar.before_tool_call(&event(agent, tool, input, state)).await? {
            ToolCallAction::Proceed => Ok(None),
            ToolCallAction::Deny(reason) => Ok(Some(reason)),
            _ => panic!("cedar only proceeds or denies"),
        }
    }

    fn no_state() -> InvocationState {
        InvocationState::new()
    }

    fn state_with(key: &str, value: &str) -> InvocationState {
        let state = InvocationState::new();
        state.insert(key, value.to_owned());
        state
    }

    /// `User::"<invocation_state.user_id>"`, or no principal at all when it is unset.
    fn user_resolver() -> PrincipalResolver {
        Arc::new(|state: &InvocationState| {
            Ok(state
                .get::<String>("user_id")
                .map(|id| EntityRef::new("User", id.as_str())))
        })
    }

    fn fixed_user(entity_type: &'static str, id: &'static str) -> PrincipalResolver {
        Arc::new(move |_| Ok(Some(EntityRef::new(entity_type, id))))
    }

    fn failing_resolver() -> PrincipalResolver {
        Arc::new(|_| Err(Error::Plugin("resolver crash".to_owned())))
    }

    #[tokio::test]
    async fn allows_permitted_tool_calls() {
        let cedar = CedarAuthorization::new(
            CedarAuthorizationConfig::new(PERMIT_SEARCH)
                .with_entities(ENTITIES)
                .with_principal_resolver(user_resolver()),
        )
        .unwrap();

        let denial = gate(
            &cedar,
            &agent(),
            "search",
            serde_json::json!({ "query": "test" }),
            state_with("user_id", "alice"),
        )
        .await
        .unwrap();
        assert_eq!(denial, None);
    }

    #[tokio::test]
    async fn denies_tools_not_in_any_permit_policy() {
        let cedar = CedarAuthorization::new(
            CedarAuthorizationConfig::new(PERMIT_SEARCH)
                .with_entities(ENTITIES)
                .with_principal_resolver(fixed_user("User", "alice")),
        )
        .unwrap();

        let denial = gate(
            &cedar,
            &agent(),
            "delete_record",
            serde_json::json!({ "id": "1" }),
            no_state(),
        )
        .await
        .unwrap()
        .expect("default-deny");
        assert!(denial.starts_with("Access denied by Cedar policy"), "{denial}");
    }

    #[tokio::test]
    async fn enforces_role_based_access_admin_can_delete_analyst_cannot() {
        let config = |id: &'static str| {
            CedarAuthorizationConfig::new(ROLE_BASED)
                .with_entities(ENTITIES)
                .with_principal_resolver(fixed_user("User", id))
        };
        let admin = CedarAuthorization::new(config("alice")).unwrap();
        let analyst = CedarAuthorization::new(config("bob")).unwrap();

        let agent = agent();
        assert_eq!(
            gate(&admin, &agent, "delete_record", serde_json::json!({}), no_state())
                .await
                .unwrap(),
            None
        );
        assert!(
            gate(
                &analyst,
                &agent,
                "delete_record",
                serde_json::json!({}),
                no_state()
            )
            .await
            .unwrap()
            .is_some(),
            "an analyst must not delete"
        );
    }

    #[tokio::test]
    async fn enforces_role_based_access_analyst_can_search_viewer_cannot() {
        let config = |id: &'static str| {
            CedarAuthorizationConfig::new(ROLE_BASED)
                .with_entities(ENTITIES)
                .with_principal_resolver(fixed_user("User", id))
        };
        let analyst = CedarAuthorization::new(config("bob")).unwrap();
        let viewer = CedarAuthorization::new(config("eve")).unwrap();

        let agent = agent();
        assert_eq!(
            gate(&analyst, &agent, "search", serde_json::json!({}), no_state())
                .await
                .unwrap(),
            None
        );
        assert!(
            gate(&viewer, &agent, "search", serde_json::json!({}), no_state())
                .await
                .unwrap()
                .is_some(),
            "a viewer must not search"
        );
    }

    #[tokio::test]
    async fn enforces_rate_limits_via_call_count_in_session_context() {
        let cedar = CedarAuthorization::new(
            CedarAuthorizationConfig::new(RATE_LIMITED)
                .with_entities(ENTITIES)
                .with_principal_resolver(fixed_user("User", "alice")),
        )
        .unwrap();

        let agent = agent();
        let mut permitted = 0;
        for _ in 0..4 {
            if gate(&cedar, &agent, "send_email", serde_json::json!({}), no_state())
                .await
                .unwrap()
                .is_none()
            {
                permitted += 1;
            }
        }
        // `call_count < 3` is evaluated with this call included, so calls 1 and 2 pass.
        assert_eq!(permitted, 2);
    }

    #[tokio::test]
    async fn enforces_environment_restrictions_via_context_enricher() {
        let cedar = CedarAuthorization::new(
            CedarAuthorizationConfig::new(ENV_RESTRICTED)
                .with_entities(ENTITIES)
                .with_principal_resolver(fixed_user("User", "alice"))
                .with_context_enricher(Arc::new(|ctx| {
                    let environment = ctx
                        .invocation_state
                        .get::<String>("environment")
                        .map_or_else(|| "unknown".to_owned(), |e| e.to_string());
                    Ok(Map::from_iter([("environment".to_owned(), environment.into())]))
                })),
        )
        .unwrap();

        let agent = agent();
        assert_eq!(
            gate(
                &cedar,
                &agent,
                "search",
                serde_json::json!({}),
                state_with("environment", "development")
            )
            .await
            .unwrap(),
            None
        );
        assert!(
            gate(
                &cedar,
                &agent,
                "search",
                serde_json::json!({}),
                state_with("environment", "production")
            )
            .await
            .unwrap()
            .is_some(),
            "production must be denied"
        );
    }

    #[tokio::test]
    async fn denies_when_principal_is_missing() {
        let cedar = CedarAuthorization::new(
            CedarAuthorizationConfig::new(PERMIT_SEARCH)
                .with_entities(ENTITIES)
                .with_principal_resolver(user_resolver()),
        )
        .unwrap();

        let denial = gate(&cedar, &agent(), "search", serde_json::json!({}), no_state())
            .await
            .unwrap();
        assert_eq!(denial.as_deref(), Some(NO_PRINCIPAL));
    }

    #[tokio::test]
    async fn supports_a_static_principal_without_invocation_state() {
        let cedar = CedarAuthorization::new(
            CedarAuthorizationConfig::new(PERMIT_SEARCH)
                .with_entities(ENTITIES)
                .with_principal(EntityRef::new("User", "alice")),
        )
        .unwrap();

        assert_eq!(
            gate(&cedar, &agent(), "search", serde_json::json!({}), no_state())
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn defaults_to_an_anonymous_user_principal() {
        let cedar =
            CedarAuthorization::new(CedarAuthorizationConfig::new(PERMIT_SEARCH).with_entities(ENTITIES))
                .unwrap();

        assert_eq!(
            gate(&cedar, &agent(), "search", serde_json::json!({}), no_state())
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn uses_an_unconstrained_resource_by_default() {
        // No entity store at all: the default `Resource::"agent"` still satisfies `resource`.
        let cedar = CedarAuthorization::new(
            CedarAuthorizationConfig::new(PERMIT_SEARCH).with_principal_resolver(fixed_user("User", "alice")),
        )
        .unwrap();

        assert_eq!(
            gate(&cedar, &agent(), "search", serde_json::json!({}), no_state())
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn constrains_on_tool_arguments_via_context_input() {
        let cedar = CedarAuthorization::new(
            CedarAuthorizationConfig::new(
                r#"permit(principal, action == Action::"delete", resource) when { context.input.record_id == "42" };"#,
            )
            .with_principal_resolver(fixed_user("User", "alice")),
        )
        .unwrap();

        let agent = agent();
        assert_eq!(
            gate(
                &cedar,
                &agent,
                "delete",
                serde_json::json!({ "record_id": "42" }),
                no_state()
            )
            .await
            .unwrap(),
            None
        );
        assert!(
            gate(
                &cedar,
                &agent,
                "delete",
                serde_json::json!({ "record_id": "99" }),
                no_state()
            )
            .await
            .unwrap()
            .is_some(),
            "record 99 must be denied"
        );
    }

    #[tokio::test]
    async fn an_enricher_adds_custom_fields_usable_in_policies() {
        let cedar = |department: &'static str| {
            CedarAuthorization::new(
                CedarAuthorizationConfig::new(
                    r#"permit(principal, action, resource) when { context.session.department == "engineering" };"#,
                )
                .with_entities(ENTITIES)
                .with_principal_resolver(fixed_user("User", "alice"))
                .with_context_enricher(Arc::new(move |_| {
                    Ok(Map::from_iter([("department".to_owned(), department.into())]))
                })),
            )
            .unwrap()
        };

        let agent = agent();
        assert_eq!(
            gate(
                &cedar("engineering"),
                &agent,
                "search",
                serde_json::json!({}),
                no_state()
            )
            .await
            .unwrap(),
            None
        );
        assert!(
            gate(
                &cedar("marketing"),
                &agent,
                "search",
                serde_json::json!({}),
                no_state()
            )
            .await
            .unwrap()
            .is_some(),
            "a mismatching enricher value must deny"
        );
    }

    #[tokio::test]
    async fn propagates_by_default_when_a_callback_fails() {
        let cedar = CedarAuthorization::new(
            CedarAuthorizationConfig::new("permit(principal, action, resource);")
                .with_principal_resolver(failing_resolver()),
        )
        .unwrap();

        let error = gate(&cedar, &agent(), "tool", serde_json::json!({}), no_state())
            .await
            .expect_err("the resolver error must propagate")
            .to_string();
        assert!(error.contains("resolver crash"), "{error}");
    }

    #[tokio::test]
    async fn denies_when_on_error_is_deny_and_a_callback_fails() {
        let cedar = CedarAuthorization::new(
            CedarAuthorizationConfig::new("permit(principal, action, resource);")
                .with_principal_resolver(failing_resolver())
                .with_on_error(OnError::Deny),
        )
        .unwrap();

        let denial = gate(&cedar, &agent(), "tool", serde_json::json!({}), no_state())
            .await
            .unwrap()
            .expect("a failing callback must deny");
        assert!(denial.starts_with("Handler threw: "), "{denial}");
        assert!(denial.contains("resolver crash"), "{denial}");
    }

    #[tokio::test]
    async fn proceeds_when_on_error_is_proceed_and_a_callback_fails() {
        let cedar = CedarAuthorization::new(
            CedarAuthorizationConfig::new("permit(principal, action, resource);")
                .with_principal_resolver(failing_resolver())
                .with_on_error(OnError::Proceed),
        )
        .unwrap();

        assert_eq!(
            gate(&cedar, &agent(), "tool", serde_json::json!({}), no_state())
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn reads_a_cedar_policy_file_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.cedar");
        std::fs::write(&path, PERMIT_SEARCH).unwrap();
        let cedar = CedarAuthorization::new(
            CedarAuthorizationConfig::new(path).with_principal_resolver(fixed_user("User", "alice")),
        )
        .unwrap();

        assert_eq!(
            gate(&cedar, &agent(), "search", serde_json::json!({}), no_state())
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn reads_a_json_entity_file_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("entities.json");
        std::fs::write(&path, ENTITIES).unwrap();
        let cedar = CedarAuthorization::new(
            CedarAuthorizationConfig::new(ROLE_BASED)
                .with_entities(path)
                .with_principal_resolver(fixed_user("User", "alice")),
        )
        .unwrap();

        // `principal.role == "admin"` only holds if the entity attributes came off disk.
        assert_eq!(
            gate(
                &cedar,
                &agent(),
                "delete_record",
                serde_json::json!({}),
                no_state()
            )
            .await
            .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn reset_call_counts_re_enables_rate_limited_tools() {
        let cedar = CedarAuthorization::new(
            CedarAuthorizationConfig::new(
                "permit(principal, action, resource) when { context.session.call_count < 2 };",
            )
            .with_principal_resolver(fixed_user("User", "alice")),
        )
        .unwrap();

        let agent = agent();
        assert_eq!(
            gate(&cedar, &agent, "send_email", serde_json::json!({}), no_state())
                .await
                .unwrap(),
            None
        );
        assert!(
            gate(&cedar, &agent, "send_email", serde_json::json!({}), no_state())
                .await
                .unwrap()
                .is_some(),
            "the second call exceeds the limit"
        );

        cedar.reset_call_counts(&agent);
        assert_eq!(
            gate(&cedar, &agent, "send_email", serde_json::json!({}), no_state())
                .await
                .unwrap(),
            None,
            "the counter was reset"
        );
    }

    #[tokio::test]
    async fn uses_namespaced_action_and_resource_types() {
        let entities = r#"[
          { "uid": { "type": "Agent::Role", "id": "admin" }, "attrs": {}, "parents": [] },
          { "uid": { "type": "Agent::User", "id": "alice" }, "attrs": {},
            "parents": [{ "type": "Agent::Role", "id": "admin" }] },
          { "uid": { "type": "Agent::User", "id": "bob" }, "attrs": {}, "parents": [] },
          { "uid": { "type": "Agent::Resource", "id": "default" }, "attrs": {}, "parents": [] }
        ]"#;
        let cedar = CedarAuthorization::new(
            CedarAuthorizationConfig::new(
                r#"permit(principal in Agent::Role::"admin", action == Agent::Action::"search", resource);"#,
            )
            .with_namespace("Agent")
            .with_entities(entities)
            .with_principal_resolver(Arc::new(|state: &InvocationState| {
                Ok(state
                    .get::<String>("user_id")
                    .map(|id| EntityRef::new("Agent::User", id.as_str())))
            })),
        )
        .unwrap();

        let agent = agent();
        assert_eq!(
            gate(
                &cedar,
                &agent,
                "search",
                serde_json::json!({}),
                state_with("user_id", "alice")
            )
            .await
            .unwrap(),
            None
        );
        assert!(
            gate(
                &cedar,
                &agent,
                "search",
                serde_json::json!({}),
                state_with("user_id", "bob")
            )
            .await
            .unwrap()
            .is_some(),
            "bob is not in the admin role"
        );
    }

    #[tokio::test]
    async fn works_with_a_namespace_and_tools() {
        let cedar = CedarAuthorization::new(
            CedarAuthorizationConfig::new(
                r#"permit(principal, action == MyApp::Action::"search", resource);"#,
            )
            .with_namespace("MyApp")
            .with_tools(tools())
            .with_entities(
                r#"[{ "uid": { "type": "MyApp::Resource", "id": "default" }, "attrs": {}, "parents": [] }]"#,
            ),
        )
        .unwrap();

        assert_eq!(
            gate(
                &cedar,
                &agent(),
                "search",
                serde_json::json!({ "query": "test" }),
                no_state()
            )
            .await
            .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn denies_tool_input_cedar_cannot_represent() {
        let cedar = CedarAuthorization::new(
            CedarAuthorizationConfig::new("permit(principal, action, resource);")
                .with_principal_resolver(fixed_user("User", "alice")),
        )
        .unwrap();

        let agent = agent();
        for input in [
            serde_json::json!({ "temperature": 0.5 }),
            serde_json::json!({ "note": null }),
        ] {
            let denial = gate(&cedar, &agent, "search", input.clone(), no_state())
                .await
                .unwrap()
                .unwrap_or_else(|| panic!("{input} has no Cedar type, so it must be denied"));
            assert!(denial.starts_with("Cedar evaluation failed: "), "{denial}");
        }
    }

    #[tokio::test]
    async fn an_enricher_cannot_overwrite_hour_utc_or_call_count() {
        let cedar = CedarAuthorization::new(
            CedarAuthorizationConfig::new(
                r#"permit(principal, action, resource) when { context.session.hour_utc != 99 && context.session.call_count == 1 };"#,
            )
            .with_principal_resolver(fixed_user("User", "alice"))
            .with_context_enricher(Arc::new(|_| {
                Ok(Map::from_iter([
                    ("hour_utc".to_owned(), 99.into()),
                    ("call_count".to_owned(), 99.into()),
                ]))
            })),
        )
        .unwrap();

        assert_eq!(
            gate(&cedar, &agent(), "search", serde_json::json!({}), no_state())
                .await
                .unwrap(),
            None
        );
    }

    #[test]
    fn validates_nested_tool_input_schemas_without_breaking() {
        let nested = ToolSpec::new("create_user", "create a user").with_input_schema(serde_json::json!({
            "type": "object",
            "properties": {
                "user": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "address": {
                            "type": "object",
                            "properties": { "city": { "type": "string" } },
                        },
                    },
                },
            },
            "required": ["user"],
        }));
        let schema = generate_schema(std::slice::from_ref(&nested), None).unwrap();
        assert!(schema.contains(r#"action "create_user""#), "{schema}");

        CedarAuthorization::new(
            CedarAuthorizationConfig::new(r#"permit(principal, action == Action::"create_user", resource);"#)
                .with_tools(vec![nested]),
        )
        .expect("a nested input schema must still validate");
    }

    #[test]
    fn stripping_a_namespace_from_an_unnamespaced_schema_is_an_error() {
        let error = strip_namespace("entity User;", "Agent").expect_err("expected an error");
        assert!(
            error.to_string().starts_with("Schema generation failed: "),
            "{error}"
        );
    }
}
