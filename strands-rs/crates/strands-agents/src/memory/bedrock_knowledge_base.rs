//! `BedrockKnowledgeBaseStore` (`strands-ts/src/vended-memory-stores/bedrock-knowledge-base/store.ts`):
//! a [`MemoryStore`] over an Amazon Bedrock knowledge base.
//!
//! Divergences from TS, all deliberate:
//! - The retrieval filter is an owned [`RetrievalFilter`] enum converted to the SDK type on the way
//!   out, so no `aws-sdk-bedrockagentruntime` type appears in the public API (crate convention).
//! - TS unwraps the JS SDK's `NumericValue` metadata wrapper (aws/aws-sdk-js-v3#8246); the Rust SDK
//!   hands back `aws_smithy_types::Document`, so numbers need no unwrapping.
//! - `_sourceLocation` carries the location *type* plus its URI/id rather than the raw SDK shape,
//!   which has no serde representation.
//! - The agent client is built lazily like TS, via a [`OnceCell`](tokio::sync::OnceCell).
//!
//! The write path (`add`: `IngestKnowledgeBaseDocuments` for `CUSTOM`, S3 `PutObject` + ingest for
//! `S3`) is not ported yet; the store is read-only until it is.

use std::collections::HashMap;

use async_trait::async_trait;
use aws_sdk_bedrockagentruntime::types as rt;
use aws_smithy_types::{Document, Number};
use tokio::sync::OnceCell;

use crate::error::{Error, Result};
use crate::memory::types::{ExtractionConfig, MemoryEntry, MemoryStore, Metadata, SearchOptions};
use crate::types::JsonValue;

/// Default result cap for this store (TS `DEFAULT_MAX_SEARCH_RESULTS`).
pub const DEFAULT_MAX_SEARCH_RESULTS: usize = 10;
/// Default metadata attribute key used for scope filtering.
pub const DEFAULT_SCOPE_METADATA_KEY: &str = "namespace";

/// Knowledge base type as reported by `GetKnowledgeBase`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KnowledgeBaseType {
    /// Kendra-backed.
    Kendra,
    /// Managed (uses `managedSearchConfiguration`).
    Managed,
    /// SQL/Redshift-backed.
    Sql,
    /// Vector store (uses `vectorSearchConfiguration`).
    Vector,
}

impl KnowledgeBaseType {
    fn from_sdk(value: &str) -> Self {
        match value {
            "KENDRA" => Self::Kendra,
            "MANAGED" => Self::Managed,
            "SQL" => Self::Sql,
            _ => Self::Vector,
        }
    }
}

/// Backend of the data source written to by `add`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataSourceType {
    /// Inline ingestion of text documents.
    Custom,
    /// S3 objects uploaded then ingested.
    S3,
    /// Any other backend; sync-based or query-only, hence read-only.
    Other,
}

/// S3 ingestion settings, required when [`DataSourceType::S3`] is used for writes.
#[derive(Clone, Debug, Default)]
pub struct BedrockKnowledgeBaseS3Config {
    /// Bucket `add` uploads to.
    pub bucket: String,
    /// Key prefix for uploaded objects.
    pub prefix: Option<String>,
}

/// Document-level access control entry stamped on writes, for ACL-aware data sources.
#[derive(Clone, Debug)]
pub struct AccessControlEntry {
    /// Principal or group name.
    pub name: String,
    /// Principal type (e.g. `USER`, `GROUP`).
    pub r#type: String,
    /// Access level (e.g. `ALLOW`).
    pub access: String,
}

/// An owned retrieval filter, converted to the SDK type when a search runs.
#[derive(Clone, Debug, PartialEq)]
pub enum RetrievalFilter {
    /// Attribute equals the value.
    Equals(FilterAttribute),
    /// Attribute does not equal the value.
    NotEquals(FilterAttribute),
    /// Attribute is greater than the value.
    GreaterThan(FilterAttribute),
    /// Attribute is greater than or equal to the value.
    GreaterThanOrEquals(FilterAttribute),
    /// Attribute is less than the value.
    LessThan(FilterAttribute),
    /// Attribute is less than or equal to the value.
    LessThanOrEquals(FilterAttribute),
    /// Attribute string contains the value.
    StringContains(FilterAttribute),
    /// Attribute starts with the value.
    StartsWith(FilterAttribute),
    /// Attribute is one of the values.
    In(FilterAttribute),
    /// Attribute is none of the values.
    NotIn(FilterAttribute),
    /// Attribute list contains the value.
    ListContains(FilterAttribute),
    /// Every nested filter matches.
    AndAll(Vec<RetrievalFilter>),
    /// Any nested filter matches.
    OrAll(Vec<RetrievalFilter>),
}

