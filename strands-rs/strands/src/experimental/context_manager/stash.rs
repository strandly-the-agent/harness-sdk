//! The stash of offloaded content (TS `context-manager/stash.ts`).

use std::collections::HashSet;
use std::sync::{Arc, LazyLock};

use regex::Regex;

use crate::error::{Error, Result};
use crate::storage::{Storage, namespace};
use crate::types::json::{JsonObject, JsonValue, from_json_str};
use crate::types::messages::{ContentBlock, Message, ToolResultContent};

/// The key prefix of every stash (TS `STASH_PREFIX`).
pub(crate) const STASH_PREFIX: &str = "context";

/// The label before stash refs in a truncated or summarized block.
pub(crate) const STASHED: &str = "[Stashed]";

static STASH_REFS_SUFFIX: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r" \[refs?: [^\]]+\]$").ok());

fn is_placeholder_text(text: &str) -> bool {
    STASH_REFS_SUFFIX
        .as_ref()
        .is_some_and(|regex| regex.is_match(text))
}

/// Whether stashed data is an offload placeholder rather than original content (TS
/// `isOffloadPlaceholder`).
pub(crate) fn is_offload_placeholder(data: &JsonValue) -> bool {
    data.get("text")
        .and_then(JsonValue::as_str)
        .is_some_and(is_placeholder_text)
}

/// ` [ref: a]` or ` [refs: a, b]`, empty for no refs (TS `formatStashRefs`).
pub(crate) fn format_stash_refs(refs: &[String]) -> String {
    match refs {
        [] => String::new(),
        [one] => format!(" [ref: {one}]"),
        many => format!(" [refs: {}]", many.join(", ")),
    }
}

fn encode(value: &impl serde::Serialize) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(Error::other)
}

#[derive(Debug)]
struct StashInner {
    storage: Arc<dyn Storage>,
    base_storage: Arc<dyn Storage>,
    caller_scoped: bool,
    session_id: String,
    storage_type_name: String,
}

/// Persists offloaded content so `retrieve_context` can return it (TS `Stash`).
///
/// Entries live under `context/<sessionId>/scopes/agent/<agentId>/` keyed `<id>_<blockIndex>`,
/// unless a namespaced view was given as the stash storage, which then is the stash root.
/// Cheap to clone; clones share the storage.
#[derive(Debug, Clone)]
pub struct Stash {
    inner: Arc<StashInner>,
}

impl Stash {
    /// A stash for one agent of one session; `custom_namespace` marks storage the caller passed
    /// as the stash's own.
    ///
    /// # Errors
    ///
    /// The session or agent id is not a valid key segment.
    pub fn new(
        storage: Arc<dyn Storage>,
        session_id: &str,
        agent_id: &str,
        custom_namespace: bool,
    ) -> Result<Self> {
        let caller_scoped = custom_namespace && storage.is_namespaced();
        let scoped = if caller_scoped {
            Arc::clone(&storage)
        } else {
            namespace(
                Arc::clone(&storage),
                &format!("{STASH_PREFIX}/{session_id}/scopes/agent/{agent_id}"),
            )?
        };
        Ok(Self {
            inner: Arc::new(StashInner {
                storage_type_name: storage.storage_type_name().to_owned(),
                storage: scoped,
                base_storage: storage,
                caller_scoped,
                session_id: session_id.to_owned(),
            }),
        })
    }

    /// The class name TS records for the backing storage (`LocalFileStorage`,
    /// `InMemoryStorage`, ...), written to session snapshots as `storageType`.
    pub fn storage_type_name(&self) -> &str {
        &self.inner.storage_type_name
    }

    /// Whether the backing storage outlives the process.
    pub fn is_durable(&self) -> bool {
        !self.inner.base_storage.is_ephemeral()
    }

    /// Stores `data` under `<id>_<block_index>` and returns that key.
    ///
    /// # Errors
    ///
    /// The storage write failed.
    pub async fn store(&self, id: &str, block_index: usize, data: &[u8]) -> Result<String> {
        let key = format!("{id}_{block_index}");
        self.inner.storage.write(&key, data).await?;
        Ok(key)
    }

    /// The keys the stash holds for `block` (TS `refsFor`): one per tool-result item, else one
    /// for the block.
    pub fn refs_for(&self, block: &ContentBlock, message: &Message, block_index: usize) -> Vec<String> {
        if let ContentBlock::ToolResult(result) = block {
            return (0..result.content().len())
                .map(|index| format!("{}_{index}", result.tool_use_id()))
                .collect();
        }
        vec![format!("{}_{block_index}", message.tracking_id())]
    }

