//! Every extension trait a user is meant to implement, implemented from outside the crate with
//! public items only, and every crate-root re-export named once. A trait that gains a
//! crate-private type in a signature, or a root name that is dropped, fails this file.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use strands::experimental::context_manager::{ContextState, ContextStrategy};
use strands::hooks::AfterInvocationEvent;
use strands::memory::{
    ExtractionResult, ExtractionTrigger, ExtractionTriggerContext, Extractor, ExtractorContext,
};
use strands::plugins::Plugin;
use strands::retry::{BackoffContext, BackoffStrategy, ModelRetryContext, ModelRetryStrategy, RetryDecision};
use strands::storage::search::{SearchStrategy, StorageSearchResult};
use strands::storage::{InMemoryStorage, Storage};
use strands::testing::MockMessageModel;
use strands::tools::ToolStream;
use strands::types::agent::LocalAgent;
use strands::types::messages::{TextBlock, ToolResultBlock};
use strands::types::tools::{ToolResultStatus, ToolSpec};
use strands::{Agent, Message, Tool, ToolContext, ToolOutcome};

struct Manual;

impl Tool for Manual {
    fn name(&self) -> &str {
        "manual"
    }

    fn description(&self) -> &str {
        "A hand-written tool."
    }

    fn tool_spec(&self) -> ToolSpec {
        ToolSpec::new("manual", "A hand-written tool.")
    }

    fn stream(&self, context: ToolContext) -> ToolStream {
        ToolStream::from_future(async move {
            let id = context.tool_use().tool_use_id().to_owned();
            ToolOutcome::Result(ToolResultBlock::new(
                id,
                ToolResultStatus::Success,
                vec!["done".into()],
            ))
        })
    }
}

struct Mine;

#[strands::async_trait]
impl Plugin for Mine {
    fn name(&self) -> &str {
        "app:mine"
    }

    async fn init_agent(self: Arc<Self>, agent: &mut LocalAgent) -> strands::Result<()> {
        let _ = agent.id();
        Ok(())
    }

    fn get_tools(&self) -> Vec<Arc<dyn Tool>> {
        vec![Arc::new(Manual)]
    }
}

#[derive(Default)]
struct Mem(Mutex<BTreeMap<String, Vec<u8>>>);

#[strands::async_trait]
impl Storage for Mem {
    async fn write(&self, key: &str, data: &[u8]) -> strands::Result<()> {
        self.0.lock().unwrap().insert(key.into(), data.into());
        Ok(())
    }

    async fn read(&self, key: &str) -> strands::Result<Option<Vec<u8>>> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }

    async fn delete(&self, key: &str) -> strands::Result<()> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }

    async fn list(&self, prefix: &str) -> strands::Result<Vec<String>> {
        let entries = self.0.lock().unwrap();
        Ok(entries
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect())
    }
}

struct Twice;

#[strands::async_trait]
impl ModelRetryStrategy for Twice {
    fn name(&self) -> &str {
        "app:twice"
    }

    async fn compute_retry_decision(&self, context: &ModelRetryContext<'_>) -> RetryDecision {
        if context.attempt_count().get() < 3 && context.error().is_some_and(strands::Error::is_model_error) {
            RetryDecision::Retry(Duration::from_millis(10))
        } else {
            RetryDecision::NoRetry
        }
    }
}

struct Flat;

impl BackoffStrategy for Flat {
    fn next_delay(&self, context: &BackoffContext) -> Duration {
        Duration::from_millis(u64::from(context.attempt.get()))
    }
}

struct Prefix;

#[strands::async_trait]
impl SearchStrategy for Prefix {
    async fn search(&self, storage: &dyn Storage, query: &str) -> strands::Result<Vec<StorageSearchResult>> {
        let keys = storage.list(query).await?;
        Ok(keys
            .into_iter()
            .map(|key| StorageSearchResult::new(key, 1.0))
            .collect())
    }
}

struct Typed;

#[strands::async_trait]
impl SearchStrategy<InMemoryStorage> for Typed {
    async fn search(
        &self,
        storage: &InMemoryStorage,
        query: &str,
    ) -> strands::Result<Vec<StorageSearchResult>> {
        let keys = storage.list(query).await?;
        Ok(keys
            .into_iter()
            .map(|key| StorageSearchResult::new(key, 1.0))
            .collect())
    }
}

