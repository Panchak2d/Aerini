//! Execution graph — petgraph wrapper for workflow structure validation and ordering.
//!
//! [`ExecutionGraph::build`] consumes a [`crate::model::Workflow`] and produces:
//!
//! - A `DiGraph<String, EdgeMeta>` (node IDs as weights).
//! - A topological execution order (`topo_order`).
//! - The set of entry nodes (nodes with no incoming edges).
//!
//! Errors returned by `build()`:
//! - [`crate::error::EngineError::CycleDetected`] — workflow has a cycle.
//! - [`crate::error::EngineError::UnknownNodeReference`] — an edge references a node ID
//!   that does not exist in `workflow.nodes`.
//! - [`crate::error::EngineError::NoEntryNodes`] — every node has at least one incoming
//!   edge; the executor would have no starting point.
//!
//! `reachable_from()` runs BFS to determine which nodes are reachable from a given start.
//! The executor uses this to skip nodes that are downstream of a disabled or failing node
//! when fallback routing is not configured.

use petgraph::algo::{is_cyclic_directed, toposort};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::Bfs;
use std::collections::{HashMap, HashSet};

use crate::error::EngineError;
use crate::model::Workflow;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct EdgeMeta {
    pub edge_id: String,
    pub from_port: String,
    pub to_port: String,
    pub condition: Option<String>,
    pub on_success: Option<String>,
    pub on_failure: Option<String>,
}

pub struct ExecutionGraph {
    pub graph: DiGraph<String, EdgeMeta>,
    pub index_map: HashMap<String, NodeIndex>,
    pub entry_nodes: Vec<String>,
    pub topo_order: Vec<String>,
}

impl ExecutionGraph {
    pub fn build(workflow: &Workflow) -> Result<Self, EngineError> {
        let mut graph: DiGraph<String, EdgeMeta> = DiGraph::new();
        let mut index_map: HashMap<String, NodeIndex> = HashMap::new();

        for node in &workflow.nodes {
            let idx = graph.add_node(node.id.clone());
            index_map.insert(node.id.clone(), idx);
        }

        for edge in &workflow.edges {
            if !index_map.contains_key(&edge.from_node) {
                return Err(EngineError::UnknownNodeReference(edge.from_node.clone()));
            }
            if !index_map.contains_key(&edge.to_node) {
                return Err(EngineError::UnknownNodeReference(edge.to_node.clone()));
            }
            if let Some(ref s) = edge.on_success {
                if !index_map.contains_key(s) {
                    return Err(EngineError::UnknownNodeReference(s.clone()));
                }
            }
            if let Some(ref f) = edge.on_failure {
                if !index_map.contains_key(f) {
                    return Err(EngineError::UnknownNodeReference(f.clone()));
                }
            }
        }

        for edge in &workflow.edges {
            let from_idx = index_map[&edge.from_node];
            let to_idx = index_map[&edge.to_node];
            graph.add_edge(from_idx, to_idx, EdgeMeta {
                edge_id: edge.id.clone(),
                from_port: edge.from_port.clone(),
                to_port: edge.to_port.clone(),
                condition: edge.condition.clone(),
                on_success: edge.on_success.clone(),
                on_failure: edge.on_failure.clone(),
            });
        }

        if is_cyclic_directed(&graph) {
            let cycle_node = Self::find_cycle_node(&graph, &index_map);
            return Err(EngineError::CycleDetected(cycle_node));
        }

        let entry_nodes: Vec<String> = workflow.nodes.iter()
            .filter(|n| {
                let idx = index_map[&n.id];
                graph.neighbors_directed(idx, petgraph::Direction::Incoming).count() == 0
            })
            .map(|n| n.id.clone())
            .collect();

        if entry_nodes.is_empty() {
            return Err(EngineError::NoEntryNodes);
        }

        let reachable = Self::reachable_nodes(&graph, &index_map, &entry_nodes);
        for node in &workflow.nodes {
            if !reachable.contains(&node.id) {
                return Err(EngineError::UnreachableNode(node.id.clone()));
            }
        }

        let topo_indices = match toposort(&graph, None) {
            Ok(v) => v,
            Err(_) => return Err(EngineError::CycleDetected(
                "unexpected cycle detected during toposort — this is a bug, please report it".to_string()
            )),
        };

        let topo_order = topo_indices.iter()
            .map(|idx| graph[*idx].clone())
            .collect();

        Ok(ExecutionGraph { graph, index_map, entry_nodes, topo_order })
    }

    #[allow(dead_code)]
    pub fn successors(&self, node_id: &str) -> Vec<String> {
        match self.index_map.get(node_id) {
            None => vec![],
            Some(&idx) => self.graph
                .neighbors_directed(idx, petgraph::Direction::Outgoing)
                .map(|n| self.graph[n].clone())
                .collect(),
        }
    }

    /// Returns the IDs of all nodes that have an edge pointing TO `node_id`.
    /// Used by the parallel executor to determine when all upstream dependencies
    /// of a node have completed before scheduling it.
    pub fn predecessors(&self, node_id: &str) -> Vec<String> {
        match self.index_map.get(node_id) {
            None => vec![],
            Some(&idx) => self.graph
                .neighbors_directed(idx, petgraph::Direction::Incoming)
                .map(|n| self.graph[n].clone())
                .collect(),
        }
    }

