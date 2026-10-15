//! Node kinds shared by the multi-agent hook and stream events.
//!
//! The `Node` trait and the concrete node types land here next to [`NodeType`].

use serde::de::{Deserialize, Deserializer};
use serde::{Serialize, Serializer};

/// What a node wraps: an agent, a nested orchestrator, or a custom implementation
/// (TS `NodeType = 'agentNode' | 'multiAgentNode' | (string & {})`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeType {
    /// The node wraps an `Agent`.
    Agent,
    /// The node wraps a nested orchestrator.
    MultiAgent,
    /// A custom node type, identified by its own name.
    Custom(String),
}

impl NodeType {
    /// The TS wire string for this node type.
    pub fn as_str(&self) -> &str {
        match self {
            NodeType::Agent => "agentNode",
            NodeType::MultiAgent => "multiAgentNode",
            NodeType::Custom(name) => name,
        }
    }
}

impl From<&str> for NodeType {
    fn from(name: &str) -> Self {
        match name {
            "agentNode" => NodeType::Agent,
            "multiAgentNode" => NodeType::MultiAgent,
            other => NodeType::Custom(other.to_owned()),
        }
    }
}

impl Serialize for NodeType {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for NodeType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(NodeType::from(String::deserialize(deserializer)?.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_known_and_custom_node_types() {
        for (ty, wire) in [
            (NodeType::Agent, "agentNode"),
            (NodeType::MultiAgent, "multiAgentNode"),
            (NodeType::Custom("a2aNode".to_owned()), "a2aNode"),
        ] {
            assert_eq!(serde_json::to_value(&ty).unwrap(), serde_json::json!(wire));
            assert_eq!(
                serde_json::from_value::<NodeType>(serde_json::json!(wire)).unwrap(),
                ty
            );
        }
    }
}
