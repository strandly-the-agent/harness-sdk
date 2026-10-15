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
//! - The agent client is built lazily like TS, via a [`tokio::sync::OnceCell`]; so is the
//!   S3 client the `S3` write path uses.
//! - [`MemoryStore::add`] returns `()`, so the document id a write mints is returned by the
//!   inherent [`BedrockKnowledgeBaseStore::add_document`] instead.
//! - `s3.prefix` is optional (TS requires it); `None` uploads to the bucket root.
//! - Metadata attribute values go through an owned `AttributeValue`, which serves both the inline
//!   `CUSTOM` attributes and the JSON S3 sidecar (TS reuses the SDK type for both).

use async_trait::async_trait;
use aws_sdk_bedrockagent::types as agent;
use aws_sdk_bedrockagentruntime::types as rt;
use aws_smithy_types::error::metadata::ProvideErrorMetadata;
use aws_smithy_types::error::operation::BuildError;
use tokio::sync::OnceCell;
use uuid::Uuid;

use crate::aws_document::{document_to_json, json_to_document};
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
    /// Key prefix for uploaded objects; a trailing slash is added when missing.
    pub prefix: Option<String>,
    /// Pre-built S3 client for uploads; one is built lazily when omitted.
    pub client: Option<aws_sdk_s3::Client>,
}

/// Document-level access control entry stamped on writes, for ACL-aware data sources.
#[derive(Clone, Debug)]
pub struct AccessControlEntry {
    /// Principal or group name.
    pub name: String,
    /// Principal type (e.g. `USER`, `GROUP`).
    pub r#type: String,
    /// Access level granted to the principal.
    pub access: AccessControlAccess,
}

/// Access level of an [`AccessControlEntry`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessControlAccess {
    /// Grant access.
    Allow,
    /// Deny access.
    Deny,
}

impl AccessControlAccess {
    /// The wire value Bedrock expects (`ALLOW` / `DENY`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "ALLOW",
            Self::Deny => "DENY",
        }
    }
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
            .value(json_to_document(&self.value)?)
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
    s3_client: OnceCell<aws_sdk_s3::Client>,
    kb_type: OnceCell<KnowledgeBaseType>,
}

impl BedrockKnowledgeBaseStore {
    /// Read-only store over `knowledge_base_id`, which also names the store.
    pub async fn new(knowledge_base_id: impl Into<String>) -> Result<Self> {
        let knowledge_base_id = knowledge_base_id.into();
        Self::with_config(BedrockKnowledgeBaseStoreConfig {
            name: knowledge_base_id.clone(),
            config: BedrockKnowledgeBaseConfig {
                knowledge_base_id,
                ..Default::default()
            },
            ..Default::default()
        })
        .await
    }

