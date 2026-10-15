//! In-memory task store: folds the events an [`A2AExecutor`](super::A2AExecutor) publishes into
//! the [`Task`] a `message/send`, `tasks/get` or `tasks/cancel` call answers with.

use std::collections::HashMap;

use parking_lot::Mutex;

use super::types::{A2AEventData, Artifact, Message, Task};

/// The tasks a server has served, keyed by task id, kept in memory for the server's lifetime.
///
/// Mirrors `@a2a-js/sdk`'s `InMemoryTaskStore` plus the event aggregation its
/// `DefaultRequestHandler` performs: [`TaskStore::apply`] folds one executor event into the
/// stored task so the final snapshot carries the terminal status, the message history and every
/// artifact.
#[derive(Debug, Default)]
pub struct TaskStore {
    tasks: Mutex<HashMap<String, Task>>,
}

impl TaskStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// The task under `id`, if this store has seen it.
    pub fn get(&self, id: &str) -> Option<Task> {
        self.tasks.lock().get(id).cloned()
    }

    /// Insert `task`, replacing any task of the same id.
    pub fn save(&self, task: Task) {
        self.tasks.lock().insert(task.id.clone(), task);
    }

    /// Append `message` to the history of the task under `id`, returning it as it now stands.
    ///
    /// `None` when no such task is stored; a message already in the history is not appended twice.
    pub fn append_history(&self, id: &str, message: Message) -> Option<Task> {
        let mut tasks = self.tasks.lock();
        let task = tasks.get_mut(id)?;
        push_history(task, message);
        Some(task.clone())
    }

    /// Fold one executor event into the stored task, returning the task as it now stands.
    ///
    /// A [`A2AEventData::Message`] event belongs to no task and is ignored; an update naming an
    /// unknown task is dropped.
    pub fn apply(&self, event: &A2AEventData) -> Option<Task> {
        let mut tasks = self.tasks.lock();
        match event {
            A2AEventData::Message(_) => None,
            A2AEventData::Task(incoming) => {
                let task = tasks
                    .entry(incoming.id.clone())
                    .or_insert_with(|| incoming.clone());
                task.status = incoming.status.clone();
                for artifact in incoming.artifacts.iter().flatten() {
                    upsert_artifact(task, artifact, false);
                }
                for message in incoming.history.iter().flatten() {
                    push_history(task, message.clone());
                }
                Some(task.clone())
            }
            A2AEventData::StatusUpdate(update) => {
                let task = tasks.get_mut(&update.task_id)?;
                task.status = update.status.clone();
                if let Some(message) = update.status.message.clone() {
                    push_history(task, message);
                }
                Some(task.clone())
            }
            A2AEventData::ArtifactUpdate(update) => {
                let task = tasks.get_mut(&update.task_id)?;
                upsert_artifact(task, &update.artifact, update.append.unwrap_or(false));
                Some(task.clone())
            }
        }
    }
}

/// Append `artifact`'s parts to the artifact of the same id, or add/replace it outright.
fn upsert_artifact(task: &mut Task, artifact: &Artifact, append: bool) {
    let artifacts = task.artifacts.get_or_insert_with(Vec::new);
    match artifacts
        .iter_mut()
        .find(|existing| existing.artifact_id == artifact.artifact_id)
    {
        Some(existing) if append => existing.parts.extend(artifact.parts.iter().cloned()),
        Some(existing) => *existing = artifact.clone(),
        None => artifacts.push(artifact.clone()),
    }
}

/// Append `message` to the task history unless a message of that id is already there.
fn push_history(task: &mut Task, message: Message) {
    let history = task.history.get_or_insert_with(Vec::new);
    if !history
        .iter()
        .any(|existing| existing.message_id == message.message_id)
    {
        history.push(message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::types::{
        Part, Role, TaskArtifactUpdateEvent, TaskState, TaskStatus, TaskStatusUpdateEvent,
    };

    fn task() -> Task {
        Task::new("task-1", "ctx-1", TaskStatus::new(TaskState::Submitted))
    }

    #[test]
    fn get_returns_none_for_an_unknown_task() {
        assert!(TaskStore::new().get("missing").is_none());
    }

    #[test]
    fn save_then_get_round_trips_the_task() {
        let store = TaskStore::new();
        store.save(task());
        assert_eq!(store.get("task-1").unwrap().context_id, "ctx-1");
    }

    #[test]
    fn task_event_upserts_and_keeps_existing_history() {
        let store = TaskStore::new();
        let mut saved = task();
        saved.history = Some(vec![Message::user("hi")]);
        store.save(saved);

        let updated = store
            .apply(&A2AEventData::Task(Task::new(
                "task-1",
                "ctx-1",
                TaskStatus::new(TaskState::Working),
            )))
            .unwrap();

        assert_eq!(updated.status.state, TaskState::Working);
        assert_eq!(updated.history.unwrap().len(), 1);
    }

    #[test]
    fn status_update_sets_state_and_appends_its_message_once() {
        let store = TaskStore::new();
        store.save(task());
        let mut status = TaskStatus::new(TaskState::Completed);
        let mut message = Message::new(Role::Agent, vec![Part::text("done")]);
        message.message_id = "m-1".to_owned();
        status.message = Some(message);
        let event = A2AEventData::StatusUpdate(TaskStatusUpdateEvent {
            task_id: "task-1".to_owned(),
            context_id: "ctx-1".to_owned(),
            status,
            final_: true,
            metadata: None,
        });

        store.apply(&event);
        let updated = store.apply(&event).unwrap();

        assert_eq!(updated.status.state, TaskState::Completed);
        assert_eq!(updated.history.unwrap().len(), 1);
    }

    #[test]
    fn artifact_update_appends_chunks_to_the_same_artifact() {
        let store = TaskStore::new();
        store.save(task());
        for (index, chunk) in ["hello ", "world"].iter().enumerate() {
            store.apply(&A2AEventData::ArtifactUpdate(TaskArtifactUpdateEvent {
                task_id: "task-1".to_owned(),
                context_id: "ctx-1".to_owned(),
                artifact: Artifact::new("artifact-1", vec![Part::text(*chunk)]),
                append: Some(index > 0),
                last_chunk: Some(index == 1),
                metadata: None,
            }));
        }

        let artifacts = store.get("task-1").unwrap().artifacts.unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].parts.len(), 2);
    }

    #[test]
    fn artifact_update_without_append_replaces_the_artifact() {
        let store = TaskStore::new();
        store.save(task());
        for chunk in ["first", "second"] {
            store.apply(&A2AEventData::ArtifactUpdate(TaskArtifactUpdateEvent {
                task_id: "task-1".to_owned(),
                context_id: "ctx-1".to_owned(),
                artifact: Artifact::new("artifact-1", vec![Part::text(chunk)]),
                append: None,
                last_chunk: None,
                metadata: None,
            }));
        }

        let artifacts = store.get("task-1").unwrap().artifacts.unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].parts[0].as_text(), Some("second"));
    }

    #[test]
    fn update_for_an_unknown_task_is_dropped() {
        let store = TaskStore::new();
        let applied = store.apply(&A2AEventData::StatusUpdate(TaskStatusUpdateEvent {
            task_id: "nope".to_owned(),
            context_id: "ctx-1".to_owned(),
            status: TaskStatus::new(TaskState::Completed),
            final_: true,
            metadata: None,
        }));
        assert!(applied.is_none());
        assert!(store.get("nope").is_none());
    }

    #[test]
    fn message_event_touches_no_task() {
        let store = TaskStore::new();
        assert!(store.apply(&A2AEventData::Message(Message::user("hi"))).is_none());
    }
}
