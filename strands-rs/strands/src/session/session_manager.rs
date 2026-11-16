//! The session manager (TS `session/session-manager.ts`).

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;

use super::snapshot_storage_adapter::{SnapshotLocation, SnapshotStorageAdapter};
use super::validation::validate_identifier;
use crate::agent::{Agent, TakeSnapshotOptions};
use crate::error::{Error, Result};
use crate::experimental::Stash;
use crate::hooks::{AfterInvocationEvent, AfterModelCallEvent, MessageAddedEvent};
use crate::plugins::Plugin;
use crate::storage::{Storage, namespace};
use crate::types::agent::LocalAgent;
use crate::types::json::JsonValue;
use crate::types::snapshot::{Scope, Snapshot};

const DEFAULT_SESSION_ID: &str = "default-session";
const NO_STORAGE_MESSAGE: &str = "SessionManager requires a storage backend. Provide storage in SessionManagerConfig or set storage on the Agent.";

/// When the latest snapshot is saved automatically (TS `SaveLatestStrategy`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum SaveLatestStrategy {
    /// After every added message.
    Message,
    /// After every invocation (the default).
    #[default]
    Invocation,
    /// Only when the snapshot trigger fires; no automatic saves.
    Trigger,
}

/// What a [`SnapshotTriggerCallback`] sees (TS `SnapshotTriggerParams`).
#[derive(Debug)]
#[non_exhaustive]
pub struct SnapshotTriggerParams<'a> {
    /// The agent after the invocation.
    pub agent_data: &'a LocalAgent,
}

/// Decides after each invocation whether to save an immutable snapshot (TS
/// `SnapshotTriggerCallback`).
pub type SnapshotTriggerCallback = Arc<dyn Fn(&SnapshotTriggerParams<'_>) -> bool + Send + Sync>;

/// Configuration for [`SessionManager`] (TS `SessionManagerConfig`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Clone, Default, bon::Builder)]
#[builder(on(String, into))]
pub struct SessionManagerConfig {
    /// Where snapshots go; defaults to the agent's `storage`. Unless it is already a namespaced
    /// view, keys go under `session/`.
    pub storage: Option<Arc<dyn Storage>>,
    /// The session id (`^[a-z0-9_-]+$`, default `default-session`).
    pub session_id: Option<String>,
    /// When the latest snapshot is saved (default [`SaveLatestStrategy::Invocation`]).
    #[builder(default)]
    pub save_latest_on: SaveLatestStrategy,
    /// Saves an immutable snapshot (and the latest) after an invocation when it returns `true`.
    pub snapshot_trigger: Option<SnapshotTriggerCallback>,
}

impl fmt::Debug for SessionManagerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionManagerConfig")
            .field("storage", &self.storage.as_ref().map(|_| ".."))
            .field("session_id", &self.session_id)
            .field("save_latest_on", &self.save_latest_on)
            .field("snapshot_trigger", &self.snapshot_trigger.as_ref().map(|_| ".."))
            .finish()
    }
}

/// Options for [`SessionManager::list_snapshot_ids`].
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(on(String, into))]
pub struct ListSnapshotIdsOptions {
    /// At most this many ids.
    pub limit: Option<usize>,
    /// Only ids after this UUID v7.
    pub start_after: Option<String>,
}

/// An agent a [`SessionManager`] can snapshot: [`Agent`] or [`LocalAgent`]. Sealed.
pub trait SnapshotTarget: sealed::Sealed {
    /// The agent view.
    fn local_agent(&self) -> &LocalAgent;
    /// The agent view, mutably.
    fn local_agent_mut(&mut self) -> &mut LocalAgent;
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for crate::agent::Agent {}
    impl Sealed for crate::types::agent::LocalAgent {}
}

impl SnapshotTarget for Agent {
    fn local_agent(&self) -> &LocalAgent {
        &self.local
    }

    fn local_agent_mut(&mut self) -> &mut LocalAgent {
        &mut self.local
    }
}

impl SnapshotTarget for LocalAgent {
    fn local_agent(&self) -> &LocalAgent {
        self
    }

