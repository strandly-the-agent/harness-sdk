//! Live S3 round-trips for `S3Storage` and `S3SessionStorage`.
//!
//! Ignored by default. Run with a writable bucket:
//! `STRANDS_TEST_S3_BUCKET=<bucket> AWS_REGION=us-west-2 \
//!   cargo test -p strands-agents --all-features --test integ_s3 -- --ignored`

#![cfg(feature = "s3")]

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map};
use strands_agents::session::{Scope, Snapshot, SnapshotLocation, SnapshotManifest, SnapshotStorage};
use strands_agents::{S3SessionStorage, S3Storage, Storage};

/// The bucket under test, or `None` with a skip message when unset.
fn bucket() -> Option<String> {
    match std::env::var("STRANDS_TEST_S3_BUCKET") {
        Ok(bucket) if !bucket.is_empty() => Some(bucket),
        _ => {
            eprintln!("skipping: set STRANDS_TEST_S3_BUCKET to a writable bucket to run this test");
            None
        }
    }
}

/// A prefix unique to this test run, so concurrent runs never collide.
fn unique_prefix(label: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!("strands-rs-integ/{label}-{nanos}")
}

#[tokio::test]
#[ignore = "live S3"]
async fn s3_storage_round_trips_keys() {
    let Some(bucket) = bucket() else { return };
    let prefix = unique_prefix("storage");
    let storage = S3Storage::builder(&bucket)
        .prefix(&prefix)
        .build()
        .await
        .expect("builds");

    storage
        .write("a/one.json", b"{\"n\":1}")
        .await
        .expect("write one");
    storage
        .write("a/two.json", b"{\"n\":2}")
        .await
        .expect("write two");

    assert_eq!(
        storage.read("a/one.json").await.expect("read"),
        Some(b"{\"n\":1}".to_vec())
    );
    assert_eq!(storage.read("a/missing.json").await.expect("read"), None);

    let mut keys = storage.list("a/").await.expect("list");
    keys.sort();
    assert_eq!(keys, vec!["a/one.json".to_owned(), "a/two.json".to_owned()]);

    storage.delete("a/one.json").await.expect("delete");
    assert_eq!(storage.read("a/one.json").await.expect("read"), None);

    // cleanup
    storage.delete("a/two.json").await.expect("delete");
    assert!(storage.list("").await.expect("list").is_empty());
}

#[tokio::test]
#[ignore = "live S3"]
async fn s3_session_storage_round_trips_snapshots() {
    let Some(bucket) = bucket() else { return };
    let prefix = unique_prefix("session");
    let storage = S3SessionStorage::builder(&bucket)
        .prefix(&prefix)
        .build()
        .await
        .expect("builds");

    let session_id = "integ-session";
    let location = SnapshotLocation::agent(session_id, "integ-agent");
    let ids = [
        "019c9bf1-14e5-7eef-96fb-cc07ae54210f",
        "019c9bf1-1d34-7eef-96fb-d1be20fd7bbd",
    ];
    let snapshot = |n: u8| Snapshot {
        scope: Scope::Agent,
        schema_version: "1.0".to_owned(),
        created_at: "2024-01-01T00:00:00.000Z".to_owned(),
        data: json!({ "n": n }).as_object().expect("object").clone(),
        app_data: Map::new(),
    };

    assert_eq!(storage.load_snapshot(&location, None).await.expect("load"), None);
    assert_eq!(
        storage
            .list_snapshot_ids(&location, None, None)
            .await
            .expect("list"),
        Vec::<String>::new()
    );

    storage
        .save_snapshot(&location, ids[0], true, &snapshot(0))
        .await
        .expect("save latest");
    storage
        .save_snapshot(&location, ids[0], false, &snapshot(1))
        .await
        .expect("save history 0");
    storage
        .save_snapshot(&location, ids[1], false, &snapshot(2))
        .await
        .expect("save history 1");

    assert_eq!(
        storage.load_snapshot(&location, None).await.expect("load latest"),
        Some(snapshot(0))
    );
    assert_eq!(
        storage
            .load_snapshot(&location, Some(ids[1]))
            .await
            .expect("load by id"),
        Some(snapshot(2))
    );
    assert_eq!(
        storage
            .list_snapshot_ids(&location, None, None)
            .await
            .expect("list"),
        vec![ids[0].to_owned(), ids[1].to_owned()]
    );
    assert_eq!(
        storage
            .list_snapshot_ids(&location, Some(1), Some(ids[0]))
            .await
            .expect("list paged"),
        vec![ids[1].to_owned()]
    );

    let manifest = storage.load_manifest(&location).await.expect("default manifest");
    assert_eq!(manifest.schema_version, "1.0");
    storage
        .save_manifest(
            &location,
            &SnapshotManifest {
                schema_version: "1.0".to_owned(),
                updated_at: "2024-01-01T00:00:00.000Z".to_owned(),
            },
        )
        .await
        .expect("save manifest");
    assert_eq!(
        storage
            .load_manifest(&location)
            .await
            .expect("load manifest")
            .updated_at,
        "2024-01-01T00:00:00.000Z"
    );

    storage.delete_session(session_id).await.expect("delete session");
    assert_eq!(storage.load_snapshot(&location, None).await.expect("load"), None);
    assert_eq!(
        storage
            .list_snapshot_ids(&location, None, None)
            .await
            .expect("list"),
        Vec::<String>::new()
    );
}