/// One `key`/`value` pair inside a [`RetrievalFilter`].
#[derive(Clone, Debug, PartialEq)]
pub struct FilterAttribute {
    /// Metadata attribute key.
    pub key: String,
    /// Value compared against.
    pub value: JsonValue,
}

impl FilterAttribute {
    /// Attribute `key` compared against `value`.
    pub fn new(key: impl Into<String>, value: impl Into<JsonValue>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }

    fn to_sdk(&self) -> Result<rt::FilterAttribute> {
        rt::FilterAttribute::builder()
            .key(self.key.clone())
            .value(json_to_document(&self.value))
            .build()
            .map_err(|error| Error::other(format!("BedrockKnowledgeBaseStore: invalid filter: {error}")))
    }
}

impl RetrievalFilter {
    fn to_sdk(&self) -> Result<rt::RetrievalFilter> {
        let nested = |filters: &Vec<RetrievalFilter>| {
            filters
                .iter()
                .map(RetrievalFilter::to_sdk)
                .collect::<Result<Vec<_>>>()
        };
        Ok(match self {
            Self::Equals(a) => rt::RetrievalFilter::Equals(a.to_sdk()?),
            Self::NotEquals(a) => rt::RetrievalFilter::NotEquals(a.to_sdk()?),
            Self::GreaterThan(a) => rt::RetrievalFilter::GreaterThan(a.to_sdk()?),
            Self::GreaterThanOrEquals(a) => rt::RetrievalFilter::GreaterThanOrEquals(a.to_sdk()?),
            Self::LessThan(a) => rt::RetrievalFilter::LessThan(a.to_sdk()?),
            Self::LessThanOrEquals(a) => rt::RetrievalFilter::LessThanOrEquals(a.to_sdk()?),
            Self::StringContains(a) => rt::RetrievalFilter::StringContains(a.to_sdk()?),
            Self::StartsWith(a) => rt::RetrievalFilter::StartsWith(a.to_sdk()?),
            Self::In(a) => rt::RetrievalFilter::In(a.to_sdk()?),
            Self::NotIn(a) => rt::RetrievalFilter::NotIn(a.to_sdk()?),
            Self::ListContains(a) => rt::RetrievalFilter::ListContains(a.to_sdk()?),
            Self::AndAll(filters) => rt::RetrievalFilter::AndAll(nested(filters)?),
            Self::OrAll(filters) => rt::RetrievalFilter::OrAll(nested(filters)?),
        })
    }
}

/// Connection to a knowledge base, reusable across stores that differ only by `scope`.
#[derive(Clone, Debug, Default)]
pub struct BedrockKnowledgeBaseConfig {
    /// Knowledge base id.
    pub knowledge_base_id: String,
    /// Data source ingested into by writes.
    pub data_source_id: Option<String>,
    /// Backend of that data source; omitted means read-only.
    pub data_source_type: Option<DataSourceType>,
    /// S3 ingestion settings, required when `data_source_type` is [`DataSourceType::S3`].
    pub s3: Option<BedrockKnowledgeBaseS3Config>,
    /// Metadata attribute key used for scope filtering; defaults to `namespace`.
    pub scope_metadata_key: Option<String>,
    /// Knowledge base type; when set, skips the `GetKnowledgeBase` call during initialization.
    pub knowledge_base_type: Option<KnowledgeBaseType>,
    /// Region override for clients this store builds itself.
    pub region: Option<String>,
    /// Pre-built runtime client for `Retrieve`; one is built on demand when omitted.
    pub runtime_client: Option<aws_sdk_bedrockagentruntime::Client>,
    /// Pre-built agent client for `GetKnowledgeBase` / ingestion; built lazily when omitted.
    pub agent_client: Option<aws_sdk_bedrockagent::Client>,
}

/// Config for [`BedrockKnowledgeBaseStore`].
#[derive(Clone, Debug, Default)]
pub struct BedrockKnowledgeBaseStoreConfig {
    /// Unique store name.
    pub name: String,
    /// Description surfaced to the model.
    pub description: Option<String>,
    /// Connection to the knowledge base.
    pub config: BedrockKnowledgeBaseConfig,
    /// Logical namespace: a metadata filter on search, stamped on writes.
    pub scope: Option<String>,
    /// Explicit retrieval filter; overrides the scope-derived filter on search.
    pub filter: Option<RetrievalFilter>,
    /// Default result cap.
    pub max_search_results: Option<usize>,
    /// Keep only results scoring at least this much (similarity-scored KBs).
    pub min_score: Option<f64>,
    /// Keep only results scoring at most this much (distance-scored KBs).
    pub max_score: Option<f64>,
    /// Accept writes (default `false`).
    pub writable: Option<bool>,
    /// Access control entries stamped on every write.
    pub access_control_list: Option<Vec<AccessControlEntry>>,
    /// Automatic-extraction config.
    pub extraction: Option<ExtractionConfig>,
}