    /// Stores every block of `message` that can be offloaded (TS `storeMessage`). Failures are
    /// logged, not returned.
    pub async fn store_message(&self, message: &Message, skip_tool_use_ids: Option<&HashSet<String>>) {
        for (block_index, block) in message.content().iter().enumerate() {
            match block {
                ContentBlock::ToolResult(result) => {
                    if skip_tool_use_ids.is_some_and(|skip| skip.contains(result.tool_use_id())) {
                        continue;
                    }
                    let placeholder = result
                        .content()
                        .iter()
                        .any(|item| matches!(item, ToolResultContent::Text(text) if is_placeholder_text(text.text())));
                    if placeholder {
                        continue;
                    }
                    for (index, item) in result.content().iter().enumerate() {
                        if let Err(error) = self.store_json(result.tool_use_id(), index, item).await {
                            tracing::warn!(
                                "toolUseId=<{}>, blockIndex=<{index}>, error=<{error}> | failed to stash sub-block",
                                result.tool_use_id()
                            );
                        }
                    }
                }
                ContentBlock::ToolUse(_) | ContentBlock::CachePoint(_) | ContentBlock::Reasoning(_) => {}
                ContentBlock::Text(text) if is_placeholder_text(text.text()) => {}
                ContentBlock::Summary(summary) if is_placeholder_text(summary.text()) => {}
                _ => {
                    if let Err(error) = self.store_json(message.tracking_id(), block_index, block).await {
                        tracing::warn!(
                            "trackingId=<{}>, error=<{error}> | failed to stash block",
                            message.tracking_id()
                        );
                    }
                }
            }
        }
    }

    async fn store_json(&self, id: &str, index: usize, value: &impl serde::Serialize) -> Result<String> {
        self.store(id, index, &encode(value)?).await
    }

    /// The stored JSON for `reference`, or `None` when absent.
    ///
    /// # Errors
    ///
    /// The read failed or the stored bytes are not JSON.
    pub async fn retrieve(&self, reference: &str) -> Result<Option<JsonValue>> {
        let Some(bytes) = self.inner.storage.read(reference).await? else {
            return Ok(None);
        };
        from_json_str(&String::from_utf8_lossy(&bytes))
            .map(Some)
            .map_err(Error::other)
    }

    /// Every stash key.
    ///
    /// # Errors
    ///
    /// The storage list failed.
    pub async fn list(&self) -> Result<Vec<String>> {
        self.inner.storage.list("").await
    }

    /// Deletes one entry.
    ///
    /// # Errors
    ///
    /// The storage delete failed.
    pub async fn delete(&self, reference: &str) -> Result<()> {
        self.inner.storage.delete(reference).await?;
        tracing::debug!("reference=<{reference}> | stash entry deleted");
        Ok(())
    }

    /// Deletes every entry.
    ///
    /// # Errors
    ///
    /// A list or delete failed.
    pub async fn clear(&self) -> Result<()> {
        for key in self.list().await? {
            self.delete(&key).await?;
        }
        Ok(())
    }

    /// Deletes every agent's stash of this session (TS `clearSession`); a no-op for a
    /// caller-scoped stash.
    ///
    /// # Errors
    ///
    /// A list or delete failed.
    pub async fn clear_session(&self) -> Result<()> {
        if self.inner.caller_scoped {
            tracing::debug!(
                "session_id=<{}> | skipping stash deletion, stash is caller-scoped",
                self.inner.session_id
            );
            return Ok(());
        }
        let prefix = format!("{STASH_PREFIX}/{}/", self.inner.session_id);
        for key in self.inner.base_storage.list(&prefix).await? {
            self.inner.base_storage.delete(&key).await?;
        }
        Ok(())
    }

    /// Every entry as `{key: data}` (TS `takeSnapshot`).
    ///
    /// # Errors
    ///
    /// A list or read failed.
    pub async fn take_snapshot(&self) -> Result<JsonObject> {
        let mut entries = JsonObject::new();
        for key in self.list().await? {
            if let Some(data) = self.retrieve(&key).await? {
                entries.insert(key, data);
            }
        }
        Ok(entries)
    }

    /// Writes every `{key: data}` entry (TS `loadSnapshot`).
    ///
    /// # Errors
    ///
    /// A write failed.
    pub async fn load_snapshot(&self, entries: &JsonObject) -> Result<()> {
        for (key, data) in entries {
            self.inner.storage.write(key, &encode(data)?).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
