//! Amazon S3 snapshot storage (`strands-ts/src/session/s3-storage.ts`).
//!
//! Object key layout, identical to the TS and Python SDKs:
//!
//! ```text
//! [<prefix>/]<session_id>/scopes/<scope>/<scope_id>/snapshots/
//!   manifest.json
//!   snapshot_latest.json
//!   immutable_history/
//!     snapshot_<uuid7>.json
//! ```
//!
//! Divergences from TS: the type is `S3SessionStorage` (TS calls it `S3Storage`) so it does not
//! clash with the unified [`crate::S3Storage`]; the client is built by
//! [`S3SessionStorage::builder`] because resolving the AWS config is async; and `scope` is the
//! [`Scope`] enum, so there is no scope string to validate when building a key.

use async_trait::async_trait;
use aws_sdk_s3::config::http::HttpResponse;
use aws_sdk_s3::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_s3::operation::get_object::GetObjectError;
use aws_sdk_s3::types::{Delete, ObjectIdentifier};
use aws_sdk_s3::Client;

use super::storage::{SnapshotLocation, SnapshotStorage};
use super::types::{now_iso8601, Snapshot, SnapshotManifest, SNAPSHOT_SCHEMA_VERSION};
use super::validation::{validate_identifier, validate_uuid_v7};
use crate::error::{BoxError, Error, Result};
use crate::storage::s3_storage::resolve_prefix;

const MANIFEST: &str = "manifest.json";
const SNAPSHOT_LATEST: &str = "snapshot_latest.json";
const IMMUTABLE_HISTORY: &str = "immutable_history/";
/// Objects requested per `ListObjectsV2` page.
const S3_PAGE_SIZE: i32 = 1000;

/// S3-backed [`SnapshotStorage`]: persists snapshots as JSON objects in a bucket.
pub struct S3SessionStorage {
    bucket: String,
    prefix: String,
    client: Client,
}

/// Builder for [`S3SessionStorage`]: prefix, region or pre-configured client.
#[derive(Default)]
pub struct S3SessionStorageBuilder {
    bucket: String,
    prefix: Option<String>,
    region: Option<String>,
    client: Option<Client>,
}

impl S3SessionStorage {
    /// Storage in `bucket` with no prefix, resolving region and credentials from the AWS chain.
    pub async fn new(bucket: impl Into<String>) -> Result<Self> {
        Self::builder(bucket).build().await
    }

    /// Builder for prefix / region / client overrides.
    pub fn builder(bucket: impl Into<String>) -> S3SessionStorageBuilder {
        S3SessionStorageBuilder {
            bucket: bucket.into(),
            ..Default::default()
        }
    }

    /// The target bucket.
    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// The resolved key prefix, including its trailing `/` (empty when unset).
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    fn get_key(&self, location: &SnapshotLocation, path: &str) -> Result<String> {
        validate_identifier(&location.session_id)?;
        validate_identifier(&location.scope_id)?;
        Ok(format!(
            "{}{}/scopes/{}/{}/snapshots/{path}",
            self.prefix,
            location.session_id,
            location.scope.as_str(),
            location.scope_id,
        ))
    }

    fn session_prefix(&self, session_id: &str) -> Result<String> {
        validate_identifier(session_id)?;
        Ok(format!("{}{session_id}/", self.prefix))
    }

    fn latest_snapshot_key(&self, location: &SnapshotLocation) -> Result<String> {
        self.get_key(location, SNAPSHOT_LATEST)
    }

    fn history_snapshot_key(&self, location: &SnapshotLocation, snapshot_id: &str) -> Result<String> {
        validate_identifier(snapshot_id)?;
        self.get_key(
            location,
            &format!("{IMMUTABLE_HISTORY}snapshot_{snapshot_id}.json"),
        )
    }

    async fn write_json<T: serde::Serialize>(&self, key: &str, data: &T) -> Result<()> {
        let body = serde_json::to_vec_pretty(data)
            .map_err(|err| session_error(format!("Failed to write S3 object {key}"), err))?;
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(body.into())
            .content_type("application/json")
            .send()
            .await
            .map_err(|err| session_error(format!("Failed to write S3 object {key}"), err))?;
        Ok(())
    }

    async fn read_json<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let output = match self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
        {
            Ok(output) => output,
            Err(err) if is_missing_object(&err) => return Ok(None),
            Err(err) => return Err(session_error(format!("S3 error reading {key}"), err)),
        };
        let bytes = output
            .body
            .collect()
            .await
            .map_err(|err| session_error(format!("S3 error reading {key}"), err))?
            .into_bytes();
        if bytes.is_empty() {
            return Ok(None);
        }
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|err| session_error(format!("Invalid JSON in S3 object {key}"), err))
    }
}