/// Memory store backed by an Amazon Bedrock knowledge base.
pub struct BedrockKnowledgeBaseStore {
    name: String,
    description: Option<String>,
    max_search_results: Option<usize>,
    min_score: Option<f64>,
    max_score: Option<f64>,
    writable: bool,
    extraction: Option<ExtractionConfig>,
    scope: Option<String>,
    scope_metadata_key: String,
    filter: Option<RetrievalFilter>,
    access_control_list: Option<Vec<AccessControlEntry>>,
    knowledge_base_id: String,
    data_source_id: Option<String>,
    data_source_type: Option<DataSourceType>,
    s3_config: Option<BedrockKnowledgeBaseS3Config>,
    region: Option<String>,
    runtime_client: aws_sdk_bedrockagentruntime::Client,
    agent_client: OnceCell<aws_sdk_bedrockagent::Client>,
    kb_type: OnceCell<KnowledgeBaseType>,
}

impl BedrockKnowledgeBaseStore {
    /// Store over the knowledge base named by `config`, building any client it does not supply.
    pub async fn new(config: BedrockKnowledgeBaseStoreConfig) -> Result<Self> {
        let connection = config.config;
        if connection.knowledge_base_id.is_empty() {
            return Err(Error::other(
                "BedrockKnowledgeBaseStore: config.knowledgeBaseId must not be empty.",
            ));
        }
        if let Some(max) = config.max_search_results {
            if max < 1 {
                return Err(Error::other(
                    "BedrockKnowledgeBaseStore: maxSearchResults must be at least 1.",
                ));
            }
        }
        if let (Some(min), Some(max)) = (config.min_score, config.max_score) {
            return Err(Error::other(format!(
                "BedrockKnowledgeBaseStore: minScore and maxScore are mutually exclusive, but both are set (minScore={min}, maxScore={max})."
            )));
        }

        let runtime_client = match connection.runtime_client {
            Some(client) => client,
            None => {
                let sdk_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
                    .load()
                    .await;
                let mut builder = aws_sdk_bedrockagentruntime::config::Builder::from(&sdk_config);
                if let Some(region) = &connection.region {
                    builder =
                        builder.region(aws_sdk_bedrockagentruntime::config::Region::new(region.clone()));
                }
                aws_sdk_bedrockagentruntime::Client::from_conf(builder.build())
            }
        };

        let agent_client = OnceCell::new();
        if let Some(client) = connection.agent_client {
            let _ = agent_client.set(client);
        }
        let kb_type = OnceCell::new();
        if let Some(value) = connection.knowledge_base_type {
            let _ = kb_type.set(value);
        }

        let writable = config.writable.unwrap_or(false);
        let store = Self {
            name: config.name,
            description: config.description,
            max_search_results: config.max_search_results,
            min_score: config.min_score,
            max_score: config.max_score,
            writable,
            extraction: config.extraction,
            scope: config.scope,
            scope_metadata_key: connection
                .scope_metadata_key
                .unwrap_or_else(|| DEFAULT_SCOPE_METADATA_KEY.to_owned()),
            filter: config.filter,
            access_control_list: config.access_control_list,
            knowledge_base_id: connection.knowledge_base_id,
            data_source_id: connection.data_source_id,
            data_source_type: connection.data_source_type,
            s3_config: connection.s3,
            region: connection.region,
            runtime_client,
            agent_client,
            kb_type,
        };
        if writable {
            store.validate_write_config()?;
        }
        Ok(store)
    }

    /// The logical namespace, when set.
    pub fn scope(&self) -> Option<&str> {
        self.scope.as_deref()
    }

    /// The metadata attribute key used for scope filtering.
    pub fn scope_metadata_key(&self) -> &str {
        &self.scope_metadata_key
    }

    /// The explicit retrieval filter, when set.
    pub fn filter(&self) -> Option<&RetrievalFilter> {
        self.filter.as_ref()
    }

    /// Access control entries stamped on writes, when set.
    pub fn access_control_list(&self) -> Option<&[AccessControlEntry]> {
        self.access_control_list.as_deref()
    }