    fn local_agent_mut(&mut self) -> &mut LocalAgent {
        self
    }
}

/// Saves and restores agent sessions as snapshots (TS `SessionManager`), registered on the agent
/// through `AgentConfig::session_manager`.
///
/// Keys follow the TS layout: `<sessionId>/scopes/agent/<agentId>/snapshots/snapshot_latest.json`
/// and `.../immutable_history/snapshot_<uuidv7>.json`. Clones share state.
///
/// ```
/// use std::sync::Arc;
///
/// use strands::session::{SessionManager, SessionManagerConfig};
/// use strands::storage::InMemoryStorage;
///
/// let manager = SessionManager::new(SessionManagerConfig {
///     storage: Some(Arc::new(InMemoryStorage::new())),
///     session_id: Some("user-123".into()),
///     ..Default::default()
/// })?;
/// assert_eq!(manager.session_id(), "user-123");
/// # Ok::<(), strands::Error>(())
/// ```
#[derive(Clone)]
pub struct SessionManager {
    inner: Arc<Inner>,
}

struct Inner {
    session_id: String,
    config_storage: bool,
    adapter: Mutex<Option<SnapshotStorageAdapter>>,
    save_latest_on: SaveLatestStrategy,
    snapshot_trigger: Option<SnapshotTriggerCallback>,
    agent_stash: Mutex<Option<Stash>>,
}

impl fmt::Debug for SessionManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionManager")
            .field("session_id", &self.inner.session_id)
            .field("save_latest_on", &self.inner.save_latest_on)
            .finish_non_exhaustive()
    }
}

fn resolve_adapter(storage: Arc<dyn Storage>) -> Result<SnapshotStorageAdapter> {
    let scoped = if storage.is_namespaced() {
        storage
    } else {
        namespace(storage, "session")?
    };
    Ok(SnapshotStorageAdapter::new(scoped))
}

impl SessionManager {
    /// The plugin name (TS `name`).
    pub const NAME: &'static str = "strands:session-manager";

    /// A session manager from `config`.
    ///
    /// # Errors
    ///
    /// The session id doesn't match `^[a-z0-9_-]+$`.
    pub fn new(config: SessionManagerConfig) -> Result<Self> {
        let session_id = config.session_id.unwrap_or_else(|| DEFAULT_SESSION_ID.to_owned());
        validate_identifier(&session_id)?;
        let config_storage = config.storage.is_some();
        let adapter = config.storage.map(resolve_adapter).transpose()?;
        Ok(Self {
            inner: Arc::new(Inner {
                session_id,
                config_storage,
                adapter: Mutex::new(adapter),
                save_latest_on: config.save_latest_on,
                snapshot_trigger: config.snapshot_trigger,
                agent_stash: Mutex::new(None),
            }),
        })
    }

    /// The session id.
    pub fn session_id(&self) -> &str {
        &self.inner.session_id
    }

    /// Saves `target`'s session snapshot as the latest, or as a new immutable snapshot with a
    /// UUID v7 id.
    ///
    /// # Errors
    ///
    /// No storage is configured, or the write failed.
    pub async fn save_snapshot(&self, target: &impl SnapshotTarget, is_latest: bool) -> Result<()> {
        self.inner.save_snapshot(target.local_agent(), is_latest).await
    }

    /// Restores the latest snapshot, or the immutable one with `snapshot_id`, into `target`;
    /// `false` when it doesn't exist.
    ///
    /// # Errors
    ///
    /// No storage is configured, the read failed, or the snapshot doesn't load.
    pub async fn restore_snapshot(
        &self,
        target: &mut impl SnapshotTarget,
        snapshot_id: Option<&str>,
    ) -> Result<bool> {
        self.inner
            .restore_snapshot(target.local_agent_mut(), snapshot_id)
            .await
    }

    /// The immutable snapshot ids of `target`, oldest first.
    ///
    /// # Errors
    ///
    /// No storage is configured, `start_after` isn't a UUID v7, or the listing failed.
    pub async fn list_snapshot_ids(
        &self,
        target: &impl SnapshotTarget,
        options: ListSnapshotIdsOptions,
    ) -> Result<Vec<String>> {
        let location = self.inner.location(target.local_agent());
        self.inner
            .adapter()?
            .list_snapshot_ids(&location, options.limit, options.start_after.as_deref())
            .await
    }

