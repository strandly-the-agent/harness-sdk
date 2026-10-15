//! Rust snippets for `memory/bedrock-knowledge-base.mdx`.

use strands_agents::Result;

async fn read_only() -> Result<()> {
    // --8<-- [start:read_only]
    use std::sync::Arc;

    use strands_agents::memory::{
        BedrockKnowledgeBaseConfig, BedrockKnowledgeBaseStore,
        BedrockKnowledgeBaseStoreConfig, MemoryManager,
    };
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    let store = BedrockKnowledgeBaseStore::with_config(BedrockKnowledgeBaseStoreConfig {
        name: "docs".to_string(),
        description: Some("Company documentation and policies.".to_string()),
        config: BedrockKnowledgeBaseConfig {
            knowledge_base_id: "KB123".to_string(),
            ..Default::default()
        },
        ..Default::default()
    })
    .await?;

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .memory_manager(MemoryManager::new(vec![Arc::new(store)])?)
        .build()?;
    // --8<-- [end:read_only]
    let _ = agent;
    Ok(())
}

async fn writable_custom() -> Result<()> {
    // --8<-- [start:writable_custom]
    use strands_agents::memory::{
        BedrockKnowledgeBaseConfig, BedrockKnowledgeBaseStore,
        BedrockKnowledgeBaseStoreConfig, DataSourceType,
    };

    let store = BedrockKnowledgeBaseStore::with_config(BedrockKnowledgeBaseStoreConfig {
        name: "preferences".to_string(),
        description: Some("User preferences and stable facts.".to_string()),
        writable: Some(true),
        config: BedrockKnowledgeBaseConfig {
            knowledge_base_id: "KB123".to_string(),
            data_source_type: Some(DataSourceType::Custom),
            data_source_id: Some("DS456".to_string()),
            ..Default::default()
        },
        ..Default::default()
    })
    .await?;
    // --8<-- [end:writable_custom]
    let _ = store;
    Ok(())
}

async fn scoped_stores() -> Result<()> {
    // --8<-- [start:scoped_stores]
    use strands_agents::memory::{
        BedrockKnowledgeBaseConfig, BedrockKnowledgeBaseStore,
        BedrockKnowledgeBaseStoreConfig, DataSourceType,
    };

    // Build the connection once, vary only name and scope per store.
    let connection = BedrockKnowledgeBaseConfig {
        knowledge_base_id: "KB123".to_string(),
        data_source_type: Some(DataSourceType::Custom),
        data_source_id: Some("DS456".to_string()),
        ..Default::default()
    };

    let alice = BedrockKnowledgeBaseStore::with_config(BedrockKnowledgeBaseStoreConfig {
        name: "alice".to_string(),
        writable: Some(true),
        scope: Some("user-alice".to_string()),
        config: connection.clone(),
        ..Default::default()
    })
    .await?;

    let bob = BedrockKnowledgeBaseStore::with_config(BedrockKnowledgeBaseStoreConfig {
        name: "bob".to_string(),
        writable: Some(true),
        scope: Some("user-bob".to_string()),
        config: connection,
        ..Default::default()
    })
    .await?;
    // --8<-- [end:scoped_stores]
    let _ = (alice, bob);
    Ok(())
}