    /// Store over the knowledge base named by `config`, building any client it does not supply.
    pub async fn with_config(config: BedrockKnowledgeBaseStoreConfig) -> Result<Self> {
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
        let s3_client = OnceCell::new();
        if let Some(client) = connection.s3.as_ref().and_then(|s3| s3.client.clone()) {
            let _ = s3_client.set(client);
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
            s3_client,
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

    async fn s3_client(&self) -> &aws_sdk_s3::Client {
        self.s3_client
            .get_or_init(|| async {
                let sdk_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
                    .load()
                    .await;
                let mut builder = aws_sdk_s3::config::Builder::from(&sdk_config);
                if let Some(region) = &self.region {
                    builder = builder.region(aws_sdk_s3::config::Region::new(region.clone()));
                }
                aws_sdk_s3::Client::from_conf(builder.build())
            })
            .await
    }

    /// Ingests `content` with optional `metadata` and returns the id Bedrock now tracks for the
    /// document: a UUID for a `CUSTOM` data source, the uploaded object's `s3://` URI for `S3`.
    ///
    /// Only `CUSTOM` and `S3` data sources accept direct ingestion; other backends are read-only.
    pub async fn add_document(&self, content: &str, metadata: Option<&Metadata>) -> Result<String> {
        if !self.writable {
            return Err(Error::other(
                "BedrockKnowledgeBaseStore: store is not writable. Set writable: true in config to enable add().",
            ));
        }
        if content.trim().is_empty() {
            return Err(Error::other(
                "BedrockKnowledgeBaseStore: content must not be empty.",
            ));
        }
        self.validate_write_config()?;
        let data_source_id = self.data_source_id.clone().unwrap_or_default();

        // S3 ingests objects, so its document points at objects uploaded first; CUSTOM carries the
        // text inline. Either way the store mints the id and hands it back to the caller.
        let (document, document_id) = if self.data_source_type == Some(DataSourceType::S3) {
            let (content_uri, sidecar_uri) = self.upload_s3_objects(content, metadata).await?;
            (
                build_s3_document(&content_uri, sidecar_uri.as_deref())?,
                content_uri,
            )
        } else {
            let document_id = Uuid::now_v7().to_string();
            (
                self.build_custom_document(&document_id, content, metadata)?,
                document_id,
            )
        };

        self.agent_client()
            .await
            .ingest_knowledge_base_documents()
            .knowledge_base_id(&self.knowledge_base_id)
            .data_source_id(&data_source_id)
            .documents(document)
            .send()
            .await
            .map_err(|error| {
                tracing::error!(
                    store = %self.name,
                    knowledge_base_id = %self.knowledge_base_id,
                    data_source_id = %data_source_id,
                    error = %error,
                    "knowledge base document ingestion failed"
                );
                if self.is_missing_acl_error(&error) {
                    Error::other(
                        "BedrockKnowledgeBaseStore: ingestion was rejected because the data source has ACL \
                         awareness enabled but this store has no accessControlList configured. Set \
                         accessControlList in the store config to write to an ACL-enabled data source.",
                    )
                } else {
                    Error::other(format!(
                        "BedrockKnowledgeBaseStore: IngestKnowledgeBaseDocuments failed for {}: {error}",
                        self.knowledge_base_id
                    ))
                }
            })?;
        Ok(document_id)
    }

    fn is_missing_acl_error<E: ProvideErrorMetadata>(&self, error: &E) -> bool {
        self.access_control_list.is_none()
            && error.code() == Some("ValidationException")
            && error
                .message()
                .is_some_and(|message| message.to_lowercase().contains("accesscontrollist"))
    }

    /// Uploads the content object plus, when there is anything to attach, a
    /// `<key>.metadata.json` sidecar beside it; returns their `s3://` URIs. Uploads are not
    /// transactional: a later failure leaves the objects in the bucket un-ingested.
    async fn upload_s3_objects(
        &self,
        content: &str,
        metadata: Option<&Metadata>,
    ) -> Result<(String, Option<String>)> {
        let s3 = self.s3_config.as_ref().ok_or_else(|| {
            Error::other("BedrockKnowledgeBaseStore: dataSourceType 'S3' requires config.s3.")
        })?;
        let prefix = match s3.prefix.as_deref() {
            None | Some("") => String::new(),
            Some(prefix) if prefix.ends_with('/') => prefix.to_owned(),
            Some(prefix) => format!("{prefix}/"),
        };
        let key = format!("{prefix}{}.txt", Uuid::now_v7());
        let content_uri = self
            .put_object(&s3.bucket, &key, content.to_owned(), "text/plain; charset=utf-8")
            .await?;

        let attributes = self.resolve_attributes(metadata);
        let acl = self.sidecar_acl();
        if attributes.is_empty() && acl.is_empty() {
            return Ok((content_uri, None));
        }
        let mut body = serde_json::Map::new();
        body.insert(
            "metadataAttributes".to_owned(),
            JsonValue::Object(
                attributes
                    .iter()
                    .map(|(key, value)| {
                        (
                            key.clone(),
                            serde_json::json!({ "value": value.to_json(), "includeForEmbedding": false }),
                        )
                    })
                    .collect(),
            ),
        );
        if !acl.is_empty() {
            body.insert("accessControlList".to_owned(), JsonValue::Array(acl));
        }
        let sidecar_uri = self
            .put_object(
                &s3.bucket,
                &format!("{key}.metadata.json"),
                JsonValue::Object(body).to_string(),
                "application/json",
            )
            .await?;
        Ok((content_uri, Some(sidecar_uri)))
    }

    async fn put_object(&self, bucket: &str, key: &str, body: String, content_type: &str) -> Result<String> {
        self.s3_client()
            .await
            .put_object()
            .bucket(bucket)
            .key(key)
            .body(aws_sdk_s3::primitives::ByteStream::from(body.into_bytes()))
            .content_type(content_type)
            .send()
            .await
            .map_err(|error| {
                tracing::error!(
                    store = %self.name,
                    uri = %format!("s3://{bucket}/{key}"),
                    error = %error,
                    "S3 upload failed before ingestion"
                );
                Error::other(format!(
                    "BedrockKnowledgeBaseStore: S3 upload to s3://{bucket}/{key} failed: {error}"
                ))
            })?;
        Ok(format!("s3://{bucket}/{key}"))
    }

    /// Scope plus caller metadata as attribute pairs, dropping keys that collide with the scope key
    /// and values of unsupported types. Shared by the inline `CUSTOM` and S3 sidecar builders.
    fn resolve_attributes(&self, metadata: Option<&Metadata>) -> Vec<(String, AttributeValue)> {
        let mut attributes = Vec::new();
        if let Some(scope) = &self.scope {
            attributes.push((
                self.scope_metadata_key.clone(),
                AttributeValue::String(scope.clone()),
            ));
        }
        for (key, value) in metadata.into_iter().flatten() {
            if self.scope.is_some() && key == &self.scope_metadata_key {
                tracing::warn!(
                    store = %self.name,
                    key = %key,
                    "dropping metadata key that collides with scopeMetadataKey"
                );
                continue;
            }
            match AttributeValue::from_json(value) {
                Some(value) => attributes.push((key.clone(), value)),
                None => tracing::debug!(
                    store = %self.name,
                    key = %key,
                    "dropping metadata value of unsupported type"
                ),
            }
        }
        attributes
    }

    /// The sidecar's `accessControlList`, which uses capitalized keys unlike the inline shape.
    fn sidecar_acl(&self) -> Vec<JsonValue> {
        self.access_control_list
            .iter()
            .flatten()
            .map(|entry| {
                serde_json::json!({ "Name": entry.name, "Type": entry.r#type, "Access": entry.access.as_str() })
            })
            .collect()
    }

    fn build_custom_document(
        &self,
        document_id: &str,
        content: &str,
        metadata: Option<&Metadata>,
    ) -> Result<agent::KnowledgeBaseDocument> {
        let attributes = self.resolve_attributes(metadata);
        // An inline ACL rides on IN_LINE_ATTRIBUTE metadata, which Bedrock requires to carry at
        // least one attribute, so an ACL with no scope/metadata has no valid inline shape.
        if self.access_control_list.is_some() && attributes.is_empty() {
            return Err(Error::other(
                "BedrockKnowledgeBaseStore: an inline accessControlList requires at least one metadata \
                 attribute, but this CUSTOM write has none. Set a scope or pass metadata to add().",
            ));
        }

        let custom = agent::CustomContent::builder()
            .custom_document_identifier(
                agent::CustomDocumentIdentifier::builder()
                    .id(document_id)
                    .build()
                    .map_err(build_failed)?,
            )
            .source_type(agent::CustomSourceType::InLine)
            .inline_content(
                agent::InlineContent::builder()
                    .r#type(agent::InlineContentType::Text)
                    .text_content(
                        agent::TextContentDoc::builder()
                            .data(content)
                            .build()
                            .map_err(build_failed)?,
                    )
                    .build()
                    .map_err(build_failed)?,
            )
            .build()
            .map_err(build_failed)?;
        let mut document = agent::KnowledgeBaseDocument::builder().content(
            agent::DocumentContent::builder()
                .data_source_type(agent::ContentDataSourceType::Custom)
                .custom(custom)
                .build()
                .map_err(build_failed)?,
        );

        if !attributes.is_empty() {
            let mut document_metadata =
                agent::DocumentMetadata::builder().r#type(agent::MetadataSourceType::InLineAttribute);
            for (key, value) in &attributes {
                document_metadata = document_metadata.inline_attributes(
                    agent::MetadataAttribute::builder()
                        .key(key)
                        .value(value.to_sdk()?)
                        .build()
                        .map_err(build_failed)?,
                );
            }
            for entry in self.access_control_list.iter().flatten() {
                document_metadata = document_metadata.access_control_list(
                    agent::DocumentAccessControlEntry::builder()
                        .name(&entry.name)
                        .r#type(agent::AccessControlPrincipalType::from(entry.r#type.as_str()))
                        .access(agent::AccessControlAccess::from(entry.access.as_str()))
                        .build()
                        .map_err(build_failed)?,
                );
            }
            document = document.metadata(document_metadata.build().map_err(build_failed)?);
        }
        Ok(document.build())
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

    fn supports_add(&self) -> bool {
        self.writable
    }

    async fn add(&self, content: &str, metadata: Option<&Metadata>) -> Result<()> {
        self.add_document(content, metadata).await.map(|_| ())
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

/// A metadata attribute value in the subset Bedrock accepts (TS `toAttributeValue`), owned so one
/// resolved list serves both inline `CUSTOM` attributes and the JSON S3 sidecar.
#[derive(Clone, Debug, PartialEq)]
enum AttributeValue {
    String(String),
    Number(f64),
    Boolean(bool),
    StringList(Vec<String>),
}

impl AttributeValue {
    fn from_json(value: &JsonValue) -> Option<Self> {
        match value {
            JsonValue::String(value) => Some(Self::String(value.clone())),
            JsonValue::Number(value) => value.as_f64().map(Self::Number),
            JsonValue::Bool(value) => Some(Self::Boolean(*value)),
            JsonValue::Array(items) if !items.is_empty() => items
                .iter()
                .map(|item| item.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .map(Self::StringList),
            _ => None,
        }
    }

    fn to_sdk(&self) -> Result<agent::MetadataAttributeValue> {
        let builder = agent::MetadataAttributeValue::builder();
        let builder = match self {
            Self::String(value) => builder
                .r#type(agent::MetadataValueType::String)
                .string_value(value),
            Self::Number(value) => builder
                .r#type(agent::MetadataValueType::Number)
                .number_value(*value),
            Self::Boolean(value) => builder
                .r#type(agent::MetadataValueType::Boolean)
                .boolean_value(*value),
            Self::StringList(values) => builder
                .r#type(agent::MetadataValueType::StringList)
                .set_string_list_value(Some(values.clone())),
        };
        builder.build().map_err(build_failed)
    }

    /// The sidecar JSON for this value, mirroring the SDK attribute shape TS serializes.
    fn to_json(&self) -> JsonValue {
        match self {
            Self::String(value) => serde_json::json!({ "type": "STRING", "stringValue": value }),
            Self::Number(value) => serde_json::json!({ "type": "NUMBER", "numberValue": value }),
            Self::Boolean(value) => serde_json::json!({ "type": "BOOLEAN", "booleanValue": value }),
            Self::StringList(values) => {
                serde_json::json!({ "type": "STRING_LIST", "stringListValue": values })
            }
        }
    }
}

fn build_failed(error: BuildError) -> Error {
    Error::other(format!(
        "BedrockKnowledgeBaseStore: building the knowledge base document failed: {error}"
    ))
}

/// An `S3` document referencing the uploaded content object, with its sidecar as metadata.
fn build_s3_document(content_uri: &str, sidecar_uri: Option<&str>) -> Result<agent::KnowledgeBaseDocument> {
    let mut document = agent::KnowledgeBaseDocument::builder().content(
        agent::DocumentContent::builder()
            .data_source_type(agent::ContentDataSourceType::S3)
            .s3(agent::S3Content::builder()
                .s3_location(
                    agent::S3Location::builder()
                        .uri(content_uri)
                        .build()
                        .map_err(build_failed)?,
                )
                .build())
            .build()
            .map_err(build_failed)?,
    );
    if let Some(uri) = sidecar_uri {
        document = document.metadata(
            agent::DocumentMetadata::builder()
                .r#type(agent::MetadataSourceType::S3Location)
                .s3_location(
                    agent::CustomS3Location::builder()
                        .uri(uri)
                        .build()
                        .map_err(build_failed)?,
                )
                .build()
                .map_err(build_failed)?,
        );
    }
    Ok(document.build())
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use aws_sdk_bedrockagent::operation::get_knowledge_base::{
        GetKnowledgeBaseError, GetKnowledgeBaseOutput,
    };
    use aws_sdk_bedrockagent::operation::ingest_knowledge_base_documents::{
        IngestKnowledgeBaseDocumentsError, IngestKnowledgeBaseDocumentsInput,
        IngestKnowledgeBaseDocumentsOutput,
    };
    use aws_sdk_bedrockagent::types::error::ValidationException;
    use aws_sdk_bedrockagentruntime::operation::retrieve::{RetrieveError, RetrieveOutput};
    use aws_sdk_s3::operation::put_object::{PutObjectError, PutObjectOutput};
    use aws_smithy_mocks::{mock, mock_client, Rule};
    use aws_smithy_types::error::ErrorMetadata;
    use aws_smithy_types::{Document, Number};
    use parking_lot::Mutex;

    use super::*;

    /// The `retrievalConfiguration` each mocked request carried.
    type Seen = Arc<Mutex<Vec<rt::KnowledgeBaseRetrievalConfiguration>>>;

    fn seen() -> Seen {
        Arc::new(Mutex::new(Vec::new()))
    }

    fn capturing_rule(seen: &Seen, results: Vec<rt::KnowledgeBaseRetrievalResult>) -> Rule {
        let captured = Arc::clone(seen);
        mock!(aws_sdk_bedrockagentruntime::Client::retrieve)
            .match_requests(move |req| {
                if let Some(config) = req.retrieval_configuration() {
                    captured.lock().push(config.clone());
                }
                true
            })
            .then_output(move || {
                RetrieveOutput::builder()
                    .set_retrieval_results(Some(results.clone()))
                    .build()
                    .expect("retrieve output")
            })
    }

    fn result(text: &str, score: Option<f64>) -> rt::KnowledgeBaseRetrievalResult {
        let mut builder = rt::KnowledgeBaseRetrievalResult::builder()
            .content(rt::RetrievalResultContent::builder().text(text).build());
        if let Some(score) = score {
            builder = builder.score(score);
        }
        builder.build()
    }

    async fn store_with(rule: &Rule, config: BedrockKnowledgeBaseStoreConfig) -> BedrockKnowledgeBaseStore {
        let mut config = config;
        config.name = "kb".to_owned();
        config.config.knowledge_base_id = "kb-123".to_owned();
        config.config.runtime_client = Some(mock_client!(aws_sdk_bedrockagentruntime, [rule]));
        if config.config.knowledge_base_type.is_none() {
            config.config.knowledge_base_type = Some(KnowledgeBaseType::Vector);
        }
        BedrockKnowledgeBaseStore::with_config(config)
            .await
            .expect("builds")
    }

    fn vector_config(
        config: &rt::KnowledgeBaseRetrievalConfiguration,
    ) -> &rt::KnowledgeBaseVectorSearchConfiguration {
        config
            .vector_search_configuration()
            .expect("vector search configuration")
    }

    #[tokio::test]
    async fn maps_results_to_memory_entries() {
        let seen = seen();
        let mut metadata = HashMap::new();
        metadata.insert("namespace".to_owned(), Document::String("tenant-a".to_owned()));
        metadata.insert("count".to_owned(), Document::Number(Number::PosInt(3)));
        let result = rt::KnowledgeBaseRetrievalResult::builder()
            .content(rt::RetrievalResultContent::builder().text("remembered").build())
            .score(0.75)
            .location(
                rt::RetrievalResultLocation::builder()
                    .r#type(rt::RetrievalResultLocationType::S3)
                    .s3_location(
                        rt::RetrievalResultS3Location::builder()
                            .uri("s3://bucket/doc.txt")
                            .build(),
                    )
                    .build()
                    .expect("location"),
            )
            .set_metadata(Some(metadata))
            .build();
        let rule = capturing_rule(&seen, vec![result]);
        let store = store_with(&rule, BedrockKnowledgeBaseStoreConfig::default()).await;

        let entries = store
            .search("query", &SearchOptions::default())
            .await
            .expect("search");

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, "remembered");
        let metadata = entries[0].metadata.as_ref().expect("metadata");
        assert_eq!(metadata["namespace"], JsonValue::String("tenant-a".to_owned()));
        assert_eq!(metadata["count"], JsonValue::Number(3.into()));
        assert_eq!(metadata["_relevanceScore"], json_number(0.75));
        assert_eq!(
            metadata["_sourceLocation"]["uri"],
            JsonValue::String("s3://bucket/doc.txt".to_owned())
        );
        assert_eq!(
            metadata["_sourceLocation"]["type"],
            JsonValue::String("S3".to_owned())
        );
    }

    #[tokio::test]
    async fn defaults_the_result_limit() {
        let seen = seen();
        let rule = capturing_rule(&seen, vec![]);
        let store = store_with(&rule, BedrockKnowledgeBaseStoreConfig::default()).await;

        store
            .search("query", &SearchOptions::default())
            .await
            .expect("search");

        assert_eq!(vector_config(&seen.lock()[0]).number_of_results(), 10);
    }

    #[tokio::test]
    async fn per_search_limit_overrides_the_store_default() {
        let seen = seen();
        let rule = capturing_rule(&seen, vec![]);
        let store = store_with(
            &rule,
            BedrockKnowledgeBaseStoreConfig {
                max_search_results: Some(5),
                ..Default::default()
            },
        )
        .await;

        store
            .search(
                "query",
                &SearchOptions {
                    max_search_results: Some(2),
                },
            )
            .await
            .expect("search");

        assert_eq!(vector_config(&seen.lock()[0]).number_of_results(), 2);
    }

    #[tokio::test]
    async fn rejects_a_per_search_limit_below_one() {
        let seen = seen();
        let rule = capturing_rule(&seen, vec![]);
        let store = store_with(&rule, BedrockKnowledgeBaseStoreConfig::default()).await;

        let error = store
            .search(
                "query",
                &SearchOptions {
                    max_search_results: Some(0),
                },
            )
            .await
            .expect_err("rejects");
        assert!(
            error.to_string().contains("maxSearchResults must be at least 1"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn rejects_a_store_limit_below_one() {
        let error = BedrockKnowledgeBaseStore::with_config(BedrockKnowledgeBaseStoreConfig {
            name: "kb".to_owned(),
            config: BedrockKnowledgeBaseConfig {
                knowledge_base_id: "kb-123".to_owned(),
                runtime_client: Some(mock_client!(aws_sdk_bedrockagentruntime, [&no_op_rule()])),
                ..Default::default()
            },
            max_search_results: Some(0),
            ..Default::default()
        })
        .await
        .map(|_| ())
        .expect_err("rejects");
        assert!(
            error.to_string().contains("maxSearchResults must be at least 1"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn rejects_min_and_max_score_together() {
        let error = BedrockKnowledgeBaseStore::with_config(BedrockKnowledgeBaseStoreConfig {
            name: "kb".to_owned(),
            config: BedrockKnowledgeBaseConfig {
                knowledge_base_id: "kb-123".to_owned(),
                runtime_client: Some(mock_client!(aws_sdk_bedrockagentruntime, [&no_op_rule()])),
                ..Default::default()
            },
            min_score: Some(0.5),
            max_score: Some(0.9),
            ..Default::default()
        })
        .await
        .map(|_| ())
        .expect_err("rejects");
        assert!(error.to_string().contains("mutually exclusive"), "{error}");
    }

    #[tokio::test]
    async fn drops_results_below_min_score_and_keeps_unscored_ones() {
        let seen = seen();
        let rule = capturing_rule(
            &seen,
            vec![
                result("kept", Some(0.8)),
                result("dropped", Some(0.2)),
                result("unscored", None),
            ],
        );
        let store = store_with(
            &rule,
            BedrockKnowledgeBaseStoreConfig {
                min_score: Some(0.5),
                ..Default::default()
            },
        )
        .await;

        let entries = store
            .search("query", &SearchOptions::default())
            .await
            .expect("search");

        let contents: Vec<&str> = entries.iter().map(|entry| entry.content.as_str()).collect();
        assert_eq!(contents, vec!["kept", "unscored"]);
    }

    #[tokio::test]
    async fn drops_results_above_max_score() {
        let seen = seen();
        let rule = capturing_rule(&seen, vec![result("near", Some(0.2)), result("far", Some(0.9))]);
        let store = store_with(
            &rule,
            BedrockKnowledgeBaseStoreConfig {
                max_score: Some(0.5),
                ..Default::default()
            },
        )
        .await;

        let entries = store
            .search("query", &SearchOptions::default())
            .await
            .expect("search");

        let contents: Vec<&str> = entries.iter().map(|entry| entry.content.as_str()).collect();
        assert_eq!(contents, vec!["near"]);
    }

    #[tokio::test]
    async fn derives_a_scope_filter() {
        let seen = seen();
        let rule = capturing_rule(&seen, vec![]);
        let store = store_with(
            &rule,
            BedrockKnowledgeBaseStoreConfig {
                scope: Some("tenant-a".to_owned()),
                ..Default::default()
            },
        )
        .await;

        store
            .search("query", &SearchOptions::default())
            .await
            .expect("search");

        let filter = vector_config(&seen.lock()[0]).filter().cloned().expect("filter");
        let rt::RetrievalFilter::Equals(attribute) = filter else {
            panic!("expected an equals filter");
        };
        assert_eq!(attribute.key(), "namespace");
        assert_eq!(attribute.value(), &Document::String("tenant-a".to_owned()));
    }

    #[tokio::test]
    async fn reuses_one_connection_config_across_scopes() {
        let seen = seen();
        let captured = Arc::clone(&seen);
        let rule = mock!(aws_sdk_bedrockagentruntime::Client::retrieve)
            .match_requests(move |req| {
                if let Some(config) = req.retrieval_configuration() {
                    captured.lock().push(config.clone());
                }
                true
            })
            .sequence()
            .output(|| {
                RetrieveOutput::builder()
                    .set_retrieval_results(Some(vec![]))
                    .build()
                    .expect("retrieve output")
            })
            .repeatedly()
            .build();
        let connection = BedrockKnowledgeBaseConfig {
            knowledge_base_id: "kb-123".to_owned(),
            knowledge_base_type: Some(KnowledgeBaseType::Vector),
            runtime_client: Some(mock_client!(aws_sdk_bedrockagentruntime, [&rule])),
            ..Default::default()
        };
        let store_for = |name: &str, scope: &str| {
            BedrockKnowledgeBaseStore::with_config(BedrockKnowledgeBaseStoreConfig {
                name: name.to_owned(),
                config: connection.clone(),
                scope: Some(scope.to_owned()),
                ..Default::default()
            })
        };
        let tenant_a = store_for("kb-a", "tenant-a").await.expect("builds");
        let tenant_b = store_for("kb-b", "tenant-b").await.expect("builds");

        tenant_a
            .search("query", &SearchOptions::default())
            .await
            .expect("search");
        tenant_b
            .search("query", &SearchOptions::default())
            .await
            .expect("search");

        let requests = seen.lock();
        assert_eq!(requests.len(), 2, "one retrieve per store");
        let scopes: Vec<String> = requests
            .iter()
            .map(|request| {
                let filter = vector_config(request).filter().cloned().expect("filter");
                let rt::RetrievalFilter::Equals(attribute) = filter else {
                    panic!("expected an equals filter");
                };
                assert_eq!(attribute.key(), "namespace");
                match attribute.value() {
                    Document::String(value) => value.clone(),
                    other => panic!("expected a string scope, got {other:?}"),
                }
            })
            .collect();
        assert_eq!(scopes, vec!["tenant-a".to_owned(), "tenant-b".to_owned()]);
    }

    #[tokio::test]
    async fn an_explicit_filter_overrides_the_scope_filter() {
        let seen = seen();
        let rule = capturing_rule(&seen, vec![]);
        let store = store_with(
            &rule,
            BedrockKnowledgeBaseStoreConfig {
                scope: Some("tenant-a".to_owned()),
                filter: Some(RetrievalFilter::StringContains(FilterAttribute::new(
                    "topic",
                    JsonValue::String("rust".to_owned()),
                ))),
                ..Default::default()
            },
        )
        .await;

        store
            .search("query", &SearchOptions::default())
            .await
            .expect("search");

        let filter = vector_config(&seen.lock()[0]).filter().cloned().expect("filter");
        let rt::RetrievalFilter::StringContains(attribute) = filter else {
            panic!("expected a stringContains filter");
        };
        assert_eq!(attribute.key(), "topic");
    }

    #[tokio::test]
    async fn uses_managed_search_configuration_for_a_managed_knowledge_base() {
        let seen = seen();
        let rule = capturing_rule(&seen, vec![]);
        let store = store_with(
            &rule,
            BedrockKnowledgeBaseStoreConfig {
                config: BedrockKnowledgeBaseConfig {
                    knowledge_base_type: Some(KnowledgeBaseType::Managed),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .await;

        store
            .search("query", &SearchOptions::default())
            .await
            .expect("search");

        let config = &seen.lock()[0];
        assert!(config.vector_search_configuration().is_none());
        assert_eq!(
            config
                .managed_search_configuration()
                .expect("managed search configuration")
                .number_of_results(),
            Some(10)
        );
    }

    #[tokio::test]
    async fn wraps_retrieve_failures() {
        let rule = mock!(aws_sdk_bedrockagentruntime::Client::retrieve).then_error(|| {
            RetrieveError::generic(
                ErrorMetadata::builder()
                    .code("AccessDenied")
                    .message("denied")
                    .build(),
            )
        });
        let store = store_with(&rule, BedrockKnowledgeBaseStoreConfig::default()).await;

        let error = store
            .search("query", &SearchOptions::default())
            .await
            .expect_err("search fails");
        assert!(error.to_string().contains("Retrieve failed"), "{error}");
    }

    #[tokio::test]
    async fn rejects_a_writable_store_without_a_writable_data_source() {
        let error = BedrockKnowledgeBaseStore::with_config(BedrockKnowledgeBaseStoreConfig {
            name: "kb".to_owned(),
            config: BedrockKnowledgeBaseConfig {
                knowledge_base_id: "kb-123".to_owned(),
                data_source_type: Some(DataSourceType::Other),
                runtime_client: Some(mock_client!(aws_sdk_bedrockagentruntime, [&no_op_rule()])),
                ..Default::default()
            },
            writable: Some(true),
            ..Default::default()
        })
        .await
        .map(|_| ())
        .expect_err("rejects");
        assert!(error.to_string().contains("dataSourceType"), "{error}");
    }

    /// Inputs each mocked `IngestKnowledgeBaseDocuments` carried.
    type Ingested = Arc<Mutex<Vec<IngestKnowledgeBaseDocumentsInput>>>;
    /// `(bucket, key, content_type, body)` of each mocked `PutObject`.
    type Uploads = Arc<Mutex<Vec<(String, String, String, String)>>>;

    fn ingest_rule(ingested: &Ingested) -> Rule {
        let captured = Arc::clone(ingested);
        mock!(aws_sdk_bedrockagent::Client::ingest_knowledge_base_documents)
            .match_requests(move |req| {
                captured.lock().push(req.clone());
                true
            })
            .then_output(|| IngestKnowledgeBaseDocumentsOutput::builder().build())
    }

    fn validation_rule(message: &'static str) -> Rule {
        mock!(aws_sdk_bedrockagent::Client::ingest_knowledge_base_documents).then_error(move || {
            IngestKnowledgeBaseDocumentsError::ValidationException(
                ValidationException::builder()
                    .message(message)
                    .meta(
                        ErrorMetadata::builder()
                            .code("ValidationException")
                            .message(message)
                            .build(),
                    )
                    .build(),
            )
        })
    }

    fn put_object_rule(uploads: &Uploads) -> Rule {
        let captured = Arc::clone(uploads);
        mock!(aws_sdk_s3::Client::put_object)
            .match_requests(move |req| {
                captured.lock().push((
                    req.bucket().unwrap_or_default().to_owned(),
                    req.key().unwrap_or_default().to_owned(),
                    req.content_type().unwrap_or_default().to_owned(),
                    String::from_utf8_lossy(req.body().bytes().unwrap_or_default()).into_owned(),
                ));
                true
            })
            .sequence()
            .output(|| PutObjectOutput::builder().build())
            .repeatedly()
            .build()
    }

    async fn writable_store(
        ingest: &Rule,
        s3: Option<BedrockKnowledgeBaseS3Config>,
        config: BedrockKnowledgeBaseStoreConfig,
    ) -> BedrockKnowledgeBaseStore {
        let mut config = config;
        config.writable = Some(true);
        config.config.data_source_id = Some("ds-1".to_owned());
        config.config.data_source_type = Some(match s3 {
            Some(_) => DataSourceType::S3,
            None => DataSourceType::Custom,
        });
        config.config.s3 = s3;
        config.config.agent_client = Some(mock_client!(aws_sdk_bedrockagent, [ingest]));
        store_with(&no_op_rule(), config).await
    }

    fn s3_config(prefix: Option<&str>, put: &Rule) -> BedrockKnowledgeBaseS3Config {
        BedrockKnowledgeBaseS3Config {
            bucket: "memories".to_owned(),
            prefix: prefix.map(str::to_owned),
            client: Some(mock_client!(aws_sdk_s3, [put])),
        }
    }

    /// The single document of the single captured ingestion request.
    fn ingested_document(ingested: &Ingested) -> agent::KnowledgeBaseDocument {
        let requests = ingested.lock();
        assert_eq!(requests.len(), 1, "exactly one ingestion");
        let documents = requests[0].documents();
        assert_eq!(documents.len(), 1, "exactly one document");
        documents[0].clone()
    }

    #[tokio::test]
    async fn exposes_add_support_from_writable() {
        let ingested = Arc::new(Mutex::new(Vec::new()));
        let store = writable_store(&ingest_rule(&ingested), None, Default::default()).await;
        assert!(store.supports_add());

        let read_only = store_with(&no_op_rule(), BedrockKnowledgeBaseStoreConfig::default()).await;
        assert!(!read_only.supports_add());
    }

    #[tokio::test]
    async fn rejects_add_on_a_non_writable_store() {
        let store = store_with(&no_op_rule(), BedrockKnowledgeBaseStoreConfig::default()).await;
        let error = store.add("fact", None).await.expect_err("rejects");
        assert!(error.to_string().contains("not writable"), "{error}");
    }

    #[tokio::test]
    async fn rejects_blank_content() {
        let ingested = Arc::new(Mutex::new(Vec::new()));
        let store = writable_store(&ingest_rule(&ingested), None, Default::default()).await;
        let error = store.add_document("   ", None).await.expect_err("rejects");
        assert!(error.to_string().contains("must not be empty"), "{error}");
        assert!(ingested.lock().is_empty(), "no ingestion attempted");
    }

    #[tokio::test]
    async fn ingests_an_inline_custom_document_and_returns_its_id() {
        let ingested = Arc::new(Mutex::new(Vec::new()));
        let store = writable_store(&ingest_rule(&ingested), None, Default::default()).await;

        let document_id = store.add_document("a fact", None).await.expect("adds");

        let requests = ingested.lock();
        assert_eq!(requests[0].knowledge_base_id(), Some("kb-123"));
        assert_eq!(requests[0].data_source_id(), Some("ds-1"));
        drop(requests);
        let document = ingested_document(&ingested);
        let content = document.content().expect("content");
        assert_eq!(content.data_source_type(), &agent::ContentDataSourceType::Custom);
        let custom = content.custom().expect("custom content");
        assert_eq!(custom.source_type(), &agent::CustomSourceType::InLine);
        assert_eq!(
            custom.custom_document_identifier().expect("identifier").id(),
            document_id.as_str(),
            "the returned id is the document identifier"
        );
        let inline = custom.inline_content().expect("inline content");
        assert_eq!(inline.r#type(), &agent::InlineContentType::Text);
        assert_eq!(inline.text_content().expect("text").data(), "a fact");
        assert!(
            document.metadata().is_none(),
            "no metadata field without scope or metadata"
        );
        assert!(
            uuid::Uuid::parse_str(&document_id).is_ok(),
            "document id is a uuid: {document_id}"
        );
    }

    #[tokio::test]
    async fn attaches_the_scope_as_a_leading_inline_attribute() {
        let ingested = Arc::new(Mutex::new(Vec::new()));
        let store = writable_store(
            &ingest_rule(&ingested),
            None,
            BedrockKnowledgeBaseStoreConfig {
                scope: Some("user-1".to_owned()),
                ..Default::default()
            },
        )
        .await;

        store.add_document("a fact", None).await.expect("adds");

        let document = ingested_document(&ingested);
        let metadata = document.metadata().expect("metadata");
        assert_eq!(metadata.r#type(), &agent::MetadataSourceType::InLineAttribute);
        let attributes = metadata.inline_attributes();
        assert_eq!(attributes.len(), 1);
        assert_eq!(attributes[0].key(), "namespace");
        let value = attributes[0].value().expect("value");
        assert_eq!(value.r#type(), &agent::MetadataValueType::String);
        assert_eq!(value.string_value(), Some("user-1"));
    }

    #[tokio::test]
    async fn drops_metadata_keys_that_collide_with_the_scope_key() {
        let ingested = Arc::new(Mutex::new(Vec::new()));
        let store = writable_store(
            &ingest_rule(&ingested),
            None,
            BedrockKnowledgeBaseStoreConfig {
                scope: Some("user-1".to_owned()),
                ..Default::default()
            },
        )
        .await;
        let metadata = Metadata::from([("namespace".to_owned(), JsonValue::String("hijack".to_owned()))]);

        store.add_document("a fact", Some(&metadata)).await.expect("adds");

        let document = ingested_document(&ingested);
        let attributes = document.metadata().expect("metadata").inline_attributes();
        assert_eq!(attributes.len(), 1, "the colliding key is dropped");
        assert_eq!(
            attributes[0].value().expect("value").string_value(),
            Some("user-1")
        );
    }

    #[tokio::test]
    async fn maps_supported_metadata_types_and_skips_unsupported_ones() {
        let ingested = Arc::new(Mutex::new(Vec::new()));
        let store = writable_store(&ingest_rule(&ingested), None, Default::default()).await;
        let metadata = Metadata::from([
            ("text".to_owned(), JsonValue::String("value".to_owned())),
            ("count".to_owned(), json_number(7.0)),
            ("flag".to_owned(), JsonValue::Bool(true)),
            (
                "tags".to_owned(),
                JsonValue::Array(vec![JsonValue::String("a".to_owned())]),
            ),
            ("nested".to_owned(), JsonValue::Object(Default::default())),
            ("empty_list".to_owned(), JsonValue::Array(Vec::new())),
            ("nothing".to_owned(), JsonValue::Null),
        ]);

        store.add_document("a fact", Some(&metadata)).await.expect("adds");

        let document = ingested_document(&ingested);
        let attributes = document.metadata().expect("metadata").inline_attributes();
        let rendered: Vec<(&str, &agent::MetadataValueType)> = attributes
            .iter()
            .map(|attribute| (attribute.key(), attribute.value().expect("value").r#type()))
            .collect();
        assert_eq!(
            rendered,
            vec![
                ("count", &agent::MetadataValueType::Number),
                ("flag", &agent::MetadataValueType::Boolean),
                ("tags", &agent::MetadataValueType::StringList),
                ("text", &agent::MetadataValueType::String),
            ],
            "unsupported values are dropped"
        );
    }

    #[tokio::test]
    async fn attaches_the_access_control_list_in_inline_metadata() {
        let ingested = Arc::new(Mutex::new(Vec::new()));
        let store = writable_store(
            &ingest_rule(&ingested),
            None,
            BedrockKnowledgeBaseStoreConfig {
                scope: Some("user-1".to_owned()),
                access_control_list: Some(vec![AccessControlEntry {
                    name: "alice".to_owned(),
                    r#type: "USER".to_owned(),
                    access: AccessControlAccess::Allow,
                }]),
                ..Default::default()
            },
        )
        .await;

        store.add_document("a fact", None).await.expect("adds");

        let document = ingested_document(&ingested);
        let metadata = document.metadata().expect("metadata");
        assert_eq!(
            metadata.inline_attributes().len(),
            1,
            "the scope attribute rides along"
        );
        let acl = metadata.access_control_list();
        assert_eq!(acl.len(), 1);
        assert_eq!(acl[0].name(), "alice");
        assert_eq!(acl[0].r#type(), &agent::AccessControlPrincipalType::User);
        assert_eq!(acl[0].access(), &agent::AccessControlAccess::Allow);
    }

    #[tokio::test]
    async fn rejects_an_inline_acl_with_no_attributes() {
        let ingested = Arc::new(Mutex::new(Vec::new()));
        let store = writable_store(
            &ingest_rule(&ingested),
            None,
            BedrockKnowledgeBaseStoreConfig {
                access_control_list: Some(vec![AccessControlEntry {
                    name: "alice".to_owned(),
                    r#type: "USER".to_owned(),
                    access: AccessControlAccess::Allow,
                }]),
                ..Default::default()
            },
        )
        .await;

        let error = store.add_document("a fact", None).await.expect_err("rejects");
        assert!(error.to_string().contains("at least one metadata"), "{error}");
        assert!(ingested.lock().is_empty(), "no ingestion attempted");
    }

    #[tokio::test]
    async fn rewrites_a_missing_acl_validation_exception() {
        let store = writable_store(
            &validation_rule("accessControlList is required for this data source"),
            None,
            Default::default(),
        )
        .await;

        let error = store.add_document("a fact", None).await.expect_err("fails");
        assert!(error.to_string().contains("ACL awareness"), "{error}");
    }

    #[tokio::test]
    async fn does_not_rewrite_when_an_acl_is_configured() {
        let store = writable_store(
            &validation_rule("accessControlList is required for this data source"),
            None,
            BedrockKnowledgeBaseStoreConfig {
                scope: Some("user-1".to_owned()),
                access_control_list: Some(vec![AccessControlEntry {
                    name: "alice".to_owned(),
                    r#type: "USER".to_owned(),
                    access: AccessControlAccess::Allow,
                }]),
                ..Default::default()
            },
        )
        .await;

        let error = store.add_document("a fact", None).await.expect_err("fails");
        assert!(
            error.to_string().contains("IngestKnowledgeBaseDocuments failed"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn uploads_the_content_object_and_ingests_an_s3_document() {
        let ingested = Arc::new(Mutex::new(Vec::new()));
        let uploads = Arc::new(Mutex::new(Vec::new()));
        let store = writable_store(
            &ingest_rule(&ingested),
            Some(s3_config(Some("memories"), &put_object_rule(&uploads))),
            Default::default(),
        )
        .await;

        let document_id = store.add_document("a fact", None).await.expect("adds");

        let uploaded = uploads.lock().clone();
        assert_eq!(uploaded.len(), 1, "no sidecar without scope or metadata");
        let (bucket, key, content_type, body) = uploaded[0].clone();
        assert_eq!(bucket, "memories");
        assert!(key.starts_with("memories/") && key.ends_with(".txt"), "{key}");
        assert_eq!(content_type, "text/plain; charset=utf-8");
        assert_eq!(body, "a fact");
        assert_eq!(document_id, format!("s3://memories/{key}"));

        let document = ingested_document(&ingested);
        let content = document.content().expect("content");
        assert_eq!(content.data_source_type(), &agent::ContentDataSourceType::S3);
        assert_eq!(
            content
                .s3()
                .expect("s3 content")
                .s3_location()
                .expect("location")
                .uri(),
            document_id.as_str()
        );
        assert!(
            document.metadata().is_none(),
            "no sidecar means no document metadata"
        );
    }

    #[tokio::test]
    async fn does_not_double_the_prefix_slash() {
        let ingested = Arc::new(Mutex::new(Vec::new()));
        let uploads = Arc::new(Mutex::new(Vec::new()));
        let store = writable_store(
            &ingest_rule(&ingested),
            Some(s3_config(Some("memories/"), &put_object_rule(&uploads))),
            Default::default(),
        )
        .await;

        store.add_document("a fact", None).await.expect("adds");

        let key = uploads.lock()[0].1.clone();
        assert!(
            key.starts_with("memories/") && !key.starts_with("memories//"),
            "{key}"
        );
    }

    #[tokio::test]
    async fn writes_a_sidecar_carrying_scope_and_metadata_and_points_the_document_at_it() {
        let ingested = Arc::new(Mutex::new(Vec::new()));
        let uploads = Arc::new(Mutex::new(Vec::new()));
        let store = writable_store(
            &ingest_rule(&ingested),
            Some(s3_config(Some("memories"), &put_object_rule(&uploads))),
            BedrockKnowledgeBaseStoreConfig {
                scope: Some("user-1".to_owned()),
                ..Default::default()
            },
        )
        .await;
        let metadata = Metadata::from([("nested".to_owned(), JsonValue::Object(Default::default()))]);

        store.add_document("a fact", Some(&metadata)).await.expect("adds");

        let uploaded = uploads.lock().clone();
        assert_eq!(uploaded.len(), 2, "content object plus sidecar");
        let (_, content_key, _, _) = uploaded[0].clone();
        let (_, sidecar_key, sidecar_type, sidecar_body) = uploaded[1].clone();
        assert_eq!(sidecar_key, format!("{content_key}.metadata.json"));
        assert_eq!(sidecar_type, "application/json");
        assert_eq!(
            serde_json::from_str::<JsonValue>(&sidecar_body).expect("sidecar json"),
            serde_json::json!({
                "metadataAttributes": {
                    "namespace": {
                        "value": { "type": "STRING", "stringValue": "user-1" },
                        "includeForEmbedding": false,
                    }
                }
            }),
            "unsupported metadata values are omitted"
        );

        let document = ingested_document(&ingested);
        let document_metadata = document.metadata().expect("metadata");
        assert_eq!(document_metadata.r#type(), &agent::MetadataSourceType::S3Location);
        assert_eq!(
            document_metadata.s3_location().expect("sidecar location").uri(),
            format!("s3://memories/{sidecar_key}")
        );
    }

    #[tokio::test]
    async fn writes_an_acl_only_sidecar_with_capitalized_keys() {
        let ingested = Arc::new(Mutex::new(Vec::new()));
        let uploads = Arc::new(Mutex::new(Vec::new()));
        let store = writable_store(
            &ingest_rule(&ingested),
            Some(s3_config(None, &put_object_rule(&uploads))),
            BedrockKnowledgeBaseStoreConfig {
                access_control_list: Some(vec![AccessControlEntry {
                    name: "alice".to_owned(),
                    r#type: "USER".to_owned(),
                    access: AccessControlAccess::Deny,
                }]),
                ..Default::default()
            },
        )
        .await;

        store.add_document("a fact", None).await.expect("adds");

        let uploaded = uploads.lock().clone();
        assert_eq!(uploaded.len(), 2, "an ACL alone still warrants a sidecar");
        assert!(!uploaded[0].1.contains('/'), "no prefix means a bare key");
        assert_eq!(
            serde_json::from_str::<JsonValue>(&uploaded[1].3).expect("sidecar json"),
            serde_json::json!({
                "metadataAttributes": {},
                "accessControlList": [{ "Name": "alice", "Type": "USER", "Access": "DENY" }],
            })
        );
    }

    #[tokio::test]
    async fn wraps_an_s3_upload_failure_before_any_ingestion() {
        let ingested = Arc::new(Mutex::new(Vec::new()));
        let put = mock!(aws_sdk_s3::Client::put_object).then_error(|| {
            PutObjectError::generic(
                ErrorMetadata::builder()
                    .code("AccessDenied")
                    .message("denied")
                    .build(),
            )
        });
        let store = writable_store(
            &ingest_rule(&ingested),
            Some(s3_config(Some("memories"), &put)),
            Default::default(),
        )
        .await;

        let error = store.add_document("a fact", None).await.expect_err("fails");
        assert!(
            error.to_string().contains("S3 upload to s3://memories/"),
            "{error}"
        );
        assert!(ingested.lock().is_empty(), "nothing is ingested");
    }

    /// Counts the `GetKnowledgeBase` calls a mocked agent client served.
    type Hits = Arc<Mutex<usize>>;

    fn get_kb_rule(kb_type: Option<&'static str>, hits: &Hits) -> Rule {
        let counter = Arc::clone(hits);
        mock!(aws_sdk_bedrockagent::Client::get_knowledge_base)
            .match_requests(move |_| {
                *counter.lock() += 1;
                true
            })
            .sequence()
            .output(move || {
                let mut builder = agent::KnowledgeBase::builder()
                    .knowledge_base_id("kb-123")
                    .name("kb")
                    .knowledge_base_arn("arn:aws:bedrock:us-west-2:1:knowledge-base/kb-123")
                    .role_arn("arn:aws:iam::1:role/kb")
                    .status(agent::KnowledgeBaseStatus::Active)
                    .created_at(aws_smithy_types::DateTime::from_secs(0))
                    .updated_at(aws_smithy_types::DateTime::from_secs(0));
                if let Some(kb_type) = kb_type {
                    builder = builder.knowledge_base_configuration(
                        agent::KnowledgeBaseConfiguration::builder()
                            .r#type(agent::KnowledgeBaseType::from(kb_type))
                            .build()
                            .expect("knowledge base configuration"),
                    );
                }
                GetKnowledgeBaseOutput::builder()
                    .knowledge_base(builder.build().expect("knowledge base"))
                    .build()
            })
            .repeatedly()
            .build()
    }

    /// A store that must ask `GetKnowledgeBase` for its type, with `agent` serving the lookup.
    async fn detecting_store(retrieve: &Rule, agent: &Rule) -> BedrockKnowledgeBaseStore {
        BedrockKnowledgeBaseStore::with_config(BedrockKnowledgeBaseStoreConfig {
            name: "kb".to_owned(),
            config: BedrockKnowledgeBaseConfig {
                knowledge_base_id: "kb-123".to_owned(),
                runtime_client: Some(mock_client!(aws_sdk_bedrockagentruntime, [retrieve])),
                agent_client: Some(mock_client!(aws_sdk_bedrockagent, [agent])),
                ..Default::default()
            },
            ..Default::default()
        })
        .await
        .expect("builds")
    }

    #[tokio::test]
    async fn carries_through_the_config_surface() {
        let store = store_with(
            &no_op_rule(),
            BedrockKnowledgeBaseStoreConfig {
                description: Some("product docs".to_owned()),
                max_search_results: Some(7),
                scope: Some("user-abc".to_owned()),
                min_score: Some(0.4),
                ..Default::default()
            },
        )
        .await;

        assert_eq!(store.name(), "kb");
        assert_eq!(store.description(), Some("product docs"));
        assert_eq!(store.max_search_results(), Some(7));
        assert_eq!(store.scope(), Some("user-abc"));
        assert!(!store.writable());
        assert!(!store.supports_add());
        assert!(store.extraction().is_none());
        assert_eq!(store.scope_metadata_key(), DEFAULT_SCOPE_METADATA_KEY);
    }

    #[tokio::test]
    async fn exposes_a_configured_extraction_config() {
        let store = store_with(
            &no_op_rule(),
            BedrockKnowledgeBaseStoreConfig {
                extraction: Some(ExtractionConfig {
                    system_prompt: Some("extract facts".to_owned()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await;

        let extraction = store.extraction().expect("extraction");
        assert_eq!(extraction.system_prompt.as_deref(), Some("extract facts"));
    }

    #[tokio::test]
    async fn rejects_a_zero_min_score_paired_with_a_max_score() {
        let error = store_config_error(BedrockKnowledgeBaseStoreConfig {
            min_score: Some(0.0),
            max_score: Some(0.4),
            ..Default::default()
        })
        .await;
        assert!(
            error
                .to_string()
                .contains("minScore and maxScore are mutually exclusive"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn rejects_an_empty_knowledge_base_id() {
        let error = BedrockKnowledgeBaseStore::with_config(BedrockKnowledgeBaseStoreConfig {
            name: "kb".to_owned(),
            config: BedrockKnowledgeBaseConfig {
                runtime_client: Some(mock_client!(aws_sdk_bedrockagentruntime, [&no_op_rule()])),
                ..Default::default()
            },
            ..Default::default()
        })
        .await
        .map(|_| ())
        .expect_err("rejects");
        assert!(
            error.to_string().contains("knowledgeBaseId must not be empty"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn rejects_a_writable_store_without_a_data_source_id() {
        let error = store_config_error(BedrockKnowledgeBaseStoreConfig {
            writable: Some(true),
            config: BedrockKnowledgeBaseConfig {
                data_source_type: Some(DataSourceType::Custom),
                ..Default::default()
            },
            ..Default::default()
        })
        .await;
        assert!(
            error.to_string().contains("requires config.dataSourceId"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn rejects_a_writable_s3_store_without_an_s3_config() {
        let error = store_config_error(BedrockKnowledgeBaseStoreConfig {
            writable: Some(true),
            config: BedrockKnowledgeBaseConfig {
                data_source_type: Some(DataSourceType::S3),
                data_source_id: Some("ds-1".to_owned()),
                ..Default::default()
            },
            ..Default::default()
        })
        .await;
        assert!(error.to_string().contains("'S3' requires config.s3"), "{error}");
    }

    #[tokio::test]
    async fn detects_a_managed_knowledge_base_and_caches_the_lookup() {
        let hits: Hits = Arc::new(Mutex::new(0));
        let seen = seen();
        let store = detecting_store(
            &capturing_rule(&seen, vec![]),
            &get_kb_rule(Some("MANAGED"), &hits),
        )
        .await;

        store.initialize().await.expect("initializes");
        store.initialize().await.expect("initializes again");
        store
            .search("query", &SearchOptions::default())
            .await
            .expect("search");

        assert_eq!(*hits.lock(), 1, "the knowledge base type is looked up once");
        let config = &seen.lock()[0];
        assert!(config.vector_search_configuration().is_none());
        assert!(config.managed_search_configuration().is_some());
    }

    #[tokio::test]
    async fn treats_an_unrecognized_knowledge_base_type_as_vector() {
        let hits: Hits = Arc::new(Mutex::new(0));
        let seen = seen();
        let store = detecting_store(
            &capturing_rule(&seen, vec![]),
            &get_kb_rule(Some("SOMETHING_NEW"), &hits),
        )
        .await;

        store
            .search("query", &SearchOptions::default())
            .await
            .expect("search");

        assert!(seen.lock()[0].vector_search_configuration().is_some());
    }

    #[tokio::test]
    async fn rejects_a_knowledge_base_with_no_reported_type() {
        let hits: Hits = Arc::new(Mutex::new(0));
        let store = detecting_store(&no_op_rule(), &get_kb_rule(None, &hits)).await;

        let error = store.initialize().await.expect_err("fails");
        assert!(
            error.to_string().contains("returned no knowledge base type"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn wraps_get_knowledge_base_failures() {
        let rule = mock!(aws_sdk_bedrockagent::Client::get_knowledge_base).then_error(|| {
            GetKnowledgeBaseError::ResourceNotFoundException(
                agent::error::ResourceNotFoundException::builder()
                    .message("no such knowledge base")
                    .build(),
            )
        });
        let store = detecting_store(&no_op_rule(), &rule).await;

        let error = store.initialize().await.expect_err("fails");
        assert!(
            error.to_string().contains("GetKnowledgeBase failed for kb-123"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn uses_a_custom_scope_metadata_key_for_search_and_writes() {
        let seen = seen();
        let ingested = Arc::new(Mutex::new(Vec::new()));
        let config = BedrockKnowledgeBaseStoreConfig {
            scope: Some("user-1".to_owned()),
            config: BedrockKnowledgeBaseConfig {
                scope_metadata_key: Some("tenant".to_owned()),
                ..Default::default()
            },
            ..Default::default()
        };
        let search_store = store_with(&capturing_rule(&seen, vec![]), config.clone()).await;
        let write_store = writable_store(&ingest_rule(&ingested), None, config).await;

        search_store
            .search("query", &SearchOptions::default())
            .await
            .expect("search");
        write_store.add_document("a fact", None).await.expect("adds");

        assert_eq!(search_store.scope_metadata_key(), "tenant");
        let configs = seen.lock();
        let filter = vector_config(&configs[0]).filter().expect("filter");
        assert_eq!(filter.as_equals().expect("equals").key(), "tenant");
        let document = ingested_document(&ingested);
        let attributes = document.metadata().expect("metadata").inline_attributes();
        assert_eq!(attributes[0].key(), "tenant");
    }

    #[tokio::test]
    async fn applies_no_filter_when_the_store_has_no_scope() {
        let seen = seen();
        let store = store_with(
            &capturing_rule(&seen, vec![]),
            BedrockKnowledgeBaseStoreConfig::default(),
        )
        .await;

        store
            .search("query", &SearchOptions::default())
            .await
            .expect("search");

        assert!(vector_config(&seen.lock()[0]).filter().is_none());
    }

    #[tokio::test]
    async fn scopes_writes_even_when_an_explicit_search_filter_is_set() {
        let ingested = Arc::new(Mutex::new(Vec::new()));
        let store = writable_store(
            &ingest_rule(&ingested),
            None,
            BedrockKnowledgeBaseStoreConfig {
                scope: Some("user-1".to_owned()),
                filter: Some(RetrievalFilter::Equals(FilterAttribute::new(
                    "source",
                    JsonValue::String("manual".to_owned()),
                ))),
                ..Default::default()
            },
        )
        .await;

        store.add_document("a fact", None).await.expect("adds");

        let document = ingested_document(&ingested);
        let attributes = document.metadata().expect("metadata").inline_attributes();
        assert_eq!(attributes[0].key(), DEFAULT_SCOPE_METADATA_KEY);
        assert_eq!(
            attributes[0].value().expect("value").string_value(),
            Some("user-1")
        );
    }

    #[tokio::test]
    async fn wraps_a_generic_ingestion_failure() {
        let rule = mock!(aws_sdk_bedrockagent::Client::ingest_knowledge_base_documents).then_error(|| {
            IngestKnowledgeBaseDocumentsError::generic(
                ErrorMetadata::builder()
                    .code("ThrottlingException")
                    .message("slow down")
                    .build(),
            )
        });
        let store = writable_store(&rule, None, BedrockKnowledgeBaseStoreConfig::default()).await;

        let error = store.add_document("a fact", None).await.expect_err("fails");
        assert!(
            error
                .to_string()
                .contains("IngestKnowledgeBaseDocuments failed for kb-123"),
            "{error}"
        );
        assert!(!error.to_string().contains("accessControlList"), "{error}");
    }

    /// The error `new` rejects `config` with, after filling in a valid id and a mocked client.
    async fn store_config_error(config: BedrockKnowledgeBaseStoreConfig) -> Error {
        let mut config = config;
        config.name = "kb".to_owned();
        config.config.knowledge_base_id = "kb-123".to_owned();
        config.config.runtime_client = Some(mock_client!(aws_sdk_bedrockagentruntime, [&no_op_rule()]));
        BedrockKnowledgeBaseStore::with_config(config)
            .await
            .map(|_| ())
            .expect_err("rejects")
    }

    fn no_op_rule() -> Rule {
        mock!(aws_sdk_bedrockagentruntime::Client::retrieve)
            .then_output(|| RetrieveOutput::builder().build().expect("retrieve output"))
    }
}
