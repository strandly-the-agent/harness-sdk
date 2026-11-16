//! Ported from strands-ts/src/storage/__tests__/local-file-storage.test.node.ts

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::{LocalFileStorage, LocalFileStorageConfig};
use crate::error::{Result, SandboxPathNotFoundError};
use crate::models::model::BoxStream;
use crate::sandbox::{ExecuteOptions, ExecutionEvent, FileInfo, NotASandboxLocalEnvironment, Sandbox};
use crate::storage::Storage;
use crate::storage::search::{SearchStrategy, StorageSearchResult};

fn setup() -> (tempfile::TempDir, String, LocalFileStorage) {
    let dir = tempfile::tempdir().unwrap();
    let base_dir = dir.path().join("store").to_string_lossy().into_owned();
    let storage = LocalFileStorage::new(base_dir.clone());
    (dir, base_dir, storage)
}

/// Records the sandbox file calls and can report every read as missing.
#[derive(Debug)]
struct RecordingSandbox {
    inner: NotASandboxLocalEnvironment,
    calls: Mutex<Vec<String>>,
    missing_reads: bool,
}

impl RecordingSandbox {
    fn new(missing_reads: bool) -> Arc<Self> {
        Arc::new(Self {
            inner: NotASandboxLocalEnvironment::new(),
            calls: Mutex::new(Vec::new()),
            missing_reads,
        })
    }

    fn record(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }
}

#[async_trait]
impl Sandbox for RecordingSandbox {
    fn execute_streaming<'a>(
        &'a self,
        command: &'a str,
        options: ExecuteOptions,
    ) -> BoxStream<'a, Result<ExecutionEvent>> {
        self.inner.execute_streaming(command, options)
    }

    fn execute_code_streaming<'a>(
        &'a self,
        code: &'a str,
        language: &'a str,
        options: ExecuteOptions,
    ) -> BoxStream<'a, Result<ExecutionEvent>> {
        self.inner.execute_code_streaming(code, language, options)
    }

    async fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        self.record(format!("read {path}"));
        if self.missing_reads {
            return Err(SandboxPathNotFoundError::new(path).into());
        }
        self.inner.read_file(path).await
    }

    async fn write_file(&self, path: &str, content: &[u8]) -> Result<()> {
        self.record(format!("write {path} {content:?}"));
        self.inner.write_file(path, content).await
    }

    async fn remove_file(&self, path: &str) -> Result<()> {
        self.record(format!("remove {path}"));
        self.inner.remove_file(path).await
    }

    async fn list_files(&self, path: &str) -> Result<Vec<FileInfo>> {
        self.record(format!("list {path}"));
        self.inner.list_files(path).await
    }
}

#[derive(Default)]
struct FakeStrategy {
    indexed: Mutex<Vec<String>>,
    searched: Mutex<Vec<String>>,
}

#[async_trait]
impl SearchStrategy<LocalFileStorage> for FakeStrategy {
    async fn index(&self, storage: &LocalFileStorage, key: &str, data: &[u8]) -> Result<()> {
        self.indexed
            .lock()
            .unwrap()
            .push(format!("{} {key} {data:?}", storage.base_dir()));
        Ok(())
    }

    async fn search(&self, storage: &LocalFileStorage, query: &str) -> Result<Vec<StorageSearchResult>> {
        self.searched
            .lock()
            .unwrap()
            .push(format!("{} {query}", storage.base_dir()));
        Ok(vec![StorageSearchResult::new("hit.txt", 1.0)])
    }
}

#[tokio::test]
async fn round_trips_bytes() {
    let (_dir, _, storage) = setup();
    storage.write("key", &[1, 2, 3]).await.unwrap();
    assert_eq!(storage.read("key").await.unwrap(), Some(vec![1, 2, 3]));
}

#[tokio::test]
async fn creates_nested_directories() {
    let (_dir, base_dir, storage) = setup();
    storage.write("a/b/c/key", &[1]).await.unwrap();
    assert_eq!(
        tokio::fs::read(format!("{base_dir}/a/b/c/key")).await.unwrap(),
        [1]
    );
}

#[tokio::test]
async fn overwrites_existing_values() {
    let (_dir, _, storage) = setup();
    storage.write("key", &[1]).await.unwrap();
    storage.write("key", &[2]).await.unwrap();
    assert_eq!(storage.read("key").await.unwrap(), Some(vec![2]));
}

#[tokio::test]
async fn returns_null_for_missing_keys() {
    let (_dir, _, storage) = setup();
    assert_eq!(storage.read("missing").await.unwrap(), None);
}

