//! Directed edges between nodes (`strands-ts/src/multiagent/edge.ts`).

use std::sync::Arc;

use futures::future::BoxFuture;

use super::nodes::Node;
use super::state::MultiAgentState;

/// Condition deciding whether an edge is traversed, given the current state (TS `EdgeHandler`).
///
/// Async-capable: the handler returns a future so conditions can await I/O.
pub type EdgeHandler = Arc<dyn for<'a> Fn(&'a MultiAgentState) -> BoxFuture<'a, bool> + Send + Sync>;

/// Wraps a synchronous predicate as an [`EdgeHandler`].
pub fn edge_handler(predicate: impl Fn(&MultiAgentState) -> bool + Send + Sync + 'static) -> EdgeHandler {
    Arc::new(move |state| {
        let traverse = predicate(state);
        Box::pin(async move { traverse })
    })
}

/// A directed edge between two nodes, traversed unconditionally unless it carries a handler.
#[derive(Clone)]
pub struct Edge {
    /// The node the edge leaves.
    pub source: Arc<dyn Node>,
    /// The node the edge enters.
    pub target: Arc<dyn Node>,
    /// Condition gating the edge; `None` means "always traverse".
    pub handler: Option<EdgeHandler>,
}

impl Edge {
    /// Edge from `source` to `target`, always traversed.
    pub fn new(source: Arc<dyn Node>, target: Arc<dyn Node>) -> Self {
        Self {
            source,
            target,
            handler: None,
        }
    }

    /// Gate this edge on `handler`.
    pub fn with_handler(mut self, handler: EdgeHandler) -> Self {
        self.handler = Some(handler);
        self
    }

    /// Whether this edge should be traversed in `state`.
    pub async fn should_traverse(&self, state: &MultiAgentState) -> bool {
        match &self.handler {
            Some(handler) => handler(state).await,
            None => true,
        }
    }
}

/// An edge as accepted by an orchestrator builder (TS `EdgeDefinition`), identifying its nodes by id.
#[derive(Clone, Default)]
pub struct EdgeDefinition {
    /// Id of the source node.
    pub source: String,
    /// Id of the target node.
    pub target: String,
    /// Condition gating the edge; `None` means "always traverse".
    pub handler: Option<EdgeHandler>,
}

impl EdgeDefinition {
    /// Unconditional edge from `source` to `target`.
    pub fn new(source: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
            handler: None,
        }
    }

    /// Gate this edge on `handler`.
    pub fn with_handler(mut self, handler: EdgeHandler) -> Self {
        self.handler = Some(handler);
        self
    }
}

impl<S: Into<String>, T: Into<String>> From<(S, T)> for EdgeDefinition {
    fn from((source, target): (S, T)) -> Self {
        Self::new(source, target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::multiagent::nodes::AgentNode;
    use crate::multiagent::state::Status;
    use crate::test_support::MockMessageModel;

    fn node() -> Arc<dyn Node> {
        let agent = Agent::builder()
            .model(MockMessageModel::shared(vec![]))
            .build()
            .unwrap();
        Arc::new(AgentNode::new(agent).unwrap())
    }

    fn source_completed() -> EdgeHandler {
        edge_handler(|state: &MultiAgentState| {
            state
                .nodes()
                .values()
                .any(|node| node.status == Status::Completed)
        })
    }

    #[tokio::test]
    async fn an_edge_without_a_handler_is_always_traversed() {
        let edge = Edge::new(node(), node());
        assert!(edge.handler.is_none());
        assert!(edge.should_traverse(&MultiAgentState::new(["a"])).await);
    }

    #[tokio::test]
    async fn a_handler_decides_whether_the_edge_is_traversed() {
        let edge = Edge::new(node(), node()).with_handler(source_completed());
        let mut state = MultiAgentState::new(["a"]);

        assert!(
            !edge.should_traverse(&state).await,
            "a pending source keeps it closed"
        );

        state.node_or_insert("a").status = Status::Completed;
        assert!(edge.should_traverse(&state).await);
    }

    #[tokio::test]
    async fn edge_definitions_convert_from_an_id_pair_and_carry_handlers() {
        let definition: EdgeDefinition = ("a", "b").into();
        assert_eq!(
            (definition.source.as_str(), definition.target.as_str()),
            ("a", "b")
        );
        assert!(definition.handler.is_none());
        assert!(EdgeDefinition::new("a", "b")
            .with_handler(source_completed())
            .handler
            .is_some());
    }
}