    /// Returns the `EdgeMeta` for every edge from `from` to `to`, a workflow can
    /// have more than one edge between the same node pair (e.g. a Switch node's
    /// `case_1` and `default` ports both wired to the same downstream node).
    /// `petgraph::Graph` permits such parallel edges; a single-edge lookup via
    /// `find_edge` returns only one of them and can silently miss a sibling edge's
    /// `on_failure` (see `find_failure_route`'s use of this).
    /// Empty `Vec` (not `None`) when `from`/`to` don't exist or aren't connected.
    pub fn edge_meta(&self, from: &str, to: &str) -> Vec<&EdgeMeta> {
        let (from_idx, to_idx) = match (self.index_map.get(from), self.index_map.get(to)) {
            (Some(f), Some(t)) => (*f, *t),
            _ => return Vec::new(),
        };
        self.graph
            .edges_connecting(from_idx, to_idx)
            .map(|edge_ref| edge_ref.weight())
            .collect()
    }

    fn reachable_nodes(
        graph: &DiGraph<String, EdgeMeta>,
        index_map: &HashMap<String, NodeIndex>,
        entry_nodes: &[String],
    ) -> HashSet<String> {
        let mut reachable = HashSet::new();
        for entry in entry_nodes {
            let start = index_map[entry];
            let mut bfs = Bfs::new(graph, start);
            while let Some(nx) = bfs.next(graph) {
                reachable.insert(graph[nx].clone());
            }
        }
        reachable
    }

    fn find_cycle_node(
        graph: &DiGraph<String, EdgeMeta>,
        index_map: &HashMap<String, NodeIndex>,
    ) -> String {
        for (id, &idx) in index_map {
            let neighbors: Vec<NodeIndex> = graph
                .neighbors_directed(idx, petgraph::Direction::Outgoing)
                .collect();
            for neighbor in neighbors {
                if petgraph::algo::has_path_connecting(graph, neighbor, idx, None) {
                    return id.clone();
                }
            }
        }
        "unknown".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::CURRENT_VERSION;
    use crate::model::{NodeType, Workflow, WorkflowEdge, WorkflowNode};

    fn test_node(id: &str) -> WorkflowNode {
        WorkflowNode {
            id: id.to_string(),
            node_type_id: "instant_test".to_string(),
            node_type: NodeType::Utility,
            name: id.to_string(),
            config: serde_json::json!({}),
            credentials: HashMap::new(),
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
            retry: Default::default(),
            fallback_node: None,
            disabled: false,
            position: Default::default(),
        }
    }

    fn test_edge(id: &str, from: &str, to: &str, on_failure: Option<&str>) -> WorkflowEdge {
        WorkflowEdge {
            id: id.to_string(),
            from_node: from.to_string(),
            from_port: "output".to_string(),
            to_node: to.to_string(),
            to_port: "input".to_string(),
            condition: None,
            on_success: None,
            on_failure: on_failure.map(|s| s.to_string()),
        }
    }

    fn test_workflow(nodes: Vec<WorkflowNode>, edges: Vec<WorkflowEdge>) -> Workflow {
        Workflow {
            schema_version: CURRENT_VERSION.to_string(),
            id: "wf_test".to_string(),
            name: "Test".to_string(),
            description: String::new(),
            nodes,
            edges,
            metadata: Default::default(),
            max_duration_secs: None,
            unlimited_duration: false,
            parallel_execution: false,
            max_concurrent_nodes: None,
            settings: Default::default(),
        }
    }

    #[test]
    fn edge_meta_normal_single_edge() {
        let wf = test_workflow(
            vec![test_node("a"), test_node("b")],
            vec![test_edge("e1", "a", "b", None)],
        );
        let g = ExecutionGraph::build(&wf).expect("graph should build");
        let metas = g.edge_meta("a", "b");
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].edge_id, "e1");
    }

    #[test]
    fn edge_meta_no_matching_edge_returns_empty_vec() {
        let wf = test_workflow(
            vec![test_node("a"), test_node("b")],
            vec![test_edge("e1", "a", "b", None)],
        );
        let g = ExecutionGraph::build(&wf).expect("graph should build");
        assert!(g.edge_meta("b", "a").is_empty(), "reverse direction has no edge");
        assert!(g.edge_meta("a", "nope").is_empty(), "unknown target node");
        assert!(g.edge_meta("nope", "a").is_empty(), "unknown source node");
    }

    // two edges between the same (from, to) pair must BOTH be returned by edge_meta, not just whichever one an internal
    // find_edge()-style single lookup happened to pick.
    #[test]
    fn edge_meta_returns_all_parallel_edges_between_same_pair() {
        let wf = test_workflow(
            vec![test_node("a"), test_node("b"), test_node("rec1"), test_node("rec2")],
            vec![
                test_edge("e1", "a", "b", Some("rec1")),
                test_edge("e2", "a", "b", Some("rec2")),
            ],
        );
        let g = ExecutionGraph::build(&wf).expect("graph should build");
        let metas = g.edge_meta("a", "b");
        assert_eq!(metas.len(), 2, "both parallel edges must be returned");
        let failures: HashSet<String> = metas
            .iter()
            .filter_map(|m| m.on_failure.clone())
            .collect();
        assert!(failures.contains("rec1"));
        assert!(failures.contains("rec2"));
    }
}