    /// Deletes every key of this session.
    ///
    /// # Errors
    ///
    /// No storage is configured, or a delete failed.
    pub async fn delete_session(&self) -> Result<()> {
        self.inner
            .adapter()?
            .delete_session(&self.inner.session_id)
            .await?;
        let stash = self
            .inner
            .agent_stash
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        match stash {
            Some(stash) => stash.clear_session().await,
            None => Ok(()),
        }
    }
}

/// Records the context manager's stash in `snapshot` (TS `_includeStashData`): a durable stash
/// by its storage type, an ephemeral one inline.
async fn include_stash_data(agent: &LocalAgent, snapshot: &mut Snapshot) -> Result<()> {
    let Some(stash) = agent.context_stash() else {
        return Ok(());
    };
    if stash.is_durable() {
        let data = serde_json::json!({"location": "external", "storageType": stash.storage_type_name()});
        snapshot.data_mut().insert("stash".to_owned(), data);
        return Ok(());
    }
    let entries = stash.take_snapshot().await?;
    if !entries.is_empty() {
        let data = serde_json::json!({"location": "inline", "entries": entries});
        snapshot.data_mut().insert("stash".to_owned(), data);
    }
    Ok(())
}

impl Inner {
    fn adapter(&self) -> Result<SnapshotStorageAdapter> {
        self.adapter
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or_else(|| Error::other(NO_STORAGE_MESSAGE))
    }

    fn location(&self, agent: &LocalAgent) -> SnapshotLocation {
        SnapshotLocation {
            session_id: self.session_id.clone(),
            scope: Scope::Agent,
            scope_id: agent.id().to_owned(),
        }
    }

    async fn save_snapshot(&self, agent: &LocalAgent, is_latest: bool) -> Result<()> {
        let mut snapshot = agent.take_snapshot(&TakeSnapshotOptions::session())?;
        include_stash_data(agent, &mut snapshot).await?;
        let snapshot_id = if is_latest {
            "latest".to_owned()
        } else {
            uuid::Uuid::now_v7().to_string()
        };
        self.adapter()?
            .save_snapshot(&self.location(agent), &snapshot_id, is_latest, &snapshot)
            .await
    }

    async fn save_immutable_and_latest(&self, agent: &LocalAgent) -> Result<()> {
        let mut snapshot = agent.take_snapshot(&TakeSnapshotOptions::session())?;
        include_stash_data(agent, &mut snapshot).await?;
        let snapshot_id = uuid::Uuid::now_v7().to_string();
        let adapter = self.adapter()?;
        let location = self.location(agent);
        let (history, latest) = futures::join!(
            adapter.save_snapshot(&location, &snapshot_id, false, &snapshot),
            adapter.save_snapshot(&location, "latest", true, &snapshot),
        );
        history.and(latest)
    }

    async fn restore_snapshot(&self, agent: &mut LocalAgent, snapshot_id: Option<&str>) -> Result<bool> {
        let location = self.location(agent);
        let Some(snapshot) = self.adapter()?.load_snapshot(&location, snapshot_id).await? else {
            return Ok(false);
        };
        agent.load_snapshot(&snapshot)?;
        self.restore_stash_data(agent, &snapshot).await?;
        Ok(true)
    }

    /// TS `_restoreStashData`.
    async fn restore_stash_data(&self, agent: &LocalAgent, snapshot: &Snapshot) -> Result<()> {
        let (Some(data), Some(stash)) = (snapshot.data().get("stash"), agent.context_stash()) else {
            return Ok(());
        };
        match data.get("location").and_then(JsonValue::as_str) {
            Some("external") => {
                let saved = data
                    .get("storageType")
                    .and_then(JsonValue::as_str)
                    .unwrap_or_default();
                let current = stash.storage_type_name();
                if !saved.is_empty() && !current.is_empty() && saved != current {
                    tracing::warn!(
                        "session_id=<{}>, snapshot_storage=<{saved}>, current_storage=<{current}> | stash storage type changed since snapshot was created, stash data may be inaccessible",
                        self.session_id
                    );
                }
                Ok(())
            }
            Some("inline") => match data.get("entries").and_then(JsonValue::as_object) {
                Some(entries) => stash.load_snapshot(entries).await,
                None => Ok(()),
            },
            _ => Ok(()),
        }
    }

