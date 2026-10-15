//! Amazon S3 [`Storage`] backend (`strands-ts/src/storage/s3-storage.ts`).
//!
//! Each key becomes one S3 object at `[<prefix>/]<key>`, so a bucket written by the TS or Python
//! SDK is readable from Rust and vice versa.
//!
//! Divergences from TS: the client is built by [`S3Storage::builder`] (there is no lazy
//! first-use construction — resolving the AWS config is async), and the `prefix` goes through
//! [`normalize_prefix`], so `..` or `\` in a prefix is an error instead of a literal key segment
//! (as in Python's `S3Storage`).

use std::sync::Arc;

use async_trait::async_trait;
use aws_sdk_s3::error::ProvideErrorMetadata;
use aws_sdk_s3::operation::get_object::GetObjectError;
use aws_sdk_s3::Client;

use super::search::{KeywordSearchStrategy, SearchStrategy, SearchStrategyRef};
use super::{normalize_key, normalize_prefix, Storage, StorageSearchResult};
use crate::error::{BoxError, Error, Result};

/// Objects requested per `ListObjectsV2` page.
const S3_PAGE_SIZE: i32 = 1000;

/// Keys are stored as S3 objects under an optional bucket prefix.
pub struct S3Storage {
    bucket: String,
    prefix: String,
    client: Client,
    search_strategy: Option<SearchStrategyRef>,
}

/// Builder for [`S3Storage`]: prefix, region or pre-configured client, search strategy.
#[derive(Default)]
pub struct S3StorageBuilder {
    bucket: String,
    prefix: Option<String>,
    region: Option<String>,
    client: Option<Client>,
    search_strategy: Option<SearchStrategyRef>,
}

impl S3Storage {
    /// Storage in `bucket` with no prefix, resolving region and credentials from the AWS chain.
    pub async fn new(bucket: impl Into<String>) -> Result<Self> {
        Self::builder(bucket).build().await
    }

