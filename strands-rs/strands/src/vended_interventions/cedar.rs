//! Cedar policy authorization of tool calls (TS `vended-interventions/cedar/`).
//!
//! A tool call is the Cedar request `principal`, `Action::"<tool name>"`, `Resource::"agent"`
//! with `context = { input: <tool input>, session: { hour_utc, call_count, ..enricher } }`.
//! TS evaluates with `@cedar-policy/cedar-wasm`; this is the `cedar-policy` crate the WASM
//! build is made from, and `cedar-policy-mcp-schema-generator` for the schema.

mod schema_generator;

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use cedar_policy::{
    AuthorizationError, Authorizer, Context, Decision, Entities, EntityId, EntityTypeName, EntityUid,
    PolicySet, Request, Schema, ValidationMode, Validator,
};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::hooks::BeforeToolCallEvent;
use crate::interventions::{BeforeToolCallAction, InterventionHandler, OnError, deny, proceed};
use crate::state_store::StateStore;
use crate::types::agent::InvocationState;
use crate::types::json::{JsonObject, JsonValue};

const INVALID_ENTITY: &str = "Invalid entity: each entity must have a uid with type and id";
/// The validation errors an auto-generated schema may report: typos in tool names.
const ACTION_ERRORS: [&str; 2] = ["unrecognized action", "unable to find an applicable action"];

/// A tool the validation schema is generated from (TS `ToolDefinition`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
pub struct ToolDefinition {
    /// The tool name, which is the Cedar action id.
    pub name: String,
    /// The tool's JSON input schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<JsonValue>,
    /// The tool description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl ToolDefinition {
    /// A tool with no input schema or description.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            input_schema: None,
            description: None,
        }
    }
}

/// A Cedar entity reference (TS `TypeAndId`): `type::"id"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
pub struct TypeAndId {
    /// The entity type, namespaced where the policies are (`User`, `Agent::User`).
    #[serde(rename = "type")]
    pub type_name: String,
    /// The entity id.
    pub id: String,
}

impl TypeAndId {
    /// `type_name::"id"`.
    pub fn new(type_name: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            type_name: type_name.into(),
            id: id.into(),
        }
    }

    fn to_uid(&self) -> std::result::Result<EntityUid, String> {
        let name = EntityTypeName::from_str(&self.type_name).map_err(|error| error.to_string())?;
        Ok(EntityUid::from_type_name_and_id(name, EntityId::new(&self.id)))
    }
}

/// The entities policies can refer to (TS `entities`), in Cedar's entity JSON format.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum CedarEntities {
    /// Entity objects.
    Json(Vec<JsonValue>),
    /// A JSON file holding the entity array; read again by [`CedarAuthorization::reload`].
    File(PathBuf),
}

impl From<Vec<JsonValue>> for CedarEntities {
    fn from(entities: Vec<JsonValue>) -> Self {
        Self::Json(entities)
    }
}

impl From<PathBuf> for CedarEntities {
    fn from(path: PathBuf) -> Self {
        Self::File(path)
    }
}

/// What a context enricher sees (the argument of TS `contextEnricher`).
#[derive(Debug)]
#[non_exhaustive]
pub struct ContextEnricherInput<'a> {
    /// The tool being called.
    pub tool_name: &'a str,
    /// The tool input.
    pub tool_input: &'a JsonValue,
    /// The caller's per-invocation values.
    pub invocation_state: &'a InvocationState,
}

/// Resolves the principal from the invocation state; `None` denies the call.
pub type PrincipalResolver = Arc<dyn Fn(&InvocationState) -> Result<Option<TypeAndId>> + Send + Sync>;