    fn validate_write_config(&self) -> Result<()> {
        match self.data_source_type {
            None | Some(DataSourceType::Other) => Err(Error::other(
                "BedrockKnowledgeBaseStore: writable requires dataSourceType 'CUSTOM' or 'S3'.",
            )),
            Some(_) if self.data_source_id.is_none() => Err(Error::other(
                "BedrockKnowledgeBaseStore: writable requires config.dataSourceId.",
            )),
            Some(DataSourceType::S3) if self.s3_config.is_none() => Err(Error::other(
                "BedrockKnowledgeBaseStore: dataSourceType 'S3' requires config.s3.",
            )),
            Some(_) => Ok(()),
        }
    }

    fn resolve_filter(&self) -> Option<RetrievalFilter> {
        if let Some(filter) = &self.filter {
            return Some(filter.clone());
        }
        self.scope.as_ref().map(|scope| {
            RetrievalFilter::Equals(FilterAttribute::new(
                self.scope_metadata_key.clone(),
                JsonValue::String(scope.clone()),
            ))
        })
    }

    async fn agent_client(&self) -> &aws_sdk_bedrockagent::Client {
        self.agent_client
            .get_or_init(|| async {
                let sdk_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
                    .load()
                    .await;
                let mut builder = aws_sdk_bedrockagent::config::Builder::from(&sdk_config);
                if let Some(region) = &self.region {
                    builder = builder.region(aws_sdk_bedrockagent::config::Region::new(region.clone()));
                }
                aws_sdk_bedrockagent::Client::from_conf(builder.build())
            })
            .await
    }

