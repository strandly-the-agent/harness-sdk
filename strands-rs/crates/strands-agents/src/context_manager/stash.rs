//! L1 stash: deterministic key/value persistence of offloaded content
//! (`strands-ts/src/context-manager/stash.ts`).

use std::collections::HashSet;

use crate::error::Result;
use crate::storage::{namespace, StorageRef};
use crate::types::{ContentBlock, JsonValue, Message, ToolResultContent};

/// Storage prefix every stash lives under.
pub const STASH_PREFIX: &str = "context";

/// Render stash references as the in-band back-reference suffix.
pub fn format_stash_refs(refs: &[String]) -> String {
    match refs {
        [] => String::new(),
        [one] => format!(" [ref: {one}]"),
        many => format!(" [refs: {}]", many.join(", ")),
    }
}

/// Resolve the stash root. A namespaced view passed as *explicit* stash storage becomes the exact
/// root (agents sharing the view share one stash); anything else gets the per-agent prefix.
pub fn resolve_stash_namespace(
    storage: StorageRef,
    session_id: &str,
    agent_id: &str,
    custom_stash_namespace: bool,
) -> StorageRef {
    if custom_stash_namespace && storage.is_namespaced() {
        return storage;
    }
    namespace(
        storage,
        &format!("{STASH_PREFIX}/{session_id}/scopes/agent/{agent_id}"),
    )
}

/// Offloaded content, keyed deterministically so keys can be recomputed after a restart.
pub struct Stash {
    storage: StorageRef,
    base_storage: StorageRef,
    session_id: String,
}

impl Stash {
    /// Build a stash over `storage` for one agent in one session.
    pub fn new(
        storage: StorageRef,
        session_id: impl Into<String>,
        agent_id: &str,
        custom_stash_namespace: bool,
    ) -> Self {
        let session_id = session_id.into();
        let scoped = resolve_stash_namespace(storage.clone(), &session_id, agent_id, custom_stash_namespace);
        Self {
            storage: scoped,
            base_storage: storage,
            session_id,
        }
    }

    /// Write one stash entry under `<id>_<block_index>`.
    pub async fn store(&self, id: &str, block_index: usize, data: &JsonValue) -> Result<String> {
        let key = format!("{id}_{block_index}");
        let bytes = serde_json::to_vec(data).map_err(crate::error::Error::other)?;
        self.storage.write(&key, &bytes).await?;
        Ok(key)
    }

    /// The keys a block's content is (or would be) stashed under. Deterministic, no I/O.
    pub fn refs_for(&self, block: &ContentBlock, message: &Message, block_index: usize) -> Vec<String> {
        match block {
            ContentBlock::ToolResult(result) => (0..result.content.len())
                .map(|index| format!("{}_{index}", result.tool_use_id))
                .collect(),
            _ => vec![format!("{}_{block_index}", message.tracking_id)],
        }
    }

    /// Stash every stashable block of `message`, skipping tool results for `skip_tool_use_ids`.
    pub async fn store_message(&self, message: &Message, skip_tool_use_ids: &HashSet<String>) {
        for (index, block) in message.content.iter().enumerate() {
            match block {
                ContentBlock::ToolResult(result) => {
                    if skip_tool_use_ids.contains(&result.tool_use_id) {
                        continue;
                    }
                    for (item_index, item) in result.content.iter().enumerate() {
                        let value = match serde_json::to_value(item) {
                            Ok(value) => value,
                            Err(error) => {
                                tracing::warn!("error=<{error}> | failed to stash tool result item");
                                continue;
                            }
                        };
                        if let Err(error) = self.store(&result.tool_use_id, item_index, &value).await {
                            tracing::warn!("error=<{error}> | failed to stash tool result item");
                        }
                    }
                }
                ContentBlock::ToolUse(_) | ContentBlock::CachePoint(_) | ContentBlock::Reasoning(_) => {}
                other => {
                    let value = match serde_json::to_value(other) {
                        Ok(value) => value,
                        Err(error) => {
                            tracing::warn!("error=<{error}> | failed to stash content block");
                            continue;
                        }
                    };
                    if let Err(error) = self.store(&message.tracking_id, index, &value).await {
                        tracing::warn!("error=<{error}> | failed to stash content block");
                    }
                }
            }
        }
    }

    /// Read one entry back; `None` when the reference is unknown.
    pub async fn retrieve(&self, reference: &str) -> Option<JsonValue> {
        let bytes = self.storage.read(reference).await.ok()??;
        serde_json::from_slice(&bytes).ok()
    }

    /// Every key in this stash.
    pub async fn list(&self) -> Result<Vec<String>> {
        self.storage.list("").await
    }

    /// Delete one entry.
    pub async fn delete(&self, reference: &str) -> Result<()> {
        self.storage.delete(reference).await?;
        tracing::debug!("reference=<{reference}> | stash entry deleted");
        Ok(())
    }

    /// Delete every entry of this stash's session, across agents. A caller-scoped stash (an
    /// explicit namespaced view) is left alone.
    pub async fn clear_session(&self) -> Result<()> {
        if std::ptr::eq(
            std::sync::Arc::as_ptr(&self.storage) as *const (),
            std::sync::Arc::as_ptr(&self.base_storage) as *const (),
        ) {
            tracing::debug!(
                "session_id=<{}> | skipping stash deletion, stash is caller-scoped",
                self.session_id
            );
            return Ok(());
        }
        let prefix = format!("{STASH_PREFIX}/{}/", self.session_id);
        for key in self.base_storage.list(&prefix).await? {
            self.base_storage.delete(&key).await?;
        }
        Ok(())
    }
}

/// Collect the tool-use ids of `retrieve_context` calls so their results are never stashed.
pub fn track_retrieval_tool_use_ids(message: &Message, skip: &mut HashSet<String>) {
    if message.role != crate::types::Role::Assistant {
        return;
    }
    for use_block in message.tool_uses() {
        if use_block.name == crate::context_manager::retrieval_tool::RETRIEVAL_TOOL_NAME {
            skip.insert(use_block.tool_use_id.clone());
        }
    }
}

/// Text of a tool-result item, for search-mode retrieval.
pub fn tool_result_item_text(item: &ToolResultContent) -> Option<String> {
    match item {
        ToolResultContent::Text(text) => Some(text.clone()),
        ToolResultContent::Json(json) => serde_json::to_string_pretty(json).ok(),
        _ => None,
    }
}
