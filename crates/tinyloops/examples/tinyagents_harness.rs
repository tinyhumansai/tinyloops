//! The same loop, driven by a harness instead of a `while` statement.
//!
//! [`simple_loop`](../examples/simple_loop.rs) keeps its control flow in Rust,
//! which is fine until the loop needs to be paused, checkpointed, resumed, or
//! observed. `TinyAgents` already owns that: its graph runtime is a durable
//! agent harness, so the loop becomes two nodes — `refine` runs one turn of the
//! workflow, `judge` decides whether to go round again — and the harness
//! supplies the stepping, the visit limits, and the seam a checkpointer plugs
//! into.
//!
//! The division of labour is the point. `TinyFlows` executes one graph and
//! decides nothing; `TinyAgents` decides what to run next and judges what came
//! back. Neither knows about the other, and this example is the whole of the
//! glue.
//!
//! `TinyAgents` is an optional dependency, so this example is gated:
//!
//! ```sh
//! cargo run -p tinyloops --features tinyagents --example tinyagents_harness
//! ```

use std::error::Error;
use std::sync::Arc;

use serde_json::{Value, json};
use tinyagents_graph::{END, GraphBuilder, NodeContext, NodeResult, TinyAgentsError};
use tinyflows::caps::Capabilities;
use tinyflows::caps::mock::mock_capabilities;
use tinyflows::compiler::CompiledWorkflow;
use tinyflows::compiler::compile;
use tinyflows::engine::run;
use tinyflows::model::{Edge, Node, NodeKind, WorkflowGraph};

/// The score at which the judge stops the loop.
const TARGET_SCORE: i64 = 5;

/// What one turn carries between the harness nodes.
#[derive(Clone, Debug)]
struct LoopState {
    /// The payload handed to the next workflow run.
    payload: Value,
    /// Turns completed so far, so the judge can enforce a budget of its own.
    turns: u32,
}

/// One turn of work, identical to the one `simple_loop` runs.
fn refine_step() -> WorkflowGraph {
    WorkflowGraph {
        nodes: vec![
            Node {
                id: "trigger".into(),
                kind: NodeKind::Trigger,
                type_version: 1,
                name: "turn".into(),
                config: Value::Null,
                ports: vec![],
                position: None,
            },
            Node {
                id: "refine".into(),
                kind: NodeKind::Transform,
                type_version: 1,
                name: "refine".into(),
                config: json!({ "set": { "score": "=.item.score + 1" } }),
                ports: vec![],
                position: None,
            },
        ],
        edges: vec![Edge {
            from_node: "trigger".into(),
            from_port: "main".into(),
            to_node: "refine".into(),
            to_port: "main".into(),
        }],
        ..Default::default()
    }
}

/// Runs the compiled workflow once and folds its output back into the state.
///
/// A workflow failure is reported as [`TinyAgentsError::Tool`]: from the
/// harness's point of view the engine is the tool this node called.
async fn refine(
    mut state: LoopState,
    workflow: Arc<CompiledWorkflow>,
    capabilities: Arc<Capabilities>,
) -> tinyagents_graph::Result<NodeResult<LoopState>> {
    let outcome = run(&workflow, state.payload.clone(), &capabilities)
        .await
        .map_err(|error| TinyAgentsError::Tool(format!("workflow run failed: {error}")))?;

    state.payload = outcome.output["nodes"]["refine"]["items"][0]["json"].clone();
    state.turns += 1;
    println!(
        "turn {}: score = {}",
        state.turns,
        state.payload["score"].as_i64().unwrap_or(0)
    );

    Ok(NodeResult::Update(state))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    // Compiled once and shared: the node handler is called on every turn, so it
    // holds handles rather than rebuilding the workflow each time.
    let workflow = Arc::new(compile(&refine_step())?);
    let capabilities = Arc::new(mock_capabilities());

    let loop_graph = GraphBuilder::<LoopState, LoopState>::overwrite()
        .add_node("refine", move |state: LoopState, _ctx: NodeContext| {
            let workflow = Arc::clone(&workflow);
            let capabilities = Arc::clone(&capabilities);
            refine(state, workflow, capabilities)
        })
        .set_entry("refine")
        // The judge. In a real loop this is where a model scores the work, or a
        // human approves it; the harness only needs a route label back.
        .add_conditional_edges(
            "refine",
            |state: &LoopState| {
                if state.payload["score"].as_i64().unwrap_or(0) >= TARGET_SCORE {
                    "done".to_string()
                } else {
                    "again".to_string()
                }
            },
            [("again", "refine"), ("done", END)],
        )
        .compile()?;

    let finished = loop_graph
        .run(LoopState {
            payload: json!({ "score": 0 }),
            turns: 0,
        })
        .await?;

    println!(
        "converged after {} turns with score {}",
        finished.state.turns,
        finished.state.payload["score"].as_i64().unwrap_or(0)
    );
    Ok(())
}