    async fn resolve_kb_type(&self) -> Result<KnowledgeBaseType> {
        if let Some(value) = self.kb_type.get() {
            return Ok(*value);
        }
        let response = self
            .agent_client()
            .await
            .get_knowledge_base()
            .knowledge_base_id(&self.knowledge_base_id)
            .send()
            .await
            .map_err(|error| {
                tracing::error!(
                    store = %self.name,
                    knowledge_base_id = %self.knowledge_base_id,
                    error = %error,
                    "knowledge base type detection failed"
                );
                Error::other(format!(
                    "BedrockKnowledgeBaseStore: GetKnowledgeBase failed for {}: {error}",
                    self.knowledge_base_id
                ))
            })?;
        let kb_type = response
            .knowledge_base()
            .and_then(|kb| kb.knowledge_base_configuration())
            .map(|config| KnowledgeBaseType::from_sdk(config.r#type().as_str()))
            .ok_or_else(|| {
                Error::other(format!(
                    "BedrockKnowledgeBaseStore: GetKnowledgeBase returned no knowledge base type for {}.",
                    self.knowledge_base_id
                ))
            })?;
        let _ = self.kb_type.set(kb_type);
        Ok(kb_type)
    }
}

#[async_trait]
impl MemoryStore for BedrockKnowledgeBaseStore {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    fn max_search_results(&self) -> Option<usize> {
        self.max_search_results
    }

    fn writable(&self) -> bool {
        self.writable
    }

    fn extraction(&self) -> Option<&ExtractionConfig> {
        self.extraction.as_ref()
    }

    async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<MemoryEntry>> {
        if options.max_search_results == Some(0) {
            return Err(Error::other(
                "BedrockKnowledgeBaseStore: maxSearchResults must be at least 1.",
            ));
        }
        let limit = options
            .max_search_results
            .or(self.max_search_results)
            .unwrap_or(DEFAULT_MAX_SEARCH_RESULTS);

        let number_of_results = i32::try_from(limit).unwrap_or(i32::MAX);
        let filter = self.resolve_filter().map(|filter| filter.to_sdk()).transpose()?;

        // A managed knowledge base takes `managedSearchConfiguration`, a vector one
        // `vectorSearchConfiguration`; both carry the same fields here.
        let retrieval_config = if self.resolve_kb_type().await? == KnowledgeBaseType::Managed {
            rt::KnowledgeBaseRetrievalConfiguration::builder().managed_search_configuration(
                rt::ManagedSearchConfiguration::builder()
                    .number_of_results(number_of_results)
                    .set_filter(filter)
                    .build(),
            )
        } else {
            rt::KnowledgeBaseRetrievalConfiguration::builder().vector_search_configuration(
                rt::KnowledgeBaseVectorSearchConfiguration::builder()
                    .number_of_results(number_of_results)
                    .set_filter(filter)
                    .build(),
            )
        };

        let response = self
            .runtime_client
            .retrieve()
            .knowledge_base_id(&self.knowledge_base_id)
            .retrieval_query(rt::KnowledgeBaseQuery::builder().text(query).build())
            .retrieval_configuration(retrieval_config.build())
            .send()
            .await
            .map_err(|error| {
                tracing::error!(
                    store = %self.name,
                    knowledge_base_id = %self.knowledge_base_id,
                    error = %error,
                    "knowledge base retrieve failed"
                );
                Error::other(format!("BedrockKnowledgeBaseStore: Retrieve failed: {error}"))
            })?;

        let results = response.retrieval_results();
        let entries: Vec<MemoryEntry> = results
            .iter()
            .filter(|result| passes_score_bound(result.score(), self.min_score, self.max_score))
            .map(|result| {
                let mut metadata: Metadata = result
                    .metadata()
                    .map(|map| {
                        map.iter()
                            .map(|(key, value)| (key.clone(), document_to_json(value)))
                            .collect()
                    })
                    .unwrap_or_default();
                if let Some(location) = result.location() {
                    metadata.insert("_sourceLocation".to_owned(), location_to_json(location));
                }
                if let Some(score) = result.score() {
                    metadata.insert("_relevanceScore".to_owned(), json_number(score));
                }
                MemoryEntry {
                    content: result
                        .content()
                        .map(|content| content.text())
                        .unwrap_or_default()
                        .to_owned(),
                    store_name: None,
                    metadata: Some(metadata),
                }
            })
            .collect();

        if self.min_score.is_some() || self.max_score.is_some() {
            tracing::debug!(
                store = %self.name,
                min_score = ?self.min_score,
                max_score = ?self.max_score,
                retrieved = results.len(),
                kept = entries.len(),
                "score bound applied"
            );
        }
        Ok(entries)
    }

    async fn initialize(&self) -> Result<()> {
        self.resolve_kb_type().await.map(|_| ())
    }
}

/// Whether a retrieval score sits inside the configured bound.
///
/// Unset bounds keep everything, and so does an unscored result: there is nothing to compare it
/// against, and a bound should not silently discard entries whose relevance is unknown.
fn passes_score_bound(score: Option<f64>, min_score: Option<f64>, max_score: Option<f64>) -> bool {
    match score {
        None => true,
        Some(score) => match (min_score, max_score) {
            (Some(min), _) => score >= min,
            (None, Some(max)) => score <= max,
            (None, None) => true,
        },
    }
}

fn json_number(value: f64) -> JsonValue {
    serde_json::Number::from_f64(value)
        .map(JsonValue::Number)
        .unwrap_or(JsonValue::Null)
}

fn location_to_json(location: &rt::RetrievalResultLocation) -> JsonValue {
    let mut map = serde_json::Map::new();
    map.insert(
        "type".to_owned(),
        JsonValue::String(location.r#type().as_str().to_owned()),
    );
    let uri = location
        .s3_location()
        .and_then(|l| l.uri())
        .or_else(|| location.web_location().and_then(|l| l.url()))
        .or_else(|| location.confluence_location().and_then(|l| l.url()))
        .or_else(|| location.salesforce_location().and_then(|l| l.url()))
        .or_else(|| location.share_point_location().and_then(|l| l.url()))
        .or_else(|| location.custom_document_location().and_then(|l| l.id()));
    if let Some(uri) = uri {
        map.insert("uri".to_owned(), JsonValue::String(uri.to_owned()));
    }
    JsonValue::Object(map)
}

fn document_to_json(document: &Document) -> JsonValue {
    match document {
        Document::Null => JsonValue::Null,
        Document::Bool(value) => JsonValue::Bool(*value),
        Document::String(value) => JsonValue::String(value.clone()),
        Document::Number(Number::PosInt(value)) => JsonValue::Number((*value).into()),
        Document::Number(Number::NegInt(value)) => JsonValue::Number((*value).into()),
        Document::Number(Number::Float(value)) => json_number(*value),
        Document::Array(values) => JsonValue::Array(values.iter().map(document_to_json).collect()),
        Document::Object(values) => JsonValue::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), document_to_json(value)))
                .collect(),
        ),
    }
}

fn json_to_document(value: &JsonValue) -> Document {
    match value {
        JsonValue::Null => Document::Null,
        JsonValue::Bool(value) => Document::Bool(*value),
        JsonValue::String(value) => Document::String(value.clone()),
        JsonValue::Number(value) => {
            if let Some(value) = value.as_u64() {
                Document::Number(Number::PosInt(value))
            } else if let Some(value) = value.as_i64() {
                Document::Number(Number::NegInt(value))
            } else {
                Document::Number(Number::Float(value.as_f64().unwrap_or_default()))
            }
        }
        JsonValue::Array(values) => Document::Array(values.iter().map(json_to_document).collect()),
        JsonValue::Object(values) => Document::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), json_to_document(value)))
                .collect::<HashMap<_, _>>(),
        ),
    }
}
