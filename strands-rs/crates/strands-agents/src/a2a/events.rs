//! Stream events yielded by [`A2AAgent::stream`](super::A2AAgent::stream) (`a2a/events.ts`).

use serde::Serialize;

use super::types::A2AEventData;
use crate::agent::AgentResult;

/// A raw A2A protocol event received from a remote agent.
///
/// Yielded for every event of a `message/stream` subscription: a `Message`, a `Task`, a
/// `status-update` or an `artifact-update`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename = "a2aStreamUpdateEvent")]
#[non_exhaustive]
pub struct A2AStreamUpdateEvent {
    /// The protocol event.
    pub event: A2AEventData,
}

/// The final event of an [`A2AAgent`](super::A2AAgent) stream, carrying the assembled result.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename = "a2aResultEvent")]
#[non_exhaustive]
pub struct A2AResultEvent {
    /// The result built from the last complete protocol event.
    pub result: AgentResult,
}

/// An event yielded by [`A2AAgent::stream`](super::A2AAgent::stream); [`Self::Result`] is always
/// last.
#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum A2AStreamEvent {
    /// A raw A2A protocol event.
    StreamUpdate(A2AStreamUpdateEvent),
    /// The invocation's result.
    Result(A2AResultEvent),
}

impl A2AStreamEvent {
    /// Short event name, matching the TS `type` discriminator.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::StreamUpdate(_) => "a2aStreamUpdateEvent",
            Self::Result(_) => "a2aResultEvent",
        }
    }

    /// The protocol event, when this is a stream update.
    pub fn event(&self) -> Option<&A2AEventData> {
        match self {
            Self::StreamUpdate(e) => Some(&e.event),
            Self::Result(_) => None,
        }
    }

    /// The final result, when this is the result event.
    pub fn result(&self) -> Option<&AgentResult> {
        match self {
            Self::Result(e) => Some(&e.result),
            Self::StreamUpdate(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::types::{TaskState, TaskStatus, TaskStatusUpdateEvent};
    use crate::agent::InvocationState;
    use crate::types::{ContentBlock, Message, Role, StopReason};
    use serde_json::json;

    fn status_update() -> A2AEventData {
        A2AEventData::StatusUpdate(TaskStatusUpdateEvent {
            task_id: "task-1".to_owned(),
            context_id: "ctx-1".to_owned(),
            status: TaskStatus::new(TaskState::Working),
            final_: false,
            metadata: None,
        })
    }

    fn agent_result() -> AgentResult {
        AgentResult::new(
            StopReason::EndTurn,
            Message::new(Role::Assistant, vec![ContentBlock::text("Done")]),
            InvocationState::new(),
        )
    }

    #[test]
    fn a_stream_update_event_carries_the_protocol_event() {
        let event = A2AStreamEvent::StreamUpdate(A2AStreamUpdateEvent {
            event: status_update(),
        });
        assert_eq!(event.type_name(), "a2aStreamUpdateEvent");
        assert_eq!(event.event(), Some(&status_update()));
        assert!(event.result().is_none());
    }

    #[test]
    fn a_stream_update_event_serializes_with_its_type_and_event() {
        let event = A2AStreamUpdateEvent {
            event: status_update(),
        };
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            json!({
                "type": "a2aStreamUpdateEvent",
                "event": {
                    "kind": "status-update",
                    "taskId": "task-1",
                    "contextId": "ctx-1",
                    "status": { "state": "working" },
                    "final": false,
                },
            })
        );
    }

    #[test]
    fn a_result_event_carries_the_agent_result() {
        let event = A2AStreamEvent::Result(A2AResultEvent {
            result: agent_result(),
        });
        assert_eq!(event.type_name(), "a2aResultEvent");
        assert!(event.event().is_none());
        assert_eq!(event.result().unwrap().text(), "Done");
    }

    #[test]
    fn a_result_event_serializes_with_its_type_and_result() {
        let json = serde_json::to_value(A2AResultEvent {
            result: agent_result(),
        })
        .unwrap();
        assert_eq!(json["type"], "a2aResultEvent");
        assert_eq!(json["result"]["type"], "agentResult");
        assert_eq!(json["result"]["stopReason"], "endTurn");
        assert_eq!(json["result"]["lastMessage"]["content"][0]["text"], "Done");
    }
}