impl S3SessionStorageBuilder {
    /// Key prefix prepended to every object key (a namespace within the bucket).
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    /// Region override; cannot be combined with [`S3SessionStorageBuilder::client`].
    pub fn region(mut self, region: impl Into<String>) -> Self {
        self.region = Some(region.into());
        self
    }

    /// Pre-configured S3 client; cannot be combined with [`S3SessionStorageBuilder::region`].
    pub fn client(mut self, client: Client) -> Self {
        self.client = Some(client);
        self
    }

    /// Resolve the AWS config (unless a client was supplied) and build the storage.
    pub async fn build(self) -> Result<S3SessionStorage> {
        if self.client.is_some() && self.region.is_some() {
            return Err(Error::Session {
                message:
                    "Cannot specify both client and region. Configure the region on the S3 client instead."
                        .to_owned(),
                source: None,
            });
        }
        let prefix = resolve_prefix(self.prefix.as_deref())?;
        let client = match self.client {
            Some(client) => client,
            None => crate::storage::s3_storage::s3_client(self.region.as_deref()).await,
        };
        Ok(S3SessionStorage {
            bucket: self.bucket,
            prefix,
            client,
        })
    }
}

fn session_error(message: String, source: impl Into<BoxError>) -> Error {
    Error::Session {
        message,
        source: Some(source.into()),
    }
}

/// `true` when a `GetObject` failure means the object (or its bucket) is absent.
fn is_missing_object(error: &SdkError<GetObjectError, HttpResponse>) -> bool {
    is_not_found(error) || matches!(error.as_service_error(), Some(GetObjectError::NoSuchKey(_)))
}

/// `true` for the error codes S3 uses for a missing object or bucket.
fn is_not_found(error: &(impl ProvideErrorMetadata + ?Sized)) -> bool {
    matches!(error.code(), Some("NoSuchKey" | "NotFound" | "NoSuchBucket"))
}

