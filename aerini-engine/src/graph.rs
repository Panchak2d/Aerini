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

    pub fn edge_meta(&self, from: &str, to: &str) -> Option<&EdgeMeta> {
        let from_idx = self.index_map.get(from)?;
        let to_idx = self.index_map.get(to)?;
        let edge_idx = self.graph.find_edge(*from_idx, *to_idx)?;
        Some(&self.graph[edge_idx])
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