/// Adds fields to `context.session`.
pub type ContextEnricher = Arc<dyn Fn(&ContextEnricherInput<'_>) -> Result<JsonObject> + Send + Sync>;

/// Configuration of [`CedarAuthorization`] (TS `CedarAuthorizationConfig`).
#[derive(Clone, Default, bon::Builder)]
#[builder(on(String, into))]
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
pub struct CedarAuthorizationConfig {
    /// Cedar policy text, or the path of a `.cedar` file.
    pub policies: String,
    /// Tools to generate a validation schema from, when `schema` is unset.
    pub tools: Option<Vec<ToolDefinition>>,
    /// Entities for policy evaluation (role memberships and the like).
    #[builder(into)]
    pub entities: Option<CedarEntities>,
    /// Cedar schema text, or the path of a `.cedarschema` file; validates the policies.
    pub schema: Option<String>,
    /// The namespace of the action, resource and default principal types.
    pub namespace: Option<String>,
    /// A fixed principal. Without it and `principal_resolver`, `User::"anonymous"`.
    pub principal: Option<TypeAndId>,
    /// Resolves the principal for each call.
    #[builder(with = |resolver: impl Fn(&InvocationState) -> Result<Option<TypeAndId>> + Send + Sync + 'static| Arc::new(resolver) as PrincipalResolver)]
    pub principal_resolver: Option<PrincipalResolver>,
    /// Adds fields to `context.session`.
    #[builder(with = |enricher: impl Fn(&ContextEnricherInput<'_>) -> Result<JsonObject> + Send + Sync + 'static| Arc::new(enricher) as ContextEnricher)]
    pub context_enricher: Option<ContextEnricher>,
    /// What to do when the resolver or the enricher fails (default [`OnError::Throw`]).
    pub on_error: Option<OnError>,
}

impl fmt::Debug for CedarAuthorizationConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CedarAuthorizationConfig")
            .field("policies", &self.policies)
            .field("tools", &self.tools)
            .field("entities", &self.entities)
            .field("schema", &self.schema)
            .field("namespace", &self.namespace)
            .field("principal", &self.principal)
            .field("on_error", &self.on_error)
            .finish_non_exhaustive()
    }
}

/// The policies and entities in force.
struct Loaded {
    policies: PolicySet,
    entities: Vec<JsonValue>,
}

/// Authorizes each tool call against Cedar policies (TS `CedarAuthorization`).
///
/// ```
/// use strands::vended_interventions::{CedarAuthorization, CedarAuthorizationConfig};
///
/// let cedar = CedarAuthorization::new(
///     CedarAuthorizationConfig::builder()
///         .policies(r#"permit(principal, action == Action::"search", resource);"#)
///         .build(),
/// )?;
/// let agent = strands::Agent::builder().intervention(cedar).build()?;
/// # Ok::<(), strands::Error>(())
/// ```
pub struct CedarAuthorization {
    on_error: OnError,
    policy_source: String,
    entity_source: Option<CedarEntities>,
    schema_source: Option<String>,
    tools: Option<Vec<ToolDefinition>>,
    namespace: Option<String>,
    principal: Option<TypeAndId>,
    principal_resolver: Option<PrincipalResolver>,
    context_enricher: Option<ContextEnricher>,
    action_type: String,
    resource: TypeAndId,
    loaded: Mutex<Loaded>,
    call_counts: Mutex<JsonObject>,
}

impl CedarAuthorization {
    /// The handler's name.
    pub const NAME: &'static str = "cedar-authorization";
    /// The app-state key holding the per-tool call counts.
    pub const STATE_KEY: &'static str = "cedar-authorization";

    /// A handler from `config`.
    ///
    /// # Errors
    ///
    /// Both `principal` and `principal_resolver` are set; a policy, schema or entity file is
    /// missing; the policies don't parse or don't validate against the schema; an entity has
    /// no uid; or the schema can't be generated from `tools`.
    pub fn new(config: CedarAuthorizationConfig) -> Result<Self> {
        if config.principal.is_some() && config.principal_resolver.is_some() {
            return Err(Error::other(
                "Provide either `principal` or `principalResolver`, not both",
            ));
        }
        let namespace = config.namespace;
        let qualified = |name: &str| match &namespace {
            Some(namespace) => format!("{namespace}::{name}"),
            None => name.to_owned(),
        };
        let principal = match config.principal_resolver {
            Some(_) => None,
            None => Some(
                config
                    .principal
                    .unwrap_or_else(|| TypeAndId::new(qualified("User"), "anonymous")),
            ),
        };
        let resource_id = if namespace.is_some() { "default" } else { "agent" };
        let loaded = load(
            &config.policies,
            config.entities.as_ref(),
            config.schema.as_deref(),
            config.tools.as_deref(),
            namespace.as_deref(),
        )?;
        Ok(Self {
            on_error: config.on_error.unwrap_or_default(),
            policy_source: config.policies,
            entity_source: config.entities,
            schema_source: config.schema,
            tools: config.tools,
            principal,
            principal_resolver: config.principal_resolver,
            context_enricher: config.context_enricher,
            action_type: qualified("Action"),
            resource: TypeAndId::new(qualified("Resource"), resource_id),
            namespace,
            loaded: Mutex::new(loaded),
            call_counts: Mutex::new(JsonObject::new()),
        })
    }

    /// Clears the call counts, here and in `app_state` (TS `resetCallCounts`).
    ///
    /// # Errors
    ///
    /// The state store rejects the write.
    pub fn reset_call_counts(&self, app_state: &StateStore) -> Result<()> {
        self.counts().clear();
        app_state.set(Self::STATE_KEY, JsonObject::new())
    }