    /// Builder for prefix / region / client / search-strategy overrides.
    pub fn builder(bucket: impl Into<String>) -> S3StorageBuilder {
        S3StorageBuilder {
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

    fn object_key(&self, key: &str) -> String {
        format!("{}{key}", self.prefix)
    }
}

impl S3StorageBuilder {
    /// Key prefix prepended to every key (a namespace within the bucket).
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    /// Region override; cannot be combined with [`S3StorageBuilder::client`].
    pub fn region(mut self, region: impl Into<String>) -> Self {
        self.region = Some(region.into());
        self
    }

    /// Pre-configured S3 client; cannot be combined with [`S3StorageBuilder::region`].
    pub fn client(mut self, client: Client) -> Self {
        self.client = Some(client);
        self
    }

    /// Strategy used for [`Storage::search`] and indexed on every write.
    pub fn search_strategy(mut self, strategy: impl SearchStrategy + 'static) -> Self {
        self.search_strategy = Some(Arc::new(strategy));
        self
    }

    /// Resolve the AWS config (unless a client was supplied) and build the storage.
    pub async fn build(self) -> Result<S3Storage> {
        if self.client.is_some() && self.region.is_some() {
            return Err(Error::storage(
                "Cannot specify both client and region. Configure the region on the S3 client instead.",
            ));
        }
        let prefix = resolve_prefix(self.prefix.as_deref())?;
        let client = match self.client {
            Some(client) => client,
            None => s3_client(self.region.as_deref()).await,
        };
        Ok(S3Storage {
            bucket: self.bucket,
            prefix,
            client,
            search_strategy: self.search_strategy,
        })
    }
}

/// `"a//b"` → `"a/b/"`, empty stays empty (`normalizePrefix` plus a trailing separator).
pub(crate) fn resolve_prefix(prefix: Option<&str>) -> Result<String> {
    let Some(prefix) = prefix else {
        return Ok(String::new());
    };
    let normalized = normalize_prefix(prefix)?;
    let normalized = normalized.trim_end_matches('/');
    if normalized.is_empty() {
        Ok(String::new())
    } else {
        Ok(format!("{normalized}/"))
    }
}

pub(crate) async fn s3_client(region: Option<&str>) -> Client {
    let sdk_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .load()
        .await;
    let mut config = aws_sdk_s3::config::Builder::from(&sdk_config);
    if let Some(region) = region {
        config = config.region(aws_sdk_s3::config::Region::new(region.to_owned()));
    }
    Client::from_conf(config.build())
}

/// `true` for the error codes S3 uses for a missing object or bucket.
fn is_not_found_code(error: &impl ProvideErrorMetadata) -> bool {
    matches!(error.code(), Some("NoSuchKey" | "NotFound" | "NoSuchBucket"))
}

fn storage_error(message: String, source: impl Into<BoxError>) -> Error {
    Error::Storage {
        message,
        source: Some(source.into()),
    }
}

#[async_trait]
impl Storage for S3Storage {
    async fn write(&self, key: &str, data: &[u8]) -> Result<()> {
        let normalized = normalize_key(key)?;
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(self.object_key(&normalized))
            .body(aws_sdk_s3::primitives::ByteStream::from(data.to_vec()))
            .send()
            .await
            .map_err(|error| {
                storage_error(
                    format!("Failed to write '{normalized}' to S3 bucket '{}'", self.bucket),
                    error,
                )
            })?;
        if let Some(strategy) = &self.search_strategy {
            strategy.index(self, &normalized, data).await?;
        }
        Ok(())
    }

    async fn read(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let normalized = normalize_key(key)?;
        let read_error = |error: BoxError| {
            storage_error(
                format!("Failed to read '{normalized}' from S3 bucket '{}'", self.bucket),
                error,
            )
        };
        let output = match self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(self.object_key(&normalized))
            .send()
            .await
        {
            Ok(output) => output,
            Err(error)
                if error
                    .as_service_error()
                    .is_some_and(GetObjectError::is_no_such_key)
                    || is_not_found_code(&error) =>
            {
                return Ok(None)
            }
            Err(error) => return Err(read_error(error.into())),
        };
        let bytes = output.body.collect().await.map_err(|e| read_error(e.into()))?;
        Ok(Some(bytes.to_vec()))
    }

    async fn delete(&self, key: &str) -> Result<()> {
        let normalized = normalize_key(key)?;
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(self.object_key(&normalized))
            .send()
            .await
            .map_err(|error| {
                storage_error(
                    format!("Failed to delete '{normalized}' from S3 bucket '{}'", self.bucket),
                    error,
                )
            })?;
        Ok(())
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let normalized = normalize_prefix(prefix)?;
        let list_prefix = format!("{}{normalized}", self.prefix);
        let mut keys = Vec::new();
        let mut continuation_token = None;
        loop {
            let output = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(&list_prefix)
                .max_keys(S3_PAGE_SIZE)
                .set_continuation_token(continuation_token)
                .send()
                .await
                .map_err(|error| {
                    storage_error(
                        format!("Failed to list S3 bucket '{}' under '{normalized}'", self.bucket),
                        error,
                    )
                })?;
            for object in output.contents() {
                if let Some(key) = object.key() {
                    keys.push(key.strip_prefix(self.prefix.as_str()).unwrap_or(key).to_owned());
                }
            }
            if !output.is_truncated().unwrap_or(false) {
                break;
            }
            continuation_token = output.next_continuation_token().map(str::to_owned);
            if continuation_token.is_none() {
                break;
            }
        }
        keys.sort();
        Ok(keys)
    }

    async fn search(&self, query: &str) -> Option<Result<Vec<StorageSearchResult>>> {
        Some(match &self.search_strategy {
            Some(strategy) => strategy.search(self, query).await,
            None => KeywordSearchStrategy.search(self, query).await,
        })
    }
}

#[cfg(test)]
mod tests {
    use aws_sdk_s3::operation::delete_object::{DeleteObjectError, DeleteObjectOutput};
    use aws_sdk_s3::operation::get_object::GetObjectOutput;
    use aws_sdk_s3::operation::list_objects_v2::{ListObjectsV2Error, ListObjectsV2Output};
    use aws_sdk_s3::operation::put_object::{PutObjectError, PutObjectOutput};
    use aws_sdk_s3::primitives::ByteStream;
    use aws_sdk_s3::types::error::NoSuchKey;
    use aws_sdk_s3::types::Object;
    use aws_smithy_mocks::{mock, mock_client, Rule};
    use aws_smithy_types::error::ErrorMetadata;
    use parking_lot::Mutex;

    use super::*;

    /// Requests a rule saw, formatted `bucket|key` (plus `|body` for puts).
    type Seen = Arc<Mutex<Vec<String>>>;

    fn seen() -> Seen {
        Arc::new(Mutex::new(Vec::new()))
    }

    async fn storage(rule: &Rule, prefix: Option<&str>) -> S3Storage {
        let mut builder = S3Storage::builder("my-bucket").client(mock_client!(aws_sdk_s3, [rule]));
        if let Some(prefix) = prefix {
            builder = builder.prefix(prefix);
        }
        builder.build().await.expect("builds with a client")
    }

    fn error_metadata(code: &str) -> ErrorMetadata {
        ErrorMetadata::builder().code(code).message(code).build()
    }

    fn object(key: &str) -> Object {
        Object::builder().key(key).build()
    }

    fn no_op_rule() -> Rule {
        mock!(Client::get_object).then_output(|| GetObjectOutput::builder().build())
    }

    // ------------------------------------------------------------------ constructor

    #[tokio::test]
    async fn throws_when_both_s3_client_and_region_are_provided() {
        let rule = no_op_rule();
        let error = S3Storage::builder("bucket")
            .client(mock_client!(aws_sdk_s3, [&rule]))
            .region("us-west-2")
            .build()
            .await
            .map(|_| ())
            .expect_err("conflicting client and region");

        assert!(matches!(error, Error::Storage { .. }), "{error:?}");
        assert!(
            error
                .to_string()
                .starts_with("Cannot specify both client and region"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn accepts_just_a_bucket_name() {
        let storage = storage(&no_op_rule(), None).await;

        assert_eq!(storage.bucket(), "my-bucket");
        assert_eq!(storage.prefix(), "");
    }

    #[tokio::test]
    async fn normalizes_the_configured_prefix() {
        let rule = no_op_rule();

        assert_eq!(storage(&rule, Some("agents/")).await.prefix(), "agents/");
        assert_eq!(
            storage(&rule, Some("/agents//nested")).await.prefix(),
            "agents/nested/"
        );
        assert_eq!(storage(&rule, Some("")).await.prefix(), "");
    }

    // ------------------------------------------------------------------ write

    #[tokio::test]
    async fn sends_a_put_object_command_with_the_correct_params() {
        let seen = seen();
        let captured = Arc::clone(&seen);
        let rule = mock!(Client::put_object)
            .match_requests(move |req| {
                captured.lock().push(format!(
                    "{}|{}|{}",
                    req.bucket().unwrap_or_default(),
                    req.key().unwrap_or_default(),
                    String::from_utf8_lossy(req.body().bytes().unwrap_or_default())
                ));
                true
            })
            .then_output(|| PutObjectOutput::builder().build());
        let storage = storage(&rule, Some("agents/")).await;

        storage.write("sessions/abc/data.json", b"payload").await.unwrap();

        assert_eq!(
            *seen.lock(),
            vec!["my-bucket|agents/sessions/abc/data.json|payload".to_owned()]
        );
        assert_eq!(rule.num_calls(), 1);
    }

    #[tokio::test]
    async fn wraps_sdk_errors_in_storage_error_on_write() {
        let rule =
            mock!(Client::put_object).then_error(|| PutObjectError::generic(error_metadata("AccessDenied")));
        let storage = storage(&rule, None).await;

        let error = storage.write("key", &[1]).await.expect_err("write fails");
        assert!(matches!(error, Error::Storage { .. }), "{error:?}");
        assert_eq!(
            error.to_string(),
            "Failed to write 'key' to S3 bucket 'my-bucket'"
        );
    }

    #[tokio::test]
    async fn rejects_an_invalid_key_before_calling_s3() {
        let rule = mock!(Client::put_object).then_output(|| PutObjectOutput::builder().build());
        let storage = storage(&rule, None).await;

        let error = storage.write("a/../b", &[1]).await.expect_err("invalid key");
        assert_eq!(
            error.to_string(),
            "Invalid storage key 'a/../b': '..' path segments are not allowed"
        );
        assert_eq!(rule.num_calls(), 0);
    }

    // ------------------------------------------------------------------ read

    #[tokio::test]
    async fn returns_bytes_when_the_object_exists() {
        let seen = seen();
        let captured = Arc::clone(&seen);
        let rule = mock!(Client::get_object)
            .match_requests(move |req| {
                captured.lock().push(format!(
                    "{}|{}",
                    req.bucket().unwrap_or_default(),
                    req.key().unwrap_or_default()
                ));
                true
            })
            .then_output(|| {
                GetObjectOutput::builder()
                    .body(ByteStream::from_static(&[1, 2, 3]))
                    .build()
            });
        let storage = storage(&rule, None).await;

        assert_eq!(storage.read("some/key").await.unwrap(), Some(vec![1, 2, 3]));
        assert_eq!(*seen.lock(), vec!["my-bucket|some/key".to_owned()]);
    }

    #[tokio::test]
    async fn returns_null_for_no_such_key() {
        let rule =
            mock!(Client::get_object).then_error(|| GetObjectError::NoSuchKey(NoSuchKey::builder().build()));
        let storage = storage(&rule, None).await;

        assert_eq!(storage.read("missing").await.unwrap(), None);
    }

    #[tokio::test]
    async fn returns_null_for_not_found() {
        let rule =
            mock!(Client::get_object).then_error(|| GetObjectError::generic(error_metadata("NotFound")));
        let storage = storage(&rule, None).await;

        assert_eq!(storage.read("missing").await.unwrap(), None);
    }

    #[tokio::test]
    async fn wraps_other_errors_in_storage_error_on_read() {
        let rule = mock!(Client::get_object)
            .then_error(|| GetObjectError::generic(error_metadata("NetworkFailure")));
        let storage = storage(&rule, None).await;

        let error = storage.read("key").await.expect_err("read fails");
        assert!(matches!(error, Error::Storage { .. }), "{error:?}");
        assert_eq!(
            error.to_string(),
            "Failed to read 'key' from S3 bucket 'my-bucket'"
        );
    }

    // ------------------------------------------------------------------ delete

    #[tokio::test]
    async fn sends_a_delete_object_command() {
        let seen = seen();
        let captured = Arc::clone(&seen);
        let rule = mock!(Client::delete_object)
            .match_requests(move |req| {
                captured.lock().push(format!(
                    "{}|{}",
                    req.bucket().unwrap_or_default(),
                    req.key().unwrap_or_default()
                ));
                true
            })
            .then_output(|| DeleteObjectOutput::builder().build());
        let storage = storage(&rule, Some("p/")).await;

        storage.delete("key").await.unwrap();

        assert_eq!(*seen.lock(), vec!["my-bucket|p/key".to_owned()]);
    }

    #[tokio::test]
    async fn wraps_errors_in_storage_error_on_delete() {
        let rule = mock!(Client::delete_object)
            .then_error(|| DeleteObjectError::generic(error_metadata("InternalError")));
        let storage = storage(&rule, None).await;

        let error = storage.delete("key").await.expect_err("delete fails");
        assert_eq!(
            error.to_string(),
            "Failed to delete 'key' from S3 bucket 'my-bucket'"
        );
    }

    // ------------------------------------------------------------------ list

    #[tokio::test]
    async fn returns_keys_with_prefix_stripped() {
        let seen = seen();
        let captured = Arc::clone(&seen);
        let rule = mock!(Client::list_objects_v2)
            .match_requests(move |req| {
                captured.lock().push(format!(
                    "{}|{}|{:?}",
                    req.bucket().unwrap_or_default(),
                    req.prefix().unwrap_or_default(),
                    req.max_keys()
                ));
                true
            })
            .then_output(|| {
                ListObjectsV2Output::builder()
                    .contents(object("prefix/a"))
                    .contents(object("prefix/b/c"))
                    .is_truncated(false)
                    .build()
            });
        let storage = storage(&rule, Some("prefix/")).await;

        assert_eq!(storage.list("").await.unwrap(), vec!["a", "b/c"]);
        assert_eq!(*seen.lock(), vec!["my-bucket|prefix/|Some(1000)".to_owned()]);
    }

    #[tokio::test]
    async fn paginates_until_is_truncated_is_false() {
        let seen = seen();
        let captured = Arc::clone(&seen);
        let rule = mock!(Client::list_objects_v2)
            .match_requests(move |req| {
                captured
                    .lock()
                    .push(req.continuation_token().unwrap_or("none").to_owned());
                true
            })
            .sequence()
            .output(|| {
                ListObjectsV2Output::builder()
                    .contents(object("a"))
                    .is_truncated(true)
                    .next_continuation_token("token1")
                    .build()
            })
            .output(|| {
                ListObjectsV2Output::builder()
                    .contents(object("b"))
                    .is_truncated(false)
                    .build()
            })
            .build();
        let storage = storage(&rule, None).await;

        assert_eq!(storage.list("").await.unwrap(), vec!["a", "b"]);
        assert_eq!(*seen.lock(), vec!["none".to_owned(), "token1".to_owned()]);
        assert_eq!(rule.num_calls(), 2);
    }

    #[tokio::test]
    async fn wraps_errors_in_storage_error_on_list() {
        let rule = mock!(Client::list_objects_v2)
            .then_error(|| ListObjectsV2Error::generic(error_metadata("BucketNotFound")));
        let storage = storage(&rule, None).await;

        let error = storage.list("prefix/").await.expect_err("list fails");
        assert_eq!(
            error.to_string(),
            "Failed to list S3 bucket 'my-bucket' under 'prefix/'"
        );
    }

    // ------------------------------------------------------------------ search

    #[tokio::test]
    async fn searches_stored_content_with_the_default_keyword_strategy() {
        let list = mock!(Client::list_objects_v2).then_output(|| {
            ListObjectsV2Output::builder()
                .contents(object("notes/alpha.txt"))
                .is_truncated(false)
                .build()
        });
        let get = mock!(Client::get_object).then_output(|| {
            GetObjectOutput::builder()
                .body(ByteStream::from_static(b"cactus"))
                .build()
        });
        let storage = S3Storage::builder("my-bucket")
            .client(mock_client!(aws_sdk_s3, [&list, &get]))
            .build()
            .await
            .unwrap();

        let hits = storage.search("cactus").await.unwrap().unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].key, "notes/alpha.txt");
    }
}
