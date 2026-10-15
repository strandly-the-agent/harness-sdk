//! Rust snippets for `memory/managing-memory.mdx`.

use strands_agents::memory::MemoryManager;
use strands_agents::Result;

/// Stand-in for the reader's own backend in the custom-store snippets.
mod my_backend {
    use strands_agents::memory::MemoryEntry;
    use strands_agents::types::Message;
    use strands_agents::Result;

    pub async fn retrieve(
        _query: &str,
        _limit: Option<usize>,
    ) -> Result<Vec<MemoryEntry>> {
        Ok(Vec::new())
    }

    pub async fn ingest_conversation(_messages: &[Message]) -> Result<()> {
        Ok(())
    }
}

async fn turn_on_writes() -> Result<()> {
    // --8<-- [start:turn_on_writes]
    use std::sync::Arc;

    use strands_agents::memory::{
        BedrockKnowledgeBaseConfig, BedrockKnowledgeBaseStore,
        BedrockKnowledgeBaseStoreConfig, DataSourceType, ExtractionConfig, MemoryManager,
        ModelExtractor,
    };
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    let extractor = ModelExtractor::new(Arc::new(BedrockModel::default_model().await?));

    let store = BedrockKnowledgeBaseStore::with_config(BedrockKnowledgeBaseStoreConfig {
        name: "preferences".to_string(),
        writable: Some(true),
        // capture memories from the conversation, every 5 turns
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

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        // with_add_tool: let the agent save memories itself
        .memory_manager(MemoryManager::with_add_tool(vec![Arc::new(store)])?)
        .build()?;
    // --8<-- [end:turn_on_writes]
    let _ = agent;
    Ok(())
}

async fn multi_store() -> Result<()> {
    // --8<-- [start:multi_store]
    use std::sync::Arc;

    use strands_agents::memory::{
        BedrockKnowledgeBaseConfig, BedrockKnowledgeBaseStore,
        BedrockKnowledgeBaseStoreConfig, DataSourceType, MemoryManager,
    };
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    // Build the connection once, vary only name and scope per store.
    let connection = BedrockKnowledgeBaseConfig {
        knowledge_base_id: "KB123".to_string(),
        data_source_type: Some(DataSourceType::Custom),
        data_source_id: Some("DS456".to_string()),
        ..Default::default()
    };

    let personal =
        BedrockKnowledgeBaseStore::with_config(BedrockKnowledgeBaseStoreConfig {
            name: "personal".to_string(),
            description: Some("Knowledge specific to this user.".to_string()),
            writable: Some(true),
            scope: Some("user-abc".to_string()),
            config: connection.clone(),
            ..Default::default()
        })
        .await?;

    let team = BedrockKnowledgeBaseStore::with_config(BedrockKnowledgeBaseStoreConfig {
        name: "team".to_string(),
        description: Some("Shared team knowledge.".to_string()),
        scope: Some("team-xyz".to_string()),
        config: connection,
        ..Default::default()
    })
    .await?;

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .memory_manager(MemoryManager::new(vec![Arc::new(personal), Arc::new(team)])?)
        .build()?;
    // --8<-- [end:multi_store]
    let _ = agent;
    Ok(())
}

async fn search_tool_config() -> Result<()> {
    // --8<-- [start:search_tool_config]
    use std::sync::Arc;

    use strands_agents::memory::{
        BedrockKnowledgeBaseConfig, BedrockKnowledgeBaseStore,
        BedrockKnowledgeBaseStoreConfig, DataSourceType, MemoryAddToolConfig,
        MemoryInjectionConfig, MemoryManager, MemoryManagerConfig, MemoryToolConfig,
    };
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

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

    let manager = MemoryManager::with_config(MemoryManagerConfig {
        stores: vec![Arc::new(store)],
        search_tool: Some(MemoryToolConfig {
            name: Some("recall".to_string()),
            description: Some("Look up what you remember about the user.".to_string()),
        }),
        // add_memory: opt in, and return as soon as writes dispatch instead of
        // awaiting them
        add_tool: Some(MemoryAddToolConfig {
            wait_for_writes: Some(false),
            ..Default::default()
        }),
        // every field is explicit here, so re-enable injection alongside the tools
        injection: Some(MemoryInjectionConfig::default()),
        ..Default::default()
    })?;

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .memory_manager(manager)
        .build()?;
    // --8<-- [end:search_tool_config]
    let _ = agent;
    Ok(())
}

async fn injection_custom() -> Result<()> {
    // --8<-- [start:injection_custom]
    use std::sync::Arc;

    use strands_agents::memory::{
        BedrockKnowledgeBaseConfig, BedrockKnowledgeBaseStore,
        BedrockKnowledgeBaseStoreConfig, InjectionTrigger, MemoryEntry,
        MemoryInjectionConfig, MemoryManager, MemoryManagerConfig, MemoryToolConfig,
    };
    use strands_agents::models::BedrockModel;
    use strands_agents::types::Message;
    use strands_agents::Agent;

    let store = BedrockKnowledgeBaseStore::with_config(BedrockKnowledgeBaseStoreConfig {
        name: "preferences".to_string(),
        config: BedrockKnowledgeBaseConfig {
            knowledge_base_id: "KB123".to_string(),
            ..Default::default()
        },
        ..Default::default()
    })
    .await?;

    let manager = MemoryManager::with_config(MemoryManagerConfig {
        stores: vec![Arc::new(store)],
        search_tool: Some(MemoryToolConfig::default()),
        injection: Some(MemoryInjectionConfig {
            // UserTurn (default) or EveryTurn; predicates are Python and TypeScript only
            trigger: InjectionTrigger::EveryTurn,
            max_entries: Some(3),
            query: Some(Arc::new(|messages: &[Message]| {
                messages.last().map(|message| message.text())
            })),
            format: Some(Arc::new(|entries: &[MemoryEntry]| {
                entries
                    .iter()
                    .map(|entry| format!("- {}", entry.content))
                    .collect::<Vec<_>>()
                    .join("\n")
            })),
        }),
        ..Default::default()
    })?;

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .memory_manager(manager)
        .build()?;
    // --8<-- [end:injection_custom]
    let _ = agent;
    Ok(())
}

async fn extraction_defaults() -> Result<()> {
    // --8<-- [start:extraction_defaults]
    use std::sync::Arc;

    use strands_agents::memory::{
        BedrockKnowledgeBaseConfig, BedrockKnowledgeBaseStore,
        BedrockKnowledgeBaseStoreConfig, DataSourceType, ExtractionConfig,
        ModelExtractor,
    };
    use strands_agents::models::BedrockModel;

    // An add-only store needs an explicit extractor; the trigger defaults to every
    // 5 turns.
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
    // --8<-- [end:extraction_defaults]
    let _ = store;
    Ok(())
}

async fn extraction_custom() -> Result<()> {
    // --8<-- [start:extraction_custom]
    use std::sync::Arc;

    use strands_agents::memory::{
        BedrockKnowledgeBaseConfig, BedrockKnowledgeBaseStore,
        BedrockKnowledgeBaseStoreConfig, DataSourceType, ExtractionConfig,
        ExtractionTrigger, MemoryManager, MemoryManagerConfig, MemoryToolConfig,
        ModelExtractor,
    };
    use strands_agents::models::BedrockModel;

    let store = BedrockKnowledgeBaseStore::with_config(BedrockKnowledgeBaseStoreConfig {
        name: "preferences".to_string(),
        writable: Some(true),
        extraction: Some(
            ExtractionConfig::default().extractor(Arc::new(
                // a cheaper model than the agent's to cut cost
                ModelExtractor::new(Arc::new(BedrockModel::default_model().await?))
                    .system_prompt("Extract durable user preferences as discrete facts."),
            )),
        ),
        config: BedrockKnowledgeBaseConfig {
            knowledge_base_id: "KB123".to_string(),
            data_source_type: Some(DataSourceType::Custom),
            data_source_id: Some("DS456".to_string()),
            ..Default::default()
        },
        ..Default::default()
    })
    .await?;

    // Rust carries the trigger on the manager, not the store.
    let manager = MemoryManager::with_config(MemoryManagerConfig {
        stores: vec![Arc::new(store)],
        search_tool: Some(MemoryToolConfig::default()),
        // after every turn, not every 5
        extraction_trigger: Some(ExtractionTrigger::Invocation),
        ..Default::default()
    })?;
    // --8<-- [end:extraction_custom]
    let _ = manager;
    Ok(())
}

async fn programmatic(memory_manager: &MemoryManager) -> Result<()> {
    // --8<-- [start:programmatic]
    use serde_json::json;
    use strands_agents::memory::{MemoryAddOptions, MemorySearchOptions, Metadata};

    // Search every store, or a subset by name.
    let all =
        memory_manager.search("travel plans", &MemorySearchOptions::default()).await?;
    let scoped = memory_manager
        .search(
            "travel plans",
            &MemorySearchOptions {
                stores: Some(vec!["personal".to_string()]),
                max_search_results: Some(5),
            },
        )
        .await?;

    // Write to writable stores, with metadata.
    memory_manager
        .add(
            "Prefers aisle seats",
            &MemoryAddOptions {
                stores: Some(vec!["personal".to_string()]),
                metadata: Some(Metadata::from([(
                    "category".to_string(),
                    json!("travel"),
                )])),
            },
        )
        .await?;
    // --8<-- [end:programmatic]
    let _ = (all, scoped);
    Ok(())
}

async fn flush(memory_manager: &MemoryManager) -> Result<()> {
    // --8<-- [start:flush]
    // At a shutdown boundary you control, before the process exits.
    memory_manager.flush().await?;
    // --8<-- [end:flush]
    Ok(())
}

fn custom_store() {
    // --8<-- [start:custom_store]
    use std::sync::Mutex;

    use async_trait::async_trait;
    use strands_agents::memory::{MemoryEntry, MemoryStore, Metadata, SearchOptions};
    use strands_agents::Result;

    #[derive(Default)]
    struct InMemoryStore {
        entries: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl MemoryStore for InMemoryStore {
        fn name(&self) -> &str {
            "preferences"
        }

        fn writable(&self) -> bool {
            true
        }

        async fn search(
            &self,
            query: &str,
            options: &SearchOptions,
        ) -> Result<Vec<MemoryEntry>> {
            let limit = options.max_search_results.unwrap_or(3);
            let entries = self.entries.lock().unwrap();
            Ok(entries
                .iter()
                .filter(|content| content.contains(query))
                .take(limit)
                .map(|content| MemoryEntry::new(content.as_str()))
                .collect())
        }

        // Opt into the optional `add` capability, otherwise the manager will not
        // route to it.
        fn supports_add(&self) -> bool {
            true
        }

        async fn add(&self, content: &str, _metadata: Option<&Metadata>) -> Result<()> {
            self.entries.lock().unwrap().push(content.to_string());
            Ok(())
        }
    }
    // --8<-- [end:custom_store]
}

fn server_side_store() {
    // --8<-- [start:server_side_store]
    use async_trait::async_trait;
    use strands_agents::memory::{
        AddMessagesContext, ExtractionConfig, MemoryEntry, MemoryStore, SearchOptions,
    };
    use strands_agents::types::Message;
    use strands_agents::Result;

    struct ServerSideStore {
        // Extract every 5 turns; no extractor, so the manager calls add_messages.
        extraction: ExtractionConfig,
    }

    #[async_trait]
    impl MemoryStore for ServerSideStore {
        fn name(&self) -> &str {
            "preferences"
        }

        fn writable(&self) -> bool {
            true
        }

        fn extraction(&self) -> Option<&ExtractionConfig> {
            Some(&self.extraction)
        }

        async fn search(
            &self,
            query: &str,
            options: &SearchOptions,
        ) -> Result<Vec<MemoryEntry>> {
            my_backend::retrieve(query, options.max_search_results).await
        }

        fn supports_add_messages(&self) -> bool {
            true
        }

        // The manager hands the raw message batch here; the backend extracts server-side.
        async fn add_messages(
            &self,
            messages: &[Message],
            _context: &AddMessagesContext,
        ) -> Result<()> {
            my_backend::ingest_conversation(messages).await
        }
    }
    // --8<-- [end:server_side_store]
}