    /// Reads the policies, entities and schema again and swaps them in if they are valid
    /// (TS `reload`).
    ///
    /// # Errors
    ///
    /// As [`Self::new`]; the handler keeps what it had.
    pub fn reload(&self) -> Result<()> {
        let loaded = load(
            &self.policy_source,
            self.entity_source.as_ref(),
            self.schema_source.as_deref(),
            self.tools.as_deref(),
            self.namespace.as_deref(),
        )?;
        *self.loaded.lock().unwrap_or_else(PoisonError::into_inner) = loaded;
        Ok(())
    }

    fn counts(&self) -> MutexGuard<'_, JsonObject> {
        self.call_counts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn increment_call_count(&self, app_state: &StateStore, tool_name: &str) -> u64 {
        let mut counts = self.counts();
        if counts.is_empty()
            && let Some(JsonValue::Object(stored)) = app_state.get_value(Self::STATE_KEY)
        {
            counts.extend(stored);
        }
        let current = counts.get(tool_name).and_then(JsonValue::as_u64).unwrap_or(0);
        let next = current.saturating_add(1);
        counts.insert(tool_name.to_owned(), next.into());
        store_counts(app_state, &counts);
        next
    }

    fn decrement_call_count(&self, app_state: &StateStore, tool_name: &str) {
        let mut counts = self.counts();
        let current = counts.get(tool_name).and_then(JsonValue::as_u64).unwrap_or(0);
        if current > 0 {
            counts.insert(tool_name.to_owned(), (current - 1).into());
            store_counts(app_state, &counts);
        }
    }

    /// The Cedar decision: `Ok(None)` permits, `Ok(Some(reason))` denies.
    fn authorize(
        &self,
        principal: &TypeAndId,
        tool_name: &str,
        context: JsonObject,
    ) -> std::result::Result<Option<String>, String> {
        let principal = principal.to_uid()?;
        let action = TypeAndId::new(self.action_type.as_str(), tool_name).to_uid()?;
        let resource = self.resource.to_uid()?;
        let context =
            Context::from_json_value(JsonValue::Object(context), None).map_err(|error| error.to_string())?;
        let request =
            Request::new(principal, action, resource, context, None).map_err(|error| error.to_string())?;
        let loaded = self.loaded.lock().unwrap_or_else(PoisonError::into_inner);
        let entities = Entities::from_json_value(JsonValue::Array(loaded.entities.clone()), None)
            .map_err(|error| error.to_string())?;
        let response = Authorizer::new().is_authorized(&request, &loaded.policies, &entities);
        if response.decision() == Decision::Allow {
            return Ok(None);
        }
        let diagnostics = response.diagnostics();
        let details: Vec<String> = diagnostics
            .reason()
            .map(ToString::to_string)
            .chain(diagnostics.errors().map(|error| match error {
                AuthorizationError::PolicyEvaluationError(error) => error.inner().to_string(),
            }))
            .filter(|detail| !detail.is_empty())
            .collect();
        Ok(Some(if details.is_empty() {
            "Access denied by Cedar policy".to_owned()
        } else {
            format!("Access denied by Cedar policy: {}", details.join(", "))
        }))
    }
}

