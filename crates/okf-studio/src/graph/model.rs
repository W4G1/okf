//! Extraction of the graph model from a loaded bundle: the same edges the
//! `okf graph` command prints, kept as typed nodes and edges so the canvas
//! can style them distinctly.

use okf_core::{Bundle, ConceptId, ResourceKind};
use std::collections::HashMap;

/// What a node represents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    /// An ordinary concept.
    Concept,
    /// An Attested Computation concept.
    Computation,
    /// A phantom node: the target of a broken link, not present on disk.
    Phantom,
    /// An external source (URL or scope descriptor from `sources`).
    Source,
}

/// One node in the graph.
#[derive(Clone, Debug)]
pub struct GraphNode {
    /// A stable string key (the concept id, raw link target, or source
    /// label). Layout positions persist across snapshots under this key.
    pub key: String,
    /// The display label.
    pub label: String,
    /// The node's kind.
    pub kind: NodeKind,
    /// The concept id, when the node is a concept.
    pub id: Option<ConceptId>,
    /// Total degree, used for label prioritization and hub repulsion.
    pub degree: usize,
}

/// The kind of an edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    /// A markdown cross-link.
    Link,
    /// A derivation edge (`sources[].resource` naming another concept).
    Derivation,
    /// A link whose target does not exist.
    Broken,
    /// An edge from a concept to one of its external sources.
    Source,
}

/// One directed edge, as indices into [`GraphModel::nodes`].
#[derive(Clone, Copy, Debug)]
pub struct GraphEdge {
    /// Index of the source node.
    pub from: usize,
    /// Index of the target node.
    pub to: usize,
    /// The edge's kind.
    pub kind: EdgeKind,
}

/// The extracted graph.
#[derive(Clone, Debug, Default)]
pub struct GraphModel {
    /// All nodes. Concepts first, then phantom targets, then sources.
    pub nodes: Vec<GraphNode>,
    /// All edges.
    pub edges: Vec<GraphEdge>,
}

impl GraphModel {
    /// Builds the model from a bundle.
    #[must_use]
    pub fn build(bundle: &Bundle) -> Self {
        let mut nodes: Vec<GraphNode> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();

        for concept in bundle.concepts() {
            let key = concept.id.to_string();
            let kind = if concept.attested_computation().is_some() {
                NodeKind::Computation
            } else {
                NodeKind::Concept
            };
            index.insert(key.clone(), nodes.len());
            nodes.push(GraphNode {
                label: key.clone(),
                key,
                kind,
                id: Some(concept.id.clone()),
                degree: 0,
            });
        }

        let mut edges: Vec<GraphEdge> = Vec::new();
        for concept in bundle.concepts() {
            let from = index[&concept.id.to_string()];
            for link in bundle.links_from(&concept.id) {
                if link.exists {
                    if let Some(&to) = index.get(&link.target.to_string()) {
                        edges.push(GraphEdge {
                            from,
                            to,
                            kind: EdgeKind::Link,
                        });
                    }
                } else {
                    let key = format!("✗{}", link.target);
                    let to = *index.entry(key.clone()).or_insert_with(|| {
                        nodes.push(GraphNode {
                            label: link.target.to_string(),
                            key,
                            kind: NodeKind::Phantom,
                            id: None,
                            degree: 0,
                        });
                        nodes.len() - 1
                    });
                    edges.push(GraphEdge {
                        from,
                        to,
                        kind: EdgeKind::Broken,
                    });
                }
            }
            for source in bundle.sources_of(&concept.id) {
                if let Some(target) = &source.concept {
                    if let Some(&to) = index.get(&target.to_string()) {
                        edges.push(GraphEdge {
                            from,
                            to,
                            kind: EdgeKind::Derivation,
                        });
                    }
                } else if matches!(
                    source.source.resource_kind(),
                    ResourceKind::Url | ResourceKind::Scope | ResourceKind::Path
                ) {
                    let label = source.source.label().to_string();
                    let key = format!("src:{label}");
                    let to = *index.entry(key.clone()).or_insert_with(|| {
                        nodes.push(GraphNode {
                            label,
                            key,
                            kind: NodeKind::Source,
                            id: None,
                            degree: 0,
                        });
                        nodes.len() - 1
                    });
                    edges.push(GraphEdge {
                        from,
                        to,
                        kind: EdgeKind::Source,
                    });
                }
            }
        }

        for edge in &edges {
            nodes[edge.from].degree += 1;
            nodes[edge.to].degree += 1;
        }

        Self { nodes, edges }
    }

    /// The node index for a concept id, if present.
    #[must_use]
    pub fn node_of(&self, id: &ConceptId) -> Option<usize> {
        self.nodes.iter().position(|n| n.id.as_ref() == Some(id))
    }

    /// The set of node indices within `k` hops of `center` (undirected),
    /// including `center` itself.
    #[must_use]
    pub fn neighborhood(&self, center: usize, k: usize) -> Vec<bool> {
        let mut included = vec![false; self.nodes.len()];
        if center >= self.nodes.len() {
            return included;
        }
        included[center] = true;
        let mut frontier = vec![center];
        for _ in 0..k {
            let mut next = Vec::new();
            for edge in &self.edges {
                for (a, b) in [(edge.from, edge.to), (edge.to, edge.from)] {
                    if frontier.contains(&a) && !included[b] {
                        included[b] = true;
                        next.push(b);
                    }
                }
            }
            frontier = next;
        }
        included
    }
}