#[tokio::test]
async fn writes_atomically_via_tmp_file() {
    let (_dir, base_dir, storage) = setup();
    storage.write("atomic/test", &[1]).await.unwrap();
    assert_eq!(
        tokio::fs::read(format!("{base_dir}/atomic/test")).await.unwrap(),
        [1]
    );
    let mut entries = tokio::fs::read_dir(format!("{base_dir}/atomic")).await.unwrap();
    let mut names = Vec::new();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    assert_eq!(names, ["test"]);
}

#[cfg(unix)]
#[tokio::test]
async fn cleans_up_tmp_file_on_rename_failure() {
    let (_dir, base_dir, storage) = setup();
    let dir = format!("{base_dir}/target");
    // A directory in place of the target makes the rename fail after the temp file is written.
    tokio::fs::create_dir_all(format!("{dir}/occupied"))
        .await
        .unwrap();
    tokio::fs::write(format!("{dir}/occupied/inner"), b"x")
        .await
        .unwrap();
    let error = storage.write("target/occupied", &[1]).await.unwrap_err();
    assert_eq!(error.name(), "StorageError");
    assert_eq!(
        error.to_string(),
        "Failed to write 'target/occupied' to local storage"
    );
    let mut entries = tokio::fs::read_dir(&dir).await.unwrap();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        assert!(!entry.file_name().to_string_lossy().contains(".__strands_tmp"));
    }
}

#[tokio::test]
async fn rejects_keys_containing_backslashes() {
    let (_dir, _, storage) = setup();
    let error = storage.write("a\\b", &[1]).await.unwrap_err();
    assert_eq!(error.name(), "StorageError");
}

#[tokio::test]
async fn rejects_backslash_path_traversal() {
    let (_dir, _, storage) = setup();
    let error = storage.read("..\\..\\etc\\passwd").await.unwrap_err();
    assert_eq!(error.name(), "StorageError");
}

#[tokio::test]
async fn removes_an_existing_key() {
    let (_dir, _, storage) = setup();
    storage.write("key", &[1]).await.unwrap();
    storage.delete("key").await.unwrap();
    assert_eq!(storage.read("key").await.unwrap(), None);
}

#[tokio::test]
async fn is_a_no_op_for_missing_keys() {
    let (_dir, _, storage) = setup();
    storage.delete("missing").await.unwrap();
}

#[tokio::test]
async fn lists_keys_under_a_prefix() {
    let (_dir, _, storage) = setup();
    storage.write("a/1", &[1]).await.unwrap();
    storage.write("a/2", &[2]).await.unwrap();
    storage.write("b/1", &[3]).await.unwrap();
    assert_eq!(storage.list("a/").await.unwrap(), ["a/1", "a/2"]);
}

#[tokio::test]
async fn returns_all_keys_for_empty_prefix() {
    let (_dir, _, storage) = setup();
    storage.write("a/1", &[1]).await.unwrap();
    storage.write("b", &[2]).await.unwrap();
    assert_eq!(storage.list("").await.unwrap(), ["a/1", "b"]);
}

#[tokio::test]
async fn returns_empty_array_when_base_directory_does_not_exist() {
    let (_dir, base_dir, _) = setup();
    let fresh = LocalFileStorage::new(format!("{base_dir}/nonexistent"));
    assert!(fresh.list("").await.unwrap().is_empty());
}

#[tokio::test]
async fn excludes_scratch_files() {
    let (_dir, base_dir, storage) = setup();
    storage.write("real", &[1]).await.unwrap();
    tokio::fs::write(format!("{base_dir}/leftover.__strands_tmp"), b"garbage")
        .await
        .unwrap();
    assert_eq!(storage.list("").await.unwrap(), ["real"]);
}

#[tokio::test]
async fn does_not_exclude_user_tmp_files() {
    let (_dir, _, storage) = setup();
    storage.write("notes.tmp", &[1]).await.unwrap();
    assert_eq!(storage.list("").await.unwrap(), ["notes.tmp"]);
}

#[tokio::test]
async fn returns_keys_sorted_lexicographically() {
    let (_dir, _, storage) = setup();
    for key in ["c", "a", "b"] {
        storage.write(key, &[1]).await.unwrap();
    }
    assert_eq!(storage.list("").await.unwrap(), ["a", "b", "c"]);
}

#[tokio::test]
async fn returns_a_local_file_storage_instance_rooted_at_the_subdirectory() {
    let (_dir, _, storage) = setup();
    let namespaced = storage.with_namespace("sub/dir").unwrap();
    namespaced.write("file.txt", &[1, 2, 3]).await.unwrap();
    assert!(namespaced.is_namespaced());
    assert_eq!(namespaced.read("file.txt").await.unwrap(), Some(vec![1, 2, 3]));
    assert_eq!(namespaced.list("").await.unwrap(), ["file.txt"]);
}