impl fmt::Debug for CedarAuthorization {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CedarAuthorization")
            .field("policies", &self.policy_source)
            .field("namespace", &self.namespace)
            .field("on_error", &self.on_error)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl InterventionHandler for CedarAuthorization {
    fn name(&self) -> &str {
        Self::NAME
    }

    fn on_error(&self) -> OnError {
        self.on_error
    }

    async fn before_tool_call(&self, event: &mut BeforeToolCallEvent) -> Result<BeforeToolCallAction> {
        let invocation_state = event.invocation_state();
        let principal = match (&self.principal, &self.principal_resolver) {
            (Some(principal), _) => Some(principal.clone()),
            (None, Some(resolver)) => resolver(invocation_state)?,
            (None, None) => None,
        };
        let Some(principal) =
            principal.filter(|principal| !principal.type_name.is_empty() && !principal.id.is_empty())
        else {
            return Ok(deny("No principal identity found in invocation state").into());
        };

        let app_state = event.agent().app_state();
        let tool_name = event.tool_use().name();
        let call_count = self.increment_call_count(app_state, tool_name);
        let empty = JsonValue::Object(JsonObject::new());
        let tool_input = match event.tool_use().input() {
            JsonValue::Null => &empty,
            input => input,
        };
        let mut session = match &self.context_enricher {
            Some(enricher) => enricher(&ContextEnricherInput {
                tool_name,
                tool_input,
                invocation_state,
            })?,
            None => JsonObject::new(),
        };
        session.insert("hour_utc".to_owned(), utc_hour(SystemTime::now()).into());
        session.insert("call_count".to_owned(), call_count.into());
        let mut context = JsonObject::new();
        context.insert("input".to_owned(), tool_input.clone());
        context.insert("session".to_owned(), JsonValue::Object(session));

        let denied = match self.authorize(&principal, tool_name, context) {
            Ok(None) => return Ok(proceed().into()),
            Ok(Some(reason)) => reason,
            Err(error) => format!("Cedar evaluation failed: {error}"),
        };
        self.decrement_call_count(app_state, tool_name);
        Ok(deny(denied).into())
    }
}

fn store_counts(app_state: &StateStore, counts: &JsonObject) {
    if let Err(error) = app_state.set(CedarAuthorization::STATE_KEY, counts) {
        tracing::warn!("failed to store the Cedar call counts: {error}");
    }
}

fn utc_hour(now: SystemTime) -> u64 {
    let seconds = now
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    (seconds / 3600) % 24
}

fn load(
    policies: &str,
    entities: Option<&CedarEntities>,
    schema: Option<&str>,
    tools: Option<&[ToolDefinition]>,
    namespace: Option<&str>,
) -> Result<Loaded> {
    let policies = load_text(policies, ".cedar", "policy")?;
    let entities = load_entities(entities)?;
    let (schema, auto_generated) = match (schema.filter(|schema| !schema.is_empty()), tools) {
        (Some(schema), _) => (Some(load_text(schema, ".cedarschema", "schema")?), false),
        (None, Some(tools)) => (Some(schema_generator::generate_schema(tools, namespace)?), true),
        (None, None) => (None, false),
    };
    let policies = validate_policies(&policies, schema.as_deref(), auto_generated)?;
    Ok(Loaded { policies, entities })
}

/// `source` itself, or the content of the file it names when it ends with `suffix`.
fn load_text(source: &str, suffix: &str, what: &str) -> Result<String> {
    if !source.ends_with(suffix) {
        return Ok(source.to_owned());
    }
    if !std::path::Path::new(source).exists() {
        return Err(Error::other(format!("Cedar {what} file not found: {source}")));
    }
    std::fs::read_to_string(source).map_err(Error::other)
}

fn load_entities(entities: Option<&CedarEntities>) -> Result<Vec<JsonValue>> {
    let parsed = match entities {
        None => return Ok(Vec::new()),
        Some(CedarEntities::Json(entities)) => entities.clone(),
        Some(CedarEntities::File(path)) => {
            let text = std::fs::read_to_string(path).map_err(Error::other)?;
            serde_json::from_str(&text).map_err(Error::other)?
        }
    };
    for entity in &parsed {
        let uid = entity
            .get("uid")
            .filter(|uid| !uid.is_null())
            .ok_or_else(|| Error::other(INVALID_ENTITY))?;
        let uid = uid.get("__entity").unwrap_or(uid);
        let present = |field| {
            uid.get(field)
                .and_then(JsonValue::as_str)
                .is_some_and(|value| !value.is_empty())
        };
        if !present("type") || !present("id") {
            return Err(Error::other(INVALID_ENTITY));
        }
    }
    Ok(parsed)
}

fn validate_policies(policies: &str, schema: Option<&str>, auto_generated: bool) -> Result<PolicySet> {
    let policies = PolicySet::from_str(policies).map_err(|errors| {
        let messages: Vec<String> = errors.iter().map(ToString::to_string).collect();
        Error::other(format!("Invalid Cedar policy: {}", messages.join(", ")))
    })?;
    let Some(schema) = schema.filter(|schema| !schema.is_empty()) else {
        return Ok(policies);
    };
    let (schema, _warnings) = Schema::from_cedarschema_str(schema)
        .map_err(|error| Error::other(format!("Cedar policy validation failed: {error}")))?;
    let errors: Vec<String> = Validator::new(schema)
        .validate(&policies, ValidationMode::Strict)
        .validation_errors()
        .map(|error| (error.policy_id().to_string(), error.to_string()))
        // A generated schema knows the tool inputs, not the session fields the handler adds,
        // so only typos in tool names count.
        .filter(|(_, message)| !auto_generated || ACTION_ERRORS.iter().any(|needle| message.contains(needle)))
        .map(|(policy_id, message)| format!("{policy_id}: {message}"))
        .collect();
    if errors.is_empty() {
        Ok(policies)
    } else {
        Err(Error::other(format!(
            "Cedar policy validation failed: {}",
            errors.join(", ")
        )))
    }
}

#[cfg(test)]
mod tests;
