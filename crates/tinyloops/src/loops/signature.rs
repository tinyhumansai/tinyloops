//! The signature a checkpoint records, and the resume it refuses.
//!
//! Invariant 9 of `docs/specs/loop-kernel.md`. The graph is generated *from*
//! the thresholds, so changing a constant changes the topology. Resuming a
//! checkpoint taken against the old topology onto the new one restores state
//! into slots that no longer mean what they meant — silent corruption rather
//! than a crash — so every checkpoint carries a hash over the emitted graph and
//! a resume that does not match is refused by name.
//!
//! # Why SHA-256 and not `DefaultHasher`
//!
//! `std::hash::DefaultHasher` is documented as unstable across releases: the
//! same input may hash differently after a toolchain bump. A signature built on
//! it would refuse resumes after an upgrade rather than after a change of
//! shape, which inverts the property this exists to provide. `sha2` is here for
//! that one reason.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tinyflows::model::WorkflowGraph;

use crate::{Error, Result};

/// A hash over an emitted graph's topology and every rendered threshold.
///
/// Covers node ids, kinds, ports, configuration — which is where the rendered
/// thresholds live, in the head's `until` and the routing switch's expression —
/// and every edge with both of its ports. It deliberately does not cover the
/// graph's name or id, which name the workflow rather than describe its shape.
///
/// # Examples
///
/// ```
/// # use tinyflows::model::WorkflowGraph;
/// # use tinyloops::GraphSignature;
/// let graph = WorkflowGraph::default();
/// assert_eq!(GraphSignature::of(&graph), GraphSignature::of(&graph));
/// assert_eq!(GraphSignature::of(&graph).as_str().len(), 64);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GraphSignature(String);

impl GraphSignature {
    /// Hashes `graph`.
    ///
    /// The digest is taken over canonical JSON: `serde_json`'s object
    /// representation orders keys, and the node and edge lists are hashed in
    /// the order the builder emitted them, which is fixed by
    /// [`NodeIds`](super::NodeIds) rather than by allocation. Two builds of one
    /// specification therefore hash identically, which is the property the
    /// refusal below rests on.
    #[must_use]
    pub fn of(graph: &WorkflowGraph) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(canonical(graph).to_string().as_bytes());
        let digest = hasher.finalize();
        let mut hex = String::with_capacity(digest.len() * 2);
        for byte in digest {
            // `write!` into a `String` cannot fail, but it returns a `Result`
            // that would have to be discarded; two hex nibbles are cheaper to
            // spell out than a swallowed error.
            hex.push(nibble(byte >> 4));
            hex.push(nibble(byte & 0x0f));
        }
        Self(hex)
    }

    /// The hex digest.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for GraphSignature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One lowercase hex digit for a nibble.
fn nibble(value: u8) -> char {
    char::from_digit(u32::from(value), 16).unwrap_or('0')
}

/// The canonical JSON a signature is taken over.
fn canonical(graph: &WorkflowGraph) -> Value {
    let nodes: Vec<Value> = graph
        .nodes
        .iter()
        .map(|node| {
            json!({
                "id": node.id,
                // Through serde so the hash reads the same discriminator the
                // graph JSON does, rather than a `Debug` rendering that a
                // variant rename would change without changing the wire form.
                "kind": serde_json::to_value(&node.kind).unwrap_or(Value::Null),
                "type_version": node.type_version,
                "ports": node.ports.iter().map(|port| port.name.clone()).collect::<Vec<_>>(),
                "config": node.config.clone(),
            })
        })
        .collect();
    let edges: Vec<Value> = graph
        .edges
        .iter()
        .map(|edge| {
            json!({
                "from_node": edge.from_node,
                "from_port": edge.from_port,
                "to_node": edge.to_node,
                "to_port": edge.to_port,
            })
        })
        .collect();
    json!({ "schema_version": graph.schema_version, "nodes": nodes, "edges": edges })
}

/// Checks a checkpoint's recorded signature against the graph about to run it.
///
/// Call this *before* the run, not inside it: the point is that a mismatched
/// resume runs no node at all.
///
/// # Errors
///
/// Returns [`Error::GraphSignatureMismatch`], naming both signatures, when
/// `recorded` is not the signature of `graph`.
///
/// # Examples
///
/// ```
/// # use tinyflows::model::{Node, NodeKind, WorkflowGraph};
/// # use tinyloops::{Error, GraphSignature, verify_resume};
/// let graph = WorkflowGraph::default();
/// let recorded = GraphSignature::of(&graph);
/// assert!(verify_resume(&recorded, &graph).is_ok());
///
/// let mut changed = graph.clone();
/// changed.nodes.push(Node {
///     id: "t".to_string(),
///     kind: NodeKind::Trigger,
///     type_version: 1,
///     name: "start".to_string(),
///     config: serde_json::Value::Null,
///     ports: Vec::new(),
///     position: None,
/// });
/// assert!(matches!(
///     verify_resume(&recorded, &changed),
///     Err(Error::GraphSignatureMismatch { .. }),
/// ));
/// ```
pub fn verify_resume(recorded: &GraphSignature, graph: &WorkflowGraph) -> Result<()> {
    let current = GraphSignature::of(graph);
    if *recorded == current {
        return Ok(());
    }
    Err(Error::GraphSignatureMismatch {
        recorded: recorded.as_str().to_string(),
        current: current.as_str().to_string(),
    })
}