#[tokio::test]
async fn isolates_keys_from_the_parent_storage() {
    let (_dir, _, storage) = setup();
    let namespaced = storage.with_namespace("isolated").unwrap();
    storage.write("root.txt", &[1]).await.unwrap();
    namespaced.write("child.txt", &[2]).await.unwrap();
    let parent_keys = storage.list("").await.unwrap();
    assert!(parent_keys.contains(&"root.txt".to_owned()));
    assert!(!parent_keys.contains(&"child.txt".to_owned()));
    assert_eq!(namespaced.list("").await.unwrap(), ["child.txt"]);
}

#[test]
fn exposes_base_dir_pointing_to_the_subdirectory() {
    let (_dir, base_dir, storage) = setup();
    assert_eq!(
        storage.with_namespace("nested").unwrap().base_dir(),
        format!("{base_dir}/nested")
    );
}

#[tokio::test]
async fn indexes_writes_and_delegates_search_to_a_search_strategy_from_config() {
    let (_dir, base_dir, _) = setup();
    let strategy = Arc::new(FakeStrategy::default());
    let configured = LocalFileStorage::with_config(
        base_dir.clone(),
        LocalFileStorageConfig {
            search_strategy: Some(strategy.clone()),
            ..Default::default()
        },
    );
    configured.write("a.txt", &[1]).await.unwrap();
    assert_eq!(
        *strategy.indexed.lock().unwrap(),
        [format!("{base_dir} a.txt [1]")]
    );
    assert_eq!(
        configured.search("query").await.unwrap().unwrap(),
        [StorageSearchResult::new("hit.txt", 1.0)]
    );
    assert_eq!(*strategy.searched.lock().unwrap(), [format!("{base_dir} query")]);
}

#[tokio::test]
async fn routes_i_o_through_a_sandbox_from_config() {
    let (_dir, base_dir, _) = setup();
    let sandbox = RecordingSandbox::new(false);
    let configured = LocalFileStorage::with_config(
        base_dir.clone(),
        LocalFileStorageConfig {
            sandbox: Some(sandbox.clone()),
            ..Default::default()
        },
    );
    configured.write("a.txt", &[1]).await.unwrap();
    assert_eq!(configured.read("a.txt").await.unwrap(), Some(vec![1]));
    assert_eq!(configured.list("").await.unwrap(), ["a.txt"]);
    configured.delete("a.txt").await.unwrap();
    assert_eq!(
        *sandbox.calls.lock().unwrap(),
        [
            format!("write {base_dir}/a.txt [1]"),
            format!("read {base_dir}/a.txt"),
            format!("list {base_dir}"),
            format!("remove {base_dir}/a.txt"),
        ]
    );
}

#[tokio::test]
async fn treats_sandbox_path_not_found_error_from_the_sandbox_as_a_missing_key() {
    let (_dir, base_dir, _) = setup();
    let sandboxed = LocalFileStorage::new(base_dir).for_sandbox(RecordingSandbox::new(true));
    assert_eq!(sandboxed.read("missing.txt").await.unwrap(), None);
    assert!(sandboxed.list("no-such-dir/").await.unwrap().is_empty());
}

#[tokio::test]
async fn carries_a_search_strategy_from_config_through_for_sandbox_and_namespace() {
    let (_dir, base_dir, _) = setup();
    let strategy = Arc::new(FakeStrategy::default());
    let configured = LocalFileStorage::with_config(
        base_dir.clone(),
        LocalFileStorageConfig {
            search_strategy: Some(strategy.clone()),
            ..Default::default()
        },
    );
    configured
        .for_sandbox(RecordingSandbox::new(false))
        .search("one")
        .await
        .unwrap()
        .unwrap();
    configured
        .with_namespace("sub")
        .unwrap()
        .search("two")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        *strategy.searched.lock().unwrap(),
        [format!("{base_dir} one"), format!("{base_dir}/sub two")]
    );
}

#[test]
fn the_base_dir_defaults_to_dot_strands_and_collapses_slashes() {
    assert_eq!(LocalFileStorage::default().base_dir(), "./.strands");
    assert_eq!(LocalFileStorage::new("/tmp//a///b/").base_dir(), "/tmp/a/b");
    assert_eq!(LocalFileStorage::new("/").base_dir(), "/");
}

// No TS case: a dropped future is Rust-only; the temp write and the rename are one blocking task.
#[test]
fn a_write_dropped_after_its_first_poll_finishes_and_leaves_no_temp_file() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let (_dir, base_dir, storage) = setup();
        {
            let mut write = Box::pin(storage.write("session/key.json", b"data"));
            assert!(futures::poll!(write.as_mut()).is_pending());
        }
        // The only blocking thread runs this after the dropped write's task.
        tokio::task::spawn_blocking(|| ()).await.unwrap();
        let names: Vec<String> = std::fs::read_dir(format!("{base_dir}/session"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["key.json"]);
        assert_eq!(
            storage.read("session/key.json").await.unwrap(),
            Some(b"data".to_vec())
        );
    });
}