/// `snapshot_([\w-]+)\.json$` applied to an object key.
fn snapshot_id_from_key(key: &str) -> Option<&str> {
    let rest = key.strip_suffix(".json")?;
    rest.match_indices("snapshot_")
        .map(|(i, m)| &rest[i + m.len()..])
        .find(|id| {
            !id.is_empty()
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
}

#[async_trait]
impl SnapshotStorage for S3SessionStorage {
    async fn save_snapshot(
        &self,
        location: &SnapshotLocation,
        snapshot_id: &str,
        is_latest: bool,
        snapshot: &Snapshot,
    ) -> Result<()> {
        let key = if is_latest {
            self.latest_snapshot_key(location)?
        } else {
            self.history_snapshot_key(location, snapshot_id)?
        };
        self.write_json(&key, snapshot).await
    }

    async fn load_snapshot(
        &self,
        location: &SnapshotLocation,
        snapshot_id: Option<&str>,
    ) -> Result<Option<Snapshot>> {
        let key = match snapshot_id {
            None => self.latest_snapshot_key(location)?,
            Some(id) => self.history_snapshot_key(location, id)?,
        };
        self.read_json(&key).await
    }

    async fn list_snapshot_ids(
        &self,
        location: &SnapshotLocation,
        limit: Option<usize>,
        start_after: Option<&str>,
    ) -> Result<Vec<String>> {
        if limit == Some(0) {
            return Ok(Vec::new());
        }
        if let Some(cursor) = start_after {
            validate_uuid_v7(cursor)?;
        }
        let prefix = self.get_key(location, IMMUTABLE_HISTORY)?;
        // S3 `StartAfter` is a full object key; build it from the uuid7 cursor. It is exclusive,
        // which matches our pagination contract.
        let start_after_key = match start_after {
            Some(cursor) => Some(self.history_snapshot_key(location, cursor)?),
            None => None,
        };
        let mut ids: Vec<String> = Vec::new();
        let mut continuation_token: Option<String> = None;
        loop {
            let max_keys = match limit {
                Some(limit) => S3_PAGE_SIZE.min((limit - ids.len()) as i32),
                None => S3_PAGE_SIZE,
            };
            let mut request = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(&prefix)
                .max_keys(max_keys)
                .set_continuation_token(continuation_token.clone());
            if continuation_token.is_none() {
                request = request.set_start_after(start_after_key.clone());
            }
            let response = match request.send().await {
                Ok(response) => response,
                Err(err) if is_not_found(&err) => return Ok(Vec::new()),
                Err(err) => {
                    return Err(session_error(
                        format!("Failed to list snapshots for session {}", location.session_id),
                        err,
                    ))
                }
            };
            ids.extend(
                response
                    .contents()
                    .iter()
                    .filter_map(|object| object.key().and_then(snapshot_id_from_key))
                    .map(str::to_owned),
            );
            if !response.is_truncated().unwrap_or(false) {
                break;
            }
            let Some(token) = response.next_continuation_token() else {
                return Err(Error::Session {
                    message: "S3 returned truncated response without continuation token".to_owned(),
                    source: None,
                });
            };
            continuation_token = Some(token.to_owned());
            if limit.is_some_and(|limit| ids.len() >= limit) {
                break;
            }
        }
        if let Some(limit) = limit {
            ids.truncate(limit);
        }
        Ok(ids)
    }

    async fn delete_session(&self, session_id: &str) -> Result<()> {
        let prefix = self.session_prefix(session_id)?;
        let mut continuation_token: Option<String> = None;
        loop {
            let response = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(&prefix)
                .max_keys(S3_PAGE_SIZE)
                .set_continuation_token(continuation_token)
                .send()
                .await
                .map_err(|err| session_error(format!("Failed to delete session {session_id}"), err))?;
            let objects: Vec<ObjectIdentifier> = response
                .contents()
                .iter()
                .filter_map(|object| object.key())
                .map(|key| ObjectIdentifier::builder().key(key).build())
                .collect::<std::result::Result<_, _>>()
                .map_err(|err| session_error(format!("Failed to delete session {session_id}"), err))?;
            if !objects.is_empty() {
                let delete = Delete::builder()
                    .set_objects(Some(objects))
                    .build()
                    .map_err(|err| session_error(format!("Failed to delete session {session_id}"), err))?;
                self.client
                    .delete_objects()
                    .bucket(&self.bucket)
                    .delete(delete)
                    .send()
                    .await
                    .map_err(|err| session_error(format!("Failed to delete session {session_id}"), err))?;
            }
            continuation_token = response
                .is_truncated()
                .unwrap_or(false)
                .then(|| response.next_continuation_token().map(str::to_owned))
                .flatten();
            if continuation_token.is_none() {
                break;
            }
        }
        Ok(())
    }

    async fn load_manifest(&self, location: &SnapshotLocation) -> Result<SnapshotManifest> {
        let key = self.get_key(location, MANIFEST)?;
        Ok(self.read_json(&key).await?.unwrap_or_else(|| SnapshotManifest {
            schema_version: SNAPSHOT_SCHEMA_VERSION.to_owned(),
            updated_at: now_iso8601(),
        }))
    }

    async fn save_manifest(&self, location: &SnapshotLocation, manifest: &SnapshotManifest) -> Result<()> {
        let key = self.get_key(location, MANIFEST)?;
        self.write_json(&key, manifest).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aws_sdk_s3::operation::delete_objects::DeleteObjectsOutput;
    use aws_sdk_s3::operation::get_object::{GetObjectError, GetObjectOutput};
    use aws_sdk_s3::operation::list_objects_v2::{ListObjectsV2Error, ListObjectsV2Output};
    use aws_sdk_s3::operation::put_object::{PutObjectError, PutObjectOutput};
    use aws_sdk_s3::primitives::ByteStream;
    use aws_sdk_s3::types::error::NoSuchKey;
    use aws_sdk_s3::types::Object;
    use aws_smithy_mocks::{mock, mock_client, Rule, RuleMode};
    use aws_smithy_types::error::ErrorMetadata;
    use parking_lot::Mutex;
    use serde_json::{json, Map};

    use super::*;
    use crate::session::types::Scope;

    const SCOPE_ID: &str = "test-agent";
    const IDS: [&str; 3] = [
        "019c9bf1-14e5-7eef-96fb-cc07ae54210f",
        "019c9bf1-1d34-7eef-96fb-d1be20fd7bbd",
        "019c9bf1-24bb-7eef-96fb-ddcc943cd859",
    ];
    const HISTORY_PREFIX: &str = "test-session/scopes/agent/test-agent/snapshots/immutable_history/";

    /// Requests a rule saw, formatted `bucket|key` (plus `|body` for puts).
    type Seen = Arc<Mutex<Vec<String>>>;

    fn seen() -> Seen {
        Arc::new(Mutex::new(Vec::new()))
    }

    async fn build(client: Client, prefix: Option<&str>) -> S3SessionStorage {
        let mut builder = S3SessionStorage::builder("test-bucket").client(client);
        if let Some(prefix) = prefix {
            builder = builder.prefix(prefix);
        }
        builder.build().await.expect("builds with a client")
    }

    async fn storage(rule: &Rule, prefix: Option<&str>) -> S3SessionStorage {
        build(mock_client!(aws_sdk_s3, [rule]), prefix).await
    }

    fn error_metadata(code: &str) -> ErrorMetadata {
        ErrorMetadata::builder().code(code).message(code).build()
    }

    fn location() -> SnapshotLocation {
        SnapshotLocation::agent("test-session", SCOPE_ID)
    }

    fn test_snapshot() -> Snapshot {
        Snapshot {
            scope: Scope::Agent,
            schema_version: "1.0".to_owned(),
            created_at: "2024-01-01T00:00:00.000Z".to_owned(),
            data: json!({ "messages": [], "state": { "testKey": "testValue" } })
                .as_object()
                .expect("object")
                .clone(),
            app_data: Map::new(),
        }
    }

    fn put_rule(seen: &Seen) -> Rule {
        let captured = Arc::clone(seen);
        mock!(Client::put_object)
            .match_requests(move |req| {
                captured.lock().push(format!(
                    "{}|{}|{}",
                    req.bucket().unwrap_or_default(),
                    req.key().unwrap_or_default(),
                    req.content_type().unwrap_or_default()
                ));
                true
            })
            .then_output(|| PutObjectOutput::builder().build())
    }

    fn get_rule(body: &'static str, seen: &Seen) -> Rule {
        let captured = Arc::clone(seen);
        mock!(Client::get_object)
            .match_requests(move |req| {
                captured.lock().push(format!(
                    "{}|{}",
                    req.bucket().unwrap_or_default(),
                    req.key().unwrap_or_default()
                ));
                true
            })
            .then_output(move || {
                GetObjectOutput::builder()
                    .body(ByteStream::from_static(body.as_bytes()))
                    .build()
            })
    }

    fn history_objects(ids: &[&str]) -> Vec<Object> {
        ids.iter()
            .map(|id| {
                Object::builder()
                    .key(format!("{HISTORY_PREFIX}snapshot_{id}.json"))
                    .build()
            })
            .collect()
    }

    fn list_rule(objects: Vec<Object>, seen: &Seen) -> Rule {
        let captured = Arc::clone(seen);
        mock!(Client::list_objects_v2)
            .match_requests(move |req| {
                captured.lock().push(format!(
                    "{}|{}|{}|{}",
                    req.bucket().unwrap_or_default(),
                    req.prefix().unwrap_or_default(),
                    req.max_keys().unwrap_or_default(),
                    req.start_after().unwrap_or("none"),
                ));
                true
            })
            .then_output(move || {
                ListObjectsV2Output::builder()
                    .set_contents(Some(objects.clone()))
                    .is_truncated(false)
                    .build()
            })
    }

    // ------------------------------------------------------------------ constructor

    #[tokio::test]
    async fn stores_bucket_and_region_configuration() {
        let storage = S3SessionStorage::builder("test-bucket")
            .region("us-west-2")
            .build()
            .await
            .expect("builds from a region");

        assert_eq!(storage.bucket(), "test-bucket");
        assert_eq!(storage.prefix(), "");
    }

    #[tokio::test]
    async fn stores_prefix_when_provided() {
        let rule = put_rule(&seen());

        assert_eq!(storage(&rule, Some("sessions")).await.prefix(), "sessions/");
        assert_eq!(storage(&rule, Some("/a//b/")).await.prefix(), "a/b/");
        assert_eq!(storage(&rule, Some("")).await.prefix(), "");
    }

    #[tokio::test]
    async fn uses_provided_s3_client_instead_of_creating_new_one() {
        let rule = put_rule(&seen());
        let storage = storage(&rule, None).await;

        storage
            .save_manifest(
                &location(),
                &SnapshotManifest {
                    schema_version: "1.0".to_owned(),
                    updated_at: "2024-01-01T00:00:00.000Z".to_owned(),
                },
            )
            .await
            .expect("saves through the mock client");

        assert_eq!(rule.num_calls(), 1);
    }

    #[tokio::test]
    async fn throws_error_when_both_s3_client_and_region_are_provided() {
        let rule = put_rule(&seen());
        let error = S3SessionStorage::builder("test-bucket")
            .client(mock_client!(aws_sdk_s3, [&rule]))
            .region("us-west-2")
            .build()
            .await
            .map(|_| ())
            .expect_err("conflicting client and region");

        assert!(matches!(error, Error::Session { .. }), "{error:?}");
        assert!(
            error
                .to_string()
                .starts_with("Cannot specify both client and region"),
            "{error}"
        );
    }

    // ------------------------------------------------------------------ save_snapshot

    #[tokio::test]
    async fn saves_snapshot_to_s3_history() {
        let seen = seen();
        let rule = put_rule(&seen);
        let storage = storage(&rule, None).await;

        storage
            .save_snapshot(&location(), IDS[0], false, &test_snapshot())
            .await
            .expect("saves");

        assert_eq!(
            *seen.lock(),
            vec![format!(
                "test-bucket|{HISTORY_PREFIX}snapshot_{}.json|application/json",
                IDS[0]
            )]
        );
    }

    #[tokio::test]
    async fn saves_snapshot_as_latest_when_is_latest_is_true() {
        let seen = seen();
        let rule = put_rule(&seen);
        let storage = storage(&rule, None).await;

        storage
            .save_snapshot(&location(), IDS[0], true, &test_snapshot())
            .await
            .expect("saves");

        assert_eq!(
            *seen.lock(),
            vec![
                "test-bucket|test-session/scopes/agent/test-agent/snapshots/snapshot_latest.json|application/json"
                    .to_owned()
            ]
        );
    }

    #[tokio::test]
    async fn uses_prefix_when_configured() {
        let seen = seen();
        let rule = put_rule(&seen);
        let storage = storage(&rule, Some("my-prefix")).await;

        storage
            .save_snapshot(&location(), IDS[0], true, &test_snapshot())
            .await
            .expect("saves");

        assert!(
            seen.lock()[0].contains("|my-prefix/test-session/scopes/agent/"),
            "{:?}",
            seen.lock()
        );
    }

    #[tokio::test]
    async fn throws_session_error_when_s3_put_fails() {
        let rule =
            mock!(Client::put_object).then_error(|| PutObjectError::generic(error_metadata("AccessDenied")));
        let storage = storage(&rule, None).await;

        let error = storage
            .save_snapshot(&location(), IDS[0], true, &test_snapshot())
            .await
            .expect_err("put fails");

        assert!(matches!(error, Error::Session { .. }), "{error:?}");
        assert!(
            error.to_string().starts_with("Failed to write S3 object "),
            "{error}"
        );
    }

    #[tokio::test]
    async fn rejects_a_scope_id_containing_path_traversal() {
        let rule = put_rule(&seen());
        let storage = storage(&rule, None).await;
        let location = SnapshotLocation::agent("test-session", "../evil");

        let error = storage
            .save_snapshot(&location, IDS[0], true, &test_snapshot())
            .await
            .expect_err("invalid scope id");

        assert!(
            error.to_string().contains("'../evil' can only contain lowercase"),
            "{error}"
        );
        assert_eq!(rule.num_calls(), 0);
    }

    #[tokio::test]
    async fn saves_multi_agent_snapshot_to_correct_s3_key() {
        let seen = seen();
        let rule = put_rule(&seen);
        let storage = storage(&rule, None).await;
        let location = SnapshotLocation {
            session_id: "test-session".to_owned(),
            scope: Scope::MultiAgent,
            scope_id: "test-graph".to_owned(),
        };

        storage
            .save_snapshot(&location, IDS[0], true, &test_snapshot())
            .await
            .expect("saves");

        assert_eq!(
            *seen.lock(),
            vec![
                "test-bucket|test-session/scopes/multiAgent/test-graph/snapshots/snapshot_latest.json|application/json"
                    .to_owned()
            ]
        );
    }

    // ------------------------------------------------------------------ load_snapshot

    #[tokio::test]
    async fn loads_latest_snapshot_when_snapshot_id_is_none() {
        let seen = seen();
        let body = r#"{"scope":"agent","schemaVersion":"1.0","createdAt":"2024-01-01T00:00:00.000Z","data":{},"appData":{}}"#;
        let rule = get_rule(body, &seen);
        let storage = storage(&rule, None).await;

        let snapshot = storage.load_snapshot(&location(), None).await.expect("loads");

        assert_eq!(snapshot.expect("some").created_at, "2024-01-01T00:00:00.000Z");
        assert_eq!(
            *seen.lock(),
            vec![
                "test-bucket|test-session/scopes/agent/test-agent/snapshots/snapshot_latest.json".to_owned()
            ]
        );
    }

    #[tokio::test]
    async fn loads_specific_snapshot_by_id() {
        let seen = seen();
        let body = r#"{"scope":"agent","schemaVersion":"1.0","createdAt":"2024-01-01T00:00:00.000Z","data":{},"appData":{}}"#;
        let rule = get_rule(body, &seen);
        let storage = storage(&rule, None).await;

        let snapshot = storage
            .load_snapshot(&location(), Some(IDS[1]))
            .await
            .expect("loads");

        assert!(snapshot.is_some());
        assert_eq!(
            *seen.lock(),
            vec![format!("test-bucket|{HISTORY_PREFIX}snapshot_{}.json", IDS[1])]
        );
    }

    #[tokio::test]
    async fn returns_none_when_s3_object_does_not_exist() {
        let rule =
            mock!(Client::get_object).then_error(|| GetObjectError::NoSuchKey(NoSuchKey::builder().build()));
        let storage = storage(&rule, None).await;

        assert_eq!(storage.load_snapshot(&location(), None).await.expect("ok"), None);
    }

    #[tokio::test]
    async fn returns_none_when_s3_response_body_is_empty() {
        let rule = get_rule("", &seen());
        let storage = storage(&rule, None).await;

        assert_eq!(storage.load_snapshot(&location(), None).await.expect("ok"), None);
    }

    #[tokio::test]
    async fn throws_session_error_when_json_is_invalid() {
        let rule = get_rule("not json", &seen());
        let storage = storage(&rule, None).await;

        let error = storage
            .load_snapshot(&location(), None)
            .await
            .expect_err("invalid json");

        assert!(matches!(error, Error::Session { .. }), "{error:?}");
        assert!(
            error.to_string().starts_with("Invalid JSON in S3 object "),
            "{error}"
        );
    }

    #[tokio::test]
    async fn throws_session_error_when_s3_get_fails() {
        let rule =
            mock!(Client::get_object).then_error(|| GetObjectError::generic(error_metadata("AccessDenied")));
        let storage = storage(&rule, None).await;

        let error = storage
            .load_snapshot(&location(), None)
            .await
            .expect_err("get fails");

        assert!(error.to_string().starts_with("S3 error reading "), "{error}");
    }

    // ------------------------------------------------------------------ list_snapshot_ids

    #[tokio::test]
    async fn returns_sorted_snapshot_ids() {
        let seen = seen();
        let rule = list_rule(history_objects(&IDS), &seen);
        let storage = storage(&rule, None).await;

        let ids = storage
            .list_snapshot_ids(&location(), None, None)
            .await
            .expect("lists");

        assert_eq!(ids, IDS.map(str::to_owned).to_vec());
        assert_eq!(
            *seen.lock(),
            vec![format!("test-bucket|{HISTORY_PREFIX}|1000|none")]
        );
    }

    #[tokio::test]
    async fn returns_empty_vec_when_no_objects_exist() {
        let rule = list_rule(Vec::new(), &seen());
        let storage = storage(&rule, None).await;

        assert_eq!(
            storage
                .list_snapshot_ids(&location(), None, None)
                .await
                .expect("lists"),
            Vec::<String>::new()
        );
    }

    #[tokio::test]
    async fn ignores_non_snapshot_objects() {
        let objects = vec![
            Object::builder()
                .key(format!("{HISTORY_PREFIX}snapshot_{}.json", IDS[0]))
                .build(),
            Object::builder()
                .key(format!("{HISTORY_PREFIX}readme.txt"))
                .build(),
            Object::builder()
                .key(format!("{HISTORY_PREFIX}snapshot_{}.txt", IDS[1]))
                .build(),
        ];
        let rule = list_rule(objects, &seen());
        let storage = storage(&rule, None).await;

        assert_eq!(
            storage
                .list_snapshot_ids(&location(), None, None)
                .await
                .expect("lists"),
            vec![IDS[0].to_owned()]
        );
    }

    #[tokio::test]
    async fn handles_objects_without_key_property() {
        let objects = vec![
            Object::builder().build(),
            Object::builder()
                .key(format!("{HISTORY_PREFIX}snapshot_{}.json", IDS[0]))
                .build(),
        ];
        let rule = list_rule(objects, &seen());
        let storage = storage(&rule, None).await;

        assert_eq!(
            storage
                .list_snapshot_ids(&location(), None, None)
                .await
                .expect("lists"),
            vec![IDS[0].to_owned()]
        );
    }

    #[tokio::test]
    async fn filters_by_start_after_for_pagination() {
        let seen = seen();
        let rule = list_rule(history_objects(&IDS[1..]), &seen);
        let storage = storage(&rule, None).await;

        let ids = storage
            .list_snapshot_ids(&location(), None, Some(IDS[0]))
            .await
            .expect("lists");

        assert_eq!(ids, vec![IDS[1].to_owned(), IDS[2].to_owned()]);
        assert_eq!(
            *seen.lock(),
            vec![format!(
                "test-bucket|{HISTORY_PREFIX}|1000|{HISTORY_PREFIX}snapshot_{}.json",
                IDS[0]
            )]
        );
    }

    #[tokio::test]
    async fn limits_results_when_limit_is_provided() {
        let seen = seen();
        let rule = list_rule(history_objects(&IDS), &seen);
        let storage = storage(&rule, None).await;

        let ids = storage
            .list_snapshot_ids(&location(), Some(2), None)
            .await
            .expect("lists");

        assert_eq!(ids, vec![IDS[0].to_owned(), IDS[1].to_owned()]);
        assert_eq!(*seen.lock(), vec![format!("test-bucket|{HISTORY_PREFIX}|2|none")]);
    }

    #[tokio::test]
    async fn combines_start_after_and_limit() {
        let rule = list_rule(history_objects(&IDS[1..]), &seen());
        let storage = storage(&rule, None).await;

        let ids = storage
            .list_snapshot_ids(&location(), Some(1), Some(IDS[0]))
            .await
            .expect("lists");

        assert_eq!(ids, vec![IDS[1].to_owned()]);
    }

    #[tokio::test]
    async fn returns_empty_vec_when_limit_is_zero() {
        let rule = list_rule(history_objects(&IDS), &seen());
        let storage = storage(&rule, None).await;

        assert_eq!(
            storage
                .list_snapshot_ids(&location(), Some(0), None)
                .await
                .expect("lists"),
            Vec::<String>::new()
        );
        assert_eq!(rule.num_calls(), 0);
    }

    #[tokio::test]
    async fn rejects_a_start_after_cursor_that_is_not_a_uuid_v7() {
        let rule = list_rule(Vec::new(), &seen());
        let storage = storage(&rule, None).await;

        let error = storage
            .list_snapshot_ids(&location(), None, Some("../evil"))
            .await
            .expect_err("invalid cursor");

        assert_eq!(error.to_string(), "'../evil' is not a valid UUID v7 snapshot ID");
        assert_eq!(rule.num_calls(), 0);
    }

    #[tokio::test]
    async fn throws_session_error_when_s3_list_fails() {
        let rule = mock!(Client::list_objects_v2)
            .then_error(|| ListObjectsV2Error::generic(error_metadata("AccessDenied")));
        let storage = storage(&rule, None).await;

        let error = storage
            .list_snapshot_ids(&location(), None, None)
            .await
            .expect_err("list fails");

        assert!(matches!(error, Error::Session { .. }), "{error:?}");
        assert_eq!(
            error.to_string(),
            "Failed to list snapshots for session test-session"
        );
    }

    // ------------------------------------------------------------------ delete_session

    #[tokio::test]
    async fn deletes_all_objects_under_the_session_prefix() {
        let seen = seen();
        let list = list_rule(history_objects(&IDS[..2]), &seen);
        let deleted = seen.clone();
        let delete = mock!(Client::delete_objects)
            .match_requests(move |req| {
                let keys: Vec<&str> = req
                    .delete()
                    .and_then(|d| Some(d.objects()))
                    .map(|objects| objects.iter().map(|o| o.key()).collect())
                    .unwrap_or_default();
                deleted.lock().push(format!("delete|{}", keys.join(",")));
                true
            })
            .then_output(|| DeleteObjectsOutput::builder().build());
        let storage = build(mock_client!(aws_sdk_s3, [&list, &delete]), None).await;

        storage.delete_session("test-session").await.expect("deletes");

        assert_eq!(list.num_calls(), 1);
        assert_eq!(delete.num_calls(), 1);
        let log = seen.lock().clone();
        assert_eq!(log[0], "test-bucket|test-session/|1000|none");
        assert_eq!(
            log[1],
            format!(
                "delete|{HISTORY_PREFIX}snapshot_{}.json,{HISTORY_PREFIX}snapshot_{}.json",
                IDS[0], IDS[1]
            )
        );
    }

    #[tokio::test]
    async fn paginates_when_session_has_more_than_1000_objects() {
        let list = mock!(Client::list_objects_v2)
            .sequence()
            .output(|| {
                ListObjectsV2Output::builder()
                    .set_contents(Some(history_objects(&IDS[..1])))
                    .is_truncated(true)
                    .next_continuation_token("token1")
                    .build()
            })
            .output(|| {
                ListObjectsV2Output::builder()
                    .set_contents(Some(history_objects(&IDS[1..2])))
                    .is_truncated(false)
                    .build()
            })
            .build();
        let delete = mock!(Client::delete_objects).then_output(|| DeleteObjectsOutput::builder().build());
        let storage = build(
            mock_client!(aws_sdk_s3, RuleMode::MatchAny, [&list, &delete]),
            None,
        )
        .await;

        storage.delete_session("test-session").await.expect("deletes");

        assert_eq!(list.num_calls(), 2);
        assert_eq!(delete.num_calls(), 2);
    }

    #[tokio::test]
    async fn no_ops_when_session_has_no_objects() {
        let list = list_rule(Vec::new(), &seen());
        let delete = mock!(Client::delete_objects).then_output(|| DeleteObjectsOutput::builder().build());
        let storage = build(mock_client!(aws_sdk_s3, [&list, &delete]), None).await;

        storage.delete_session("test-session").await.expect("no-ops");

        assert_eq!(list.num_calls(), 1);
        assert_eq!(delete.num_calls(), 0);
    }

    #[tokio::test]
    async fn uses_prefix_when_configured_for_delete() {
        let seen = seen();
        let list = list_rule(Vec::new(), &seen);
        let storage = storage(&list, Some("my-prefix")).await;

        storage.delete_session("test-session").await.expect("no-ops");

        assert_eq!(
            *seen.lock(),
            vec!["test-bucket|my-prefix/test-session/|1000|none".to_owned()]
        );
    }

    #[tokio::test]
    async fn throws_session_error_when_s3_list_fails_during_delete() {
        let rule = mock!(Client::list_objects_v2)
            .then_error(|| ListObjectsV2Error::generic(error_metadata("AccessDenied")));
        let storage = storage(&rule, None).await;

        let error = storage
            .delete_session("test-session")
            .await
            .expect_err("list fails");

        assert!(matches!(error, Error::Session { .. }), "{error:?}");
        assert_eq!(error.to_string(), "Failed to delete session test-session");
    }

    // ------------------------------------------------------------------ manifest

    #[tokio::test]
    async fn loads_existing_manifest() {
        let seen = seen();
        let rule = get_rule(
            r#"{"schemaVersion":"1.0","updatedAt":"2024-01-01T00:00:00.000Z"}"#,
            &seen,
        );
        let storage = storage(&rule, None).await;

        let manifest = storage.load_manifest(&location()).await.expect("loads");

        assert_eq!(manifest.updated_at, "2024-01-01T00:00:00.000Z");
        assert_eq!(
            *seen.lock(),
            vec!["test-bucket|test-session/scopes/agent/test-agent/snapshots/manifest.json".to_owned()]
        );
    }

    #[tokio::test]
    async fn returns_default_manifest_when_s3_object_does_not_exist() {
        let rule =
            mock!(Client::get_object).then_error(|| GetObjectError::NoSuchKey(NoSuchKey::builder().build()));
        let storage = storage(&rule, None).await;

        let manifest = storage.load_manifest(&location()).await.expect("loads");

        assert_eq!(manifest.schema_version, SNAPSHOT_SCHEMA_VERSION);
        assert!(manifest.updated_at.ends_with('Z'), "{}", manifest.updated_at);
    }

    #[tokio::test]
    async fn throws_session_error_when_manifest_json_is_invalid() {
        let rule = get_rule("{invalid}", &seen());
        let storage = storage(&rule, None).await;

        let error = storage
            .load_manifest(&location())
            .await
            .expect_err("invalid json");

        assert!(
            error.to_string().starts_with("Invalid JSON in S3 object "),
            "{error}"
        );
    }

    #[tokio::test]
    async fn saves_manifest_to_s3() {
        let seen = seen();
        let rule = put_rule(&seen);
        let storage = storage(&rule, None).await;

        storage
            .save_manifest(
                &location(),
                &SnapshotManifest {
                    schema_version: "1.0".to_owned(),
                    updated_at: "2024-01-01T00:00:00.000Z".to_owned(),
                },
            )
            .await
            .expect("saves");

        assert_eq!(
            *seen.lock(),
            vec![
                "test-bucket|test-session/scopes/agent/test-agent/snapshots/manifest.json|application/json"
                    .to_owned()
            ]
        );
    }

    #[tokio::test]
    async fn throws_session_error_when_manifest_put_fails() {
        let rule =
            mock!(Client::put_object).then_error(|| PutObjectError::generic(error_metadata("AccessDenied")));
        let storage = storage(&rule, None).await;

        let error = storage
            .save_manifest(
                &location(),
                &SnapshotManifest {
                    schema_version: "1.0".to_owned(),
                    updated_at: "2024-01-01T00:00:00.000Z".to_owned(),
                },
            )
            .await
            .expect_err("put fails");

        assert!(
            error.to_string().starts_with("Failed to write S3 object "),
            "{error}"
        );
    }
}