    /// TS `_onAgentInitialized`.
    async fn restore_on_initialize(&self, agent: &mut LocalAgent) -> Result<()> {
        let had_messages = !agent.messages().is_empty();
        let restored = self.restore_snapshot(agent, None).await?;
        if restored && had_messages {
            tracing::warn!(
                "agent_id=<{}>, session_id=<{}> | agent had existing messages that were overwritten by session restore",
                agent.id(),
                self.session_id
            );
        }
        if restored && agent.model().stateful() && !agent.messages().is_empty() {
            tracing::debug!(
                "agent_id=<{}>, message_count=<{}> | discarding restored messages for stateful model",
                agent.id(),
                agent.messages().len()
            );
            agent.messages_mut().clear();
        }
        Ok(())
    }

    /// TS `_onAfterAgentInvocation`.
    async fn save_after_invocation(&self, agent: &LocalAgent) -> Result<()> {
        if self.save_latest_on != SaveLatestStrategy::Trigger {
            self.save_snapshot(agent, true).await?;
        }
        let fire = self
            .snapshot_trigger
            .as_ref()
            .is_some_and(|trigger| trigger(&SnapshotTriggerParams { agent_data: agent }));
        if fire {
            self.save_immutable_and_latest(agent).await?;
        }
        Ok(())
    }

    fn log_failure(&self, result: Result<()>) {
        if let Err(error) = result {
            tracing::error!(
                "session_id=<{}>, error=<{error}> | failed to save the session snapshot",
                self.session_id
            );
        }
    }
}

#[async_trait]
impl Plugin for SessionManager {
    fn name(&self) -> &str {
        Self::NAME
    }

    /// Resolves the storage, restores the latest snapshot (TS restores on `InitializedEvent`; this
    /// runs at the session manager's registration slot so a failed restore fails
    /// `initialize`), and installs the save hooks.
    async fn init_agent(self: Arc<Self>, agent: &mut LocalAgent) -> Result<()> {
        let inner = Arc::clone(&self.inner);
        if !inner.config_storage {
            let storage = agent
                .storage()
                .cloned()
                .ok_or_else(|| Error::other(NO_STORAGE_MESSAGE))?;
            *inner.adapter.lock().unwrap_or_else(PoisonError::into_inner) = Some(resolve_adapter(storage)?);
        }
        *inner.agent_stash.lock().unwrap_or_else(PoisonError::into_inner) = agent.context_stash().cloned();
        inner.restore_on_initialize(agent).await?;
        let hooks = agent.hooks();
        if inner.save_latest_on == SaveLatestStrategy::Message {
            let inner = Arc::clone(&inner);
            hooks.add_async_hook::<MessageAddedEvent<'static>>(move |event| {
                let inner = Arc::clone(&inner);
                Box::pin(async move {
                    let result = inner.save_snapshot(event.agent(), true).await;
                    inner.log_failure(result);
                })
            });
        }
        if inner.save_latest_on != SaveLatestStrategy::Trigger {
            let inner = Arc::clone(&inner);
            hooks.add_async_hook::<AfterModelCallEvent<'static>>(move |event| {
                let inner = Arc::clone(&inner);
                Box::pin(async move {
                    let redacted = event.stop_data().is_some_and(|data| data.redaction().is_some());
                    if redacted {
                        let result = inner.save_snapshot(event.agent(), true).await;
                        inner.log_failure(result);
                    }
                })
            });
        }
        hooks.add_async_hook::<AfterInvocationEvent<'static>>(move |event| {
            let inner = Arc::clone(&inner);
            Box::pin(async move {
                let result = inner.save_after_invocation(event.agent()).await;
                inner.log_failure(result);
            })
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests;
