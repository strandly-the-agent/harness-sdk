//! Live Bedrock Knowledge Base memory-store test (port of the `BedrockKnowledgeBaseStore` write +
//! search round trip). It calls the real service and is `#[ignore]`d; it needs a knowledge base
//! that already exists, so it skips with a message when `STRANDS_TEST_KB_ID` is unset:
//!
//! ```text
//! AWS_REGION=us-west-2 STRANDS_TEST_KB_ID=XXXXXXXXXX \
//!   STRANDS_TEST_KB_DATA_SOURCE_ID=YYYYYYYYYY \
//!   cargo test -p strands-agents --all-features --test integ_memory_kb -- --ignored --nocapture
//! ```

#![cfg(feature = "bedrock-kb")]

use std::env;

use strands_agents::memory::{
    BedrockKnowledgeBaseConfig, BedrockKnowledgeBaseStore, BedrockKnowledgeBaseStoreConfig, DataSourceType,
    MemoryStore, SearchOptions,
};

/// Ingests a scoped document and then retrieves it back through `search`.
///
/// A `CUSTOM` data source indexes asynchronously, so the search is not asserted to contain the
/// document just written: the round trip proves both calls reach the service and parse.
#[tokio::test]
#[ignore = "live Bedrock KB"]
async fn ingests_a_document_and_searches_the_knowledge_base() {
    let Ok(knowledge_base_id) = env::var("STRANDS_TEST_KB_ID") else {
        println!("skipping: set STRANDS_TEST_KB_ID (and optionally STRANDS_TEST_KB_DATA_SOURCE_ID) to run");
        return;
    };
    let data_source_id = env::var("STRANDS_TEST_KB_DATA_SOURCE_ID").ok();
    let writable = data_source_id.is_some();

    let store = BedrockKnowledgeBaseStore::new(BedrockKnowledgeBaseStoreConfig {
        name: "live-kb".to_owned(),
        scope: Some("strandly-managed-integ".to_owned()),
        writable: Some(writable),
        config: BedrockKnowledgeBaseConfig {
            knowledge_base_id,
            data_source_id,
            data_source_type: writable.then_some(DataSourceType::Custom),
            region: env::var("AWS_REGION").ok(),
            ..Default::default()
        },
        ..Default::default()
    })
    .await
    .expect("builds a live store");

    store
        .initialize()
        .await
        .expect("resolves the knowledge base type");

    if writable {
        let id = store
            .add_document("Strandly remembers that the live KB round trip ran.", None)
            .await
            .expect("ingests a document");
        println!("ingested document id: {id}");
    } else {
        println!("STRANDS_TEST_KB_DATA_SOURCE_ID unset: search-only run");
    }

    let entries = store
        .search("live KB round trip", &SearchOptions::default())
        .await
        .expect("searches the knowledge base");
    println!("search returned {} entries", entries.len());
}
