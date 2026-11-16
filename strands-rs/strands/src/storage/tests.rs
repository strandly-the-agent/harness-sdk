//! Ported from strands-ts/src/storage/__tests__/namespaced-storage.test.ts

use std::sync::Arc;

use super::{InMemoryStorage, Storage, namespace};

fn setup() -> (InMemoryStorage, Arc<dyn Storage>) {
    let backend = InMemoryStorage::new();
    let namespaced = namespace(Arc::new(backend.clone()), "prefix").unwrap();
    (backend, namespaced)
}

#[tokio::test]
async fn prepends_namespace_to_write_keys() {
    let (backend, namespaced) = setup();
    namespaced.write("key", &[1, 2, 3]).await.unwrap();
    assert_eq!(backend.read("prefix/key").await.unwrap(), Some(vec![1, 2, 3]));
}

#[tokio::test]
async fn prepends_namespace_to_read_keys() {
    let (backend, namespaced) = setup();
    backend.write("prefix/key", &[4, 5]).await.unwrap();
    assert_eq!(namespaced.read("key").await.unwrap(), Some(vec![4, 5]));
}

#[tokio::test]
async fn returns_null_for_missing_keys() {
    let (_, namespaced) = setup();
    assert_eq!(namespaced.read("nonexistent").await.unwrap(), None);
}

#[tokio::test]
async fn prepends_namespace_to_delete_keys() {
    let (backend, namespaced) = setup();
    backend.write("prefix/key", &[1]).await.unwrap();
    namespaced.delete("key").await.unwrap();
    assert_eq!(backend.read("prefix/key").await.unwrap(), None);
}

#[tokio::test]
async fn lists_keys_with_namespace_stripped() {
    let (backend, namespaced) = setup();
    backend.write("prefix/a", &[1]).await.unwrap();
    backend.write("prefix/b", &[2]).await.unwrap();
    backend.write("other/c", &[3]).await.unwrap();
    assert_eq!(namespaced.list("").await.unwrap(), ["a", "b"]);
}

#[tokio::test]
async fn lists_keys_with_sub_prefix() {
    let (backend, namespaced) = setup();
    backend.write("prefix/session/abc", &[1]).await.unwrap();
    backend.write("prefix/session/def", &[2]).await.unwrap();
    backend.write("prefix/offloader/xyz", &[3]).await.unwrap();
    assert_eq!(
        namespaced.list("session/").await.unwrap(),
        ["session/abc", "session/def"]
    );
}

#[tokio::test]
async fn composes_nested_namespaces() {
    let backend = InMemoryStorage::new();
    let outer = namespace(Arc::new(backend.clone()), "prefix").unwrap();
    let nested = namespace(outer, "sub").unwrap();
    nested.write("key", &[9]).await.unwrap();
    assert_eq!(backend.read("prefix/sub/key").await.unwrap(), Some(vec![9]));
}

#[tokio::test]
async fn handles_empty_namespace_as_no_op_prefix() {
    let backend = InMemoryStorage::new();
    let empty = namespace(Arc::new(backend.clone()), "").unwrap();
    empty.write("key", &[7]).await.unwrap();
    assert_eq!(backend.read("key").await.unwrap(), Some(vec![7]));
}

#[test]
fn sets_namespaced_symbol_on_returned_view() {
    let (_, namespaced) = setup();
    assert!(namespaced.is_namespaced());
}

#[test]
fn does_not_have_namespaced_symbol_on_raw_storage() {
    let (backend, _) = setup();
    assert!(!backend.is_namespaced());
}

// No TS case: pins the logged divergence from the TS trailing-slash prefix bug (docs/DIVERGENCES.md).
#[tokio::test]
async fn a_trailing_slash_prefix_lists_whole_keys() {
    let backend = InMemoryStorage::new();
    let namespaced = namespace(Arc::new(backend.clone()), "prefix/").unwrap();
    namespaced.write("key", &[1]).await.unwrap();
    assert_eq!(namespaced.list("").await.unwrap(), ["key"]);
}

// No TS case: pins that a generic view keeps the ephemeral marker and searches only its prefix.
#[tokio::test]
async fn the_view_keeps_the_ephemeral_marker_and_filters_search() {
    let (backend, namespaced) = setup();
    backend.write("prefix/theme.md", b"dark mode").await.unwrap();
    backend.write("other/theme.md", b"dark mode").await.unwrap();
    assert!(namespaced.is_ephemeral());
    let results = namespaced.search("dark").await.unwrap().unwrap();
    let keys: Vec<&str> = results.iter().map(|result| result.key.as_str()).collect();
    assert_eq!(keys, ["theme.md"]);
}

// No TS case: pins `normalize_key` and `normalize_prefix` directly, with the TS error texts.
#[test]
fn keys_are_normalized_with_the_ts_error_texts() {
    use super::{normalize_key, normalize_prefix};
    assert_eq!(normalize_key("/a//b/").unwrap(), "a/b");
    assert_eq!(normalize_prefix("/a//b/").unwrap(), "a/b/");
    assert_eq!(normalize_prefix("").unwrap(), "");
    let text = |error: crate::Error| (error.name(), error.to_string());
    assert_eq!(
        text(normalize_key("").unwrap_err()),
        ("StorageError", "Storage key must not be empty".into())
    );
    assert_eq!(
        text(normalize_key("a\\b").unwrap_err()).1,
        "Invalid storage key 'a\\b': backslashes are not allowed"
    );
    assert_eq!(
        text(normalize_key("a/../b").unwrap_err()).1,
        "Invalid storage key 'a/../b': '..' path segments are not allowed"
    );
    assert_eq!(
        text(normalize_prefix("../").unwrap_err()).1,
        "Invalid storage prefix '../': '..' path segments are not allowed"
    );
    assert_eq!(
        text(normalize_prefix("a\\").unwrap_err()).1,
        "Invalid storage prefix 'a\\': backslashes are not allowed"
    );
}
