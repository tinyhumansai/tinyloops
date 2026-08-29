//! The smallest loop this repository exists to run.
//!
//! A loop is a workflow graph compiled once and then run repeatedly, where each
//! run's output becomes the next run's input and a stop rule decides when the
//! work has converged. This example is that shape and nothing else: swap the
//! graph for a real one and the stop rule for a real judge, and the code around
//! them does not change.
//!
//! It runs offline against `tinyflows`' mock capabilities, so it is
//! deterministic and needs no provider credentials.
//!
//! Run it with:
//!
//! ```sh
//! cargo run -p tinyloops --example simple_loop
//! ```

use std::error::Error;

use serde_json::{Value, json};
use tinyflows::caps::mock::mock_capabilities;
use tinyflows::compiler::compile;
use tinyflows::engine::run;
use tinyflows::model::{Edge, Node, NodeKind, WorkflowGraph};

/// The score at which the loop considers the work good enough to stop.
const TARGET_SCORE: i64 = 5;

/// The hard turn budget. A loop without one is a way to spend an afternoon
/// discovering that a judge never says yes, so the budget is part of the
/// loop rather than something a caller remembers to add.
const MAX_TURNS: u32 = 8;

/// One turn of work: read the score handed in, and hand back a better one.
///
/// Every node config value prefixed with `=` is a jaq program evaluated against
/// `{ item, run }`, which is where a real loop would call a model, a tool, or a
/// sub-workflow instead of doing arithmetic.
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

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    // Compile once, outside the loop. A compiled workflow is the reusable
    // artifact; recompiling every turn would pay validation and lowering costs
    // for a graph that has not changed.
    let step = compile(&refine_step())?;
    let capabilities = mock_capabilities();

    let mut state = json!({ "score": 0 });
    let mut turns = 0;

    while turns < MAX_TURNS {
        let outcome = run(&step, state.clone(), &capabilities).await?;
        // The run state is keyed by node id; the loop carries forward what the
        // last node produced.
        state = outcome.output["nodes"]["refine"]["items"][0]["json"].clone();
        turns += 1;

        let score = state["score"].as_i64().unwrap_or(0);
        println!("turn {turns}: score = {score}");

        if score >= TARGET_SCORE {
            println!("converged after {turns} turns");
            return Ok(());
        }
    }

    println!("stopped at the {MAX_TURNS}-turn budget without converging");
    Ok(())
}