async fn s3_store() -> Result<()> {
    // --8<-- [start:s3_store]
    use strands_agents::memory::{
        BedrockKnowledgeBaseConfig, BedrockKnowledgeBaseS3Config,
        BedrockKnowledgeBaseStore, BedrockKnowledgeBaseStoreConfig, DataSourceType,
    };

    let store = BedrockKnowledgeBaseStore::with_config(BedrockKnowledgeBaseStoreConfig {
        name: "preferences".to_string(),
        writable: Some(true),
        config: BedrockKnowledgeBaseConfig {
            knowledge_base_id: "KB123".to_string(),
            data_source_type: Some(DataSourceType::S3),
            data_source_id: Some("DS789".to_string()),
            s3: Some(BedrockKnowledgeBaseS3Config {
                bucket: "my-agent-memories".to_string(),
                prefix: Some("memories/".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    })
    .await?;
    // --8<-- [end:s3_store]
    let _ = store;
    Ok(())
}

async fn acl_store() -> Result<()> {
    // --8<-- [start:acl_store]
    use strands_agents::memory::{
        AccessControlAccess, AccessControlEntry, BedrockKnowledgeBaseConfig,
        BedrockKnowledgeBaseStore, BedrockKnowledgeBaseStoreConfig, DataSourceType,
    };

    let store = BedrockKnowledgeBaseStore::with_config(BedrockKnowledgeBaseStoreConfig {
        name: "policies".to_string(),
        writable: Some(true),
        scope: Some("hr".to_string()),
        access_control_list: Some(vec![AccessControlEntry {
            access: AccessControlAccess::Allow,
            name: "alice@example.com".to_string(),
            r#type: "USER".to_string(),
        }]),
        config: BedrockKnowledgeBaseConfig {
            knowledge_base_id: "KB123".to_string(),
            data_source_type: Some(DataSourceType::Custom),
            data_source_id: Some("DS456".to_string()),
            ..Default::default()
        },
        ..Default::default()
    })
    .await?;
    // --8<-- [end:acl_store]
    let _ = store;
    Ok(())
}

async fn search_and_add() -> Result<()> {
    // --8<-- [start:search]
    use serde_json::json;
    use strands_agents::memory::{
        BedrockKnowledgeBaseConfig, BedrockKnowledgeBaseStore,
        BedrockKnowledgeBaseStoreConfig, DataSourceType, MemoryStore, Metadata,
        SearchOptions,
    };

    let store = BedrockKnowledgeBaseStore::with_config(BedrockKnowledgeBaseStoreConfig {
        name: "preferences".to_string(),
        writable: Some(true),
        config: BedrockKnowledgeBaseConfig {
            knowledge_base_id: "KB123".to_string(),
            data_source_type: Some(DataSourceType::Custom),
            data_source_id: Some("DS456".to_string()),
            ..Default::default()
        },
        ..Default::default()
    })
    .await?;

    let results = store
        .search(
            "what are my preferences?",
            &SearchOptions { max_search_results: Some(5) },
        )
        .await?;
    for entry in &results {
        let score = entry.metadata.as_ref().and_then(|m| m.get("_relevanceScore"));
        println!("{} {score:?}", entry.content);
    }

    // add_document returns the new document's id (a UUID for CUSTOM, an s3:// URI
    // for S3); the trait's `add` discards it.
    let metadata = Metadata::from([("category".to_string(), json!("travel"))]);
    let document_id =
        store.add_document("User prefers aisle seats", Some(&metadata)).await?;
    // --8<-- [end:search]
    let _ = document_id;
    Ok(())
}

async fn extraction() -> Result<()> {
    // --8<-- [start:extraction]
    use std::sync::Arc;

    use strands_agents::memory::{
        BedrockKnowledgeBaseConfig, BedrockKnowledgeBaseStore,
        BedrockKnowledgeBaseStoreConfig, DataSourceType, ExtractionConfig,
        ModelExtractor,
    };
    use strands_agents::models::BedrockModel;

    // An add-only store extracts client-side, so it needs an explicit extractor.
    let extractor = ModelExtractor::new(Arc::new(BedrockModel::default_model().await?));

    let store = BedrockKnowledgeBaseStore::with_config(BedrockKnowledgeBaseStoreConfig {
        name: "preferences".to_string(),
        writable: Some(true),
        extraction: Some(ExtractionConfig::default().extractor(Arc::new(extractor))),
        config: BedrockKnowledgeBaseConfig {
            knowledge_base_id: "KB123".to_string(),
            data_source_type: Some(DataSourceType::Custom),
            data_source_id: Some("DS456".to_string()),
            ..Default::default()
        },
        ..Default::default()
    })
    .await?;
    // --8<-- [end:extraction]
    let _ = store;
    Ok(())
}