struct Lines;

#[strands::async_trait]
impl Extractor for Lines {
    async fn extract(
        &self,
        messages: &[Message],
        context: &ExtractorContext,
    ) -> strands::Result<Vec<ExtractionResult>> {
        let _ = context.default_model.is_some();
        Ok(messages
            .iter()
            .map(|message| ExtractionResult::new(format!("{:?}", message.role())))
            .collect())
    }
}

struct EveryTurn;

impl ExtractionTrigger for EveryTurn {
    fn name(&self) -> &str {
        "every-turn"
    }

    fn attach(&self, context: ExtractionTriggerContext<'_>) {
        let fire = context.fire.clone();
        let _cleanup = context
            .agent
            .hooks()
            .add_hook(move |_event: &mut AfterInvocationEvent<'_>| fire.fire(None));
    }
}

struct DropHalf;

#[strands::async_trait]
impl ContextStrategy for DropHalf {
    fn name(&self) -> &str {
        "drop-half"
    }

    async fn apply(&self, context: &mut ContextState<'_>) -> strands::Result<bool> {
        let count = context.messages().len() / 2;
        context.messages_mut().drain(..count);
        Ok(count > 0)
    }
}

#[tokio::test]
async fn a_downstream_tool_plugin_storage_and_retry_strategy_run_in_an_agent() -> strands::Result<()> {
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![TextBlock::new("hi").into()]);
    let mut agent = Agent::builder()
        .model(model)
        .plugin(Mine)
        .retry_strategy(Twice)
        .storage(Arc::new(Mem::default()))
        .printer(false)
        .build()?;

    let result = agent.invoke("x").await?;
    assert_eq!(result.to_string(), "hi");
    let direct = agent.tool("manual").invoke(serde_json::json!({})).await?;
    assert_eq!(direct.status(), &ToolResultStatus::Success);
    agent.shutdown().await
}

#[tokio::test]
async fn a_downstream_storage_serves_a_downstream_search_strategy() -> strands::Result<()> {
    let storage = Mem::default();
    storage.write("notes/a", b"1").await?;
    storage.write("other/b", b"2").await?;
    assert_eq!(storage.read("notes/a").await?, Some(b"1".to_vec()));

    let found = Prefix.search(&storage, "notes/").await?;
    assert_eq!(found.len(), 1);

    let typed = InMemoryStorage::default();
    typed.write("notes/a", b"1").await?;
    assert_eq!(Typed.search(&typed, "notes/").await?.len(), 1);

    storage.delete("notes/a").await?;
    assert!(storage.list("notes/").await?.is_empty());
    Ok(())
}

#[test]
fn downstream_strategies_extractors_and_triggers_are_trait_objects() {
    let _backoff: Arc<dyn BackoffStrategy> = Arc::new(Flat);
    let _search: Arc<dyn SearchStrategy> = Arc::new(Prefix);
    let _extractor: Arc<dyn Extractor> = Arc::new(Lines);
    let trigger: Arc<dyn ExtractionTrigger> = Arc::new(EveryTurn);
    let strategy: Arc<dyn ContextStrategy> = Arc::new(DropHalf);
    assert_eq!(trigger.name(), "every-turn");
    assert_eq!(strategy.name(), "drop-half");
}

/// Names each crate-root re-export, so dropping one fails this test and not only a doc link.
#[test]
fn every_crate_root_re_export_is_nameable() {
    #[expect(unused_imports, reason = "the imports are the assertion")]
    use strands::{
        Agent, AgentConfig, AgentResult, AgentStream, AgentStreamEvent, CancelHandle, CancelOptions,
        CancelSignal, ContentBlock, Error, InterruptResponse, InvocationState, InvokeArgs, InvokeOptions,
        JsonValue, Message, Result, StateStore, StopReason, Tool, ToolContext, ToolError, ToolOutcome,
        ToolSource, async_trait, indexmap, schemars, tool, tool_list, tools,
    };
    #[cfg(feature = "bedrock")]
    #[expect(unused_imports, reason = "the imports are the assertion")]
    use strands::{
        BedrockCacheConfig, BedrockCacheTtl, BedrockGuardrailConfig, BedrockGuardrailRedactionConfig,
        BedrockModel, BedrockModelConfig, BedrockModelOptions,
    };
}
