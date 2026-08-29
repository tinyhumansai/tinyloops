//! The builder: one specification in, one `WorkflowGraph` out.
//!
//! Pure. No clock, no randomness, no I/O — the same inputs emit the same bytes,
//! which is what makes [`GraphSignature`](super::GraphSignature) a statement
//! about the graph rather than about the moment it was built.

use serde_json::{Value, json};
use tinyflows::model::{Edge as GraphEdge, Node, NodeKind, Port, WorkflowGraph};

use super::termination::TerminationCondition;
use super::types::{NodeIds, payload_address};
use crate::arm::ArmSet;
use crate::policy::{Autonomy, Route, Thresholds, ladder};
use crate::state::LoopState;
use crate::step::{
    RUN_LOOP_STEP, STEP_ATTEMPT, STEP_PASS, STEP_PLAN, STEP_REPORT, STEP_RESEARCH, StepRegistry,
};
use crate::{Error, Result};

/// The step the merge barrier runs: fold every arm's whole state onto the base
/// the pass started from.
///
/// Not one of [`STEP_NAMES`](crate::STEP_NAMES), which was written for the
/// bodies a pass runs in sequence and predates the barrier. That is a loud
/// difference rather than a quiet one: [`LoopBuilder::build`] resolves every
/// step it emits against the registry, so a registry populated only from
/// `STEP_NAMES` fails to build with [`Error::UnknownStep`] naming this step,
/// which is the closed set doing its job at build time.
pub const STEP_MERGE: &str = "merge";

/// The five routes, in the order the ladder tests them.
///
/// Written as values rather than as strings so the emitted port names come from
/// [`Route::as_str`] — the same function the generated jq emits — and a rename
/// cannot leave a port nothing routes to.
const ROUTES: [Route; 5] = [
    Route::Blocked,
    Route::Solved,
    Route::Reported,
    Route::Diversify,
    Route::Retry,
];

/// The port a switch falls back to when its expression produces nothing a port
/// is named for.
///
/// Wired to `pass` like every real route. Under this engine a jq program that
/// fails to compile yields `null` silently, and `null` routes here; sending it
/// to the node every route already enters means a broken ladder costs a pass
/// rather than stranding the run mid-body with no node to activate.
const DEFAULT_PORT: &str = "default";

/// Builds the one graph a goal run is.
///
/// # Examples
///
/// ```
/// # use std::sync::Arc;
/// # use serde_json::Value;
/// # use tinyloops::{
/// #     Advanced, Arm, ArmOutcome, ArmSet, Autonomy, CanWrite, LoopBuilder, LoopState, NoWrite,
/// #     Result, Step, StepContext, StepRegistry, Thresholds,
/// # };
/// # struct Body(&'static str);
/// # impl Step for Body {
/// #     fn name(&self) -> &'static str { self.0 }
/// #     fn run(&self, state: LoopState, ctx: StepContext<'_, CanWrite>) -> Result<Advanced> {
/// #         Ok(ctx.advance(state))
/// #     }
/// # }
/// # impl Arm for Body {
/// #     fn name(&self) -> &'static str { self.0 }
/// #     fn evaluate(
/// #         &self,
/// #         base: &LoopState,
/// #         _report: &Value,
/// #         _ctx: StepContext<'_, NoWrite>,
/// #     ) -> Result<ArmOutcome> {
/// #         Ok(ArmOutcome::unchanged(self.name(), base))
/// #     }
/// # }
/// # let mut registry = StepRegistry::new();
/// # for name in ["plan", "research", "attempt", "merge", "pass", "report", "reflect", "judge"] {
/// #     registry.register(Arc::new(Body(match name {
/// #         "plan" => "plan", "research" => "research", "attempt" => "attempt",
/// #         "merge" => "merge", "pass" => "pass", "report" => "report",
/// #         "reflect" => "reflect", _ => "judge",
/// #     })))?;
/// # }
/// let arms = ArmSet::new(vec![
///     Arc::new(Body("reflect")) as Arc<dyn Arm>,
///     Arc::new(Body("judge")),
/// ])?;
///
/// let graph = LoopBuilder::new(Thresholds::default(), arms, registry)
///     .goal("ship the release")
///     .autonomy(Autonomy::Unattended)
///     .build()?;
///
/// assert!(tinyflows::compiler::compile(&graph).is_ok());
/// # Ok::<(), tinyloops::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct LoopBuilder {
    thresholds: Thresholds,
    arms: ArmSet,
    registry: StepRegistry,
    ids: NodeIds,
    autonomy: Autonomy,
    goal: String,
    termination: TerminationCondition,
    name: String,
}

impl LoopBuilder {
    /// A builder for one loop.
    ///
    /// The autonomy defaults to [`Autonomy::Report`], the same default the
    /// policy layer carries and for the same reason: the conservative setting
    /// is the one that is safe to get wrong. A `Report` graph plans, researches,
    /// stands down, and reports; it emits no attempt, no arms, and no loop. Ask
    /// for [`Autonomy::Assisted`] or [`Autonomy::Unattended`] to get a graph
    /// that acts.
    #[must_use]
    pub fn new(thresholds: Thresholds, arms: ArmSet, registry: StepRegistry) -> Self {
        Self {
            thresholds,
            arms,
            registry,
            ids: NodeIds::default(),
            autonomy: Autonomy::default(),
            goal: String::new(),
            termination: TerminationCondition::default(),
            name: "tinyloops goal run".to_string(),
        }
    }

    /// The goal the run seeds its accumulator with.
    #[must_use]
    pub fn goal(mut self, goal: impl Into<String>) -> Self {
        self.goal = goal.into();
        self
    }

    /// How much the run may do without asking.
    #[must_use]
    pub fn autonomy(mut self, autonomy: Autonomy) -> Self {
        self.autonomy = autonomy;
        self
    }

    /// The node ids to emit under, for a host running two loops in one graph.
    #[must_use]
    pub fn ids(mut self, ids: NodeIds) -> Self {
        self.ids = ids;
        self
    }

    /// The stop test the loop head carries.
    #[must_use]
    pub fn termination(mut self, termination: TerminationCondition) -> Self {
        self.termination = termination;
        self
    }

    /// The emitted workflow's name.
    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Emits the graph.
    ///
    /// # Errors
    ///
    /// - [`Error::UnknownStep`] when a node would name a step the registry does
    ///   not hold. The closed step set is checked here as well as at call time,
    ///   because a graph naming a step that does not exist runs green, changes
    ///   nothing, and routes on a state nobody advanced.
    /// - [`Error::StateEncoding`] when the seed accumulator cannot be
    ///   serialized into `config.state.init`.
    /// - [`Error::InvalidLoopGraph`] when the emitted graph does not pass the
    ///   engine's own structural validation.
    pub fn build(self) -> Result<WorkflowGraph> {
        let seed = serde_json::to_value(LoopState::new(self.goal.clone()))
            .map_err(|_| Error::StateEncoding)?;

        let (nodes, edges) = if self.autonomy == Autonomy::Report {
            self.dry_run_shape(&seed)
        } else {
            self.loop_shape(&seed)
        };

        for node in &nodes {
            if let Some(step) = node
                .config
                .get("args")
                .and_then(|args| args.get("step"))
                .and_then(Value::as_str)
            {
                self.registry.get(step)?;
            }
        }

        let graph = WorkflowGraph {
            name: self.name.clone(),
            nodes,
            edges,
            ..WorkflowGraph::default()
        };

        tinyflows::validate::validate(&graph).map_err(|error| Error::InvalidLoopGraph {
            reason: error.to_string(),
        })?;

        Ok(graph)
    }

    /// The shape a run that may act emits.
    fn loop_shape(&self, seed: &Value) -> (Vec<Node>, Vec<GraphEdge>) {
        let ids = self.ids;
        let mut nodes = self.preamble(seed);
        let mut edges = vec![
            edge(ids.trigger, ids.plan),
            edge(ids.plan, ids.research),
            edge(ids.research, ids.side_arms),
            edge(ids.side_arms, ids.loop_head),
        ];

        nodes.push(self.head());

        // The body's first node. At `Assisted` an approval sits in front of the
        // attempt, so the topology says what may happen without asking — a
        // prompt instruction is not a control.
        if self.autonomy == Autonomy::Assisted {
            nodes.push(approval(ids.approval));
            edges.push(GraphEdge {
                from_node: ids.loop_head.to_string(),
                from_port: "body".to_string(),
                to_node: ids.approval.to_string(),
                to_port: "main".to_string(),
            });
            edges.push(GraphEdge {
                from_node: ids.approval.to_string(),
                from_port: "approved".to_string(),
                to_node: ids.attempt.to_string(),
                to_port: "main".to_string(),
            });
            // A refused pass still closes through `pass`, so the head counts it
            // and `max_iterations` still bounds the run.
            edges.push(GraphEdge {
                from_node: ids.approval.to_string(),
                from_port: "rejected".to_string(),
                to_node: ids.pass.to_string(),
                to_port: "main".to_string(),
            });
        } else {
            edges.push(GraphEdge {
                from_node: ids.loop_head.to_string(),
                from_port: "body".to_string(),
                to_node: ids.attempt.to_string(),
                to_port: "main".to_string(),
            });
        }

        // The attempt reads the accumulator the head folded at the top of this
        // pass, so it is current. An *arm* may not: the head folds at the top,
        // so anywhere further into the body the accumulator is one pass behind.
        nodes.push(tool_call(
            ids.attempt,
            STEP_ATTEMPT,
            json!({ "state": ids.accumulator_address() }),
        ));

        // Invariant 6: the fan-out edges and the fold the barrier performs are
        // derived from one `ArmSet`, so "every arm converges" and "every arm is
        // folded" are one fact rather than two that can drift.
        let mut fold_inputs = serde_json::Map::new();
        for arm in self.arms.names() {
            nodes.push(tool_call(
                arm,
                arm,
                json!({ "report": payload_address(ids.attempt) }),
            ));
            edges.push(edge(ids.attempt, arm));
            edges.push(edge(arm, ids.merge));
            fold_inputs.insert(arm.to_string(), Value::String(payload_address(arm)));
        }

        nodes.push(tool_call(
            ids.merge,
            STEP_MERGE,
            json!({
                "base": payload_address(ids.attempt),
                "arms": Value::Object(fold_inputs),
            }),
        ));
        edges.push(edge(ids.merge, ids.route));

        nodes.push(self.routing_switch());
        for route in ROUTES {
            edges.push(GraphEdge {
                from_node: ids.route.to_string(),
                from_port: route.as_str().to_string(),
                to_node: ids.pass.to_string(),
                to_port: "main".to_string(),
            });
        }
        edges.push(GraphEdge {
            from_node: ids.route.to_string(),
            from_port: DEFAULT_PORT.to_string(),
            to_node: ids.pass.to_string(),
            to_port: "main".to_string(),
        });

        nodes.push(tool_call(
            ids.pass,
            STEP_PASS,
            json!({ "state": payload_address(ids.merge) }),
        ));
        // The one edge back to the head, and the reason `pass` exists.
        edges.push(edge(ids.pass, ids.loop_head));

        edges.push(GraphEdge {
            from_node: ids.loop_head.to_string(),
            from_port: "done".to_string(),
            to_node: ids.stand_down.to_string(),
            to_port: "main".to_string(),
        });
        nodes.push(stand_down(ids.stand_down, ids.side_arms));
        edges.push(edge(ids.stand_down, ids.report));
        nodes.push(tool_call(
            ids.report,
            STEP_REPORT,
            json!({ "state": ids.accumulator_address() }),
        ));

        (nodes, edges)
    }

    /// The shape [`Autonomy::Report`] emits: describe, retire, report.
    ///
    /// No loop head, no attempt, no arms — asserted on the emitted nodes rather
    /// than on a prompt telling a model to hold back.
    fn dry_run_shape(&self, seed: &Value) -> (Vec<Node>, Vec<GraphEdge>) {
        let ids = self.ids;
        let mut nodes = self.preamble(seed);
        nodes.push(stand_down(ids.stand_down, ids.side_arms));
        nodes.push(tool_call(
            ids.report,
            STEP_REPORT,
            json!({ "state": payload_address(ids.research) }),
        ));
        let edges = vec![
            edge(ids.trigger, ids.plan),
            edge(ids.plan, ids.research),
            edge(ids.research, ids.side_arms),
            edge(ids.side_arms, ids.stand_down),
            edge(ids.stand_down, ids.report),
        ];
        (nodes, edges)
    }

    /// Trigger, `plan`, `research`, and the work opened beside the loop.
    fn preamble(&self, seed: &Value) -> Vec<Node> {
        let ids = self.ids;
        vec![
            Node {
                id: ids.trigger.to_string(),
                kind: NodeKind::Trigger,
                type_version: 1,
                name: "start".to_string(),
                config: json!({ "trigger_kind": "manual" }),
                ports: Vec::new(),
                position: None,
            },
            // The seed is a literal, not an expression: there is no upstream
            // node to read it from, and an expression with nothing behind it
            // resolves to `null` without saying so.
            tool_call(ids.plan, STEP_PLAN, json!({ "state": seed.clone() })),
            tool_call(
                ids.research,
                STEP_RESEARCH,
                json!({ "state": payload_address(ids.plan) }),
            ),
            // `Spawn`, not another `tool_call`: this work does not gate the
            // pass, it outlives it, and it has to be retired at the end. With
            // no `TaskRunner` injected it runs inline and the ticket comes back
            // already settled, so a host without a scheduler computes the same
            // answer — what it loses is the overlap, not the result.
            Node {
                id: ids.side_arms.to_string(),
                kind: NodeKind::Spawn,
                type_version: 1,
                name: "open the arms beside the loop".to_string(),
                config: json!({
                    "target": "tool",
                    "slug": RUN_LOOP_STEP,
                    "args": {
                        "step": STEP_RESEARCH,
                        "state": payload_address(ids.research),
                    },
                }),
                ports: Vec::new(),
                position: None,
            },
        ]
    }

    /// The loop head, with its cap, its stop test, and its accumulator.
    fn head(&self) -> Node {
        let ids = self.ids;
        Node {
            id: ids.loop_head.to_string(),
            kind: NodeKind::Loop,
            type_version: 1,
            name: "the goal loop".to_string(),
            config: json!({
                // Every number here is interpolated from `Thresholds`.
                "max_iterations": self.thresholds.max_attempts,
                // `continue` rather than `error`: a run that spent its attempts
                // still has to reach `stand_down` and `report`, and a run that
                // failed at the head reports nothing about why it stopped.
                "on_exceeded": "continue",
                "until": self.termination.expression(&self.thresholds),
                "emit": "state",
                "state": {
                    "init": payload_address(ids.research),
                    // An assignment of the whole state the pass returned, never
                    // an increment of the previous one. An activation replayed
                    // after a resume applies this twice; `attempts + 1` twice is
                    // wrong by one and nothing reports it, while assigning the
                    // count the pass computed is right however many times it
                    // lands.
                    "update": payload_address(ids.pass),
                },
            }),
            ports: vec![port("body"), port("done")],
            position: None,
        }
    }

    /// The routing switch, keyed on the generated ladder.
    fn routing_switch(&self) -> Node {
        let mut ports: Vec<Port> = ROUTES.iter().map(|route| port(route.as_str())).collect();
        ports.push(port(DEFAULT_PORT));
        Node {
            id: self.ids.route.to_string(),
            kind: NodeKind::Switch,
            type_version: 1,
            name: "route the pass".to_string(),
            config: json!({ "expression": self.routing_expression() }),
            ports,
            position: None,
        }
    }

    /// The jq the routing switch branches on.
    ///
    /// [`ladder`] rendered verbatim, with one reshaping pipe in front of it.
    /// The ladder reads its accumulator as `.state // .item`, and at a switch
    /// there is no `state` key and `item` is the barrier's output envelope, so
    /// the pipe presents the folded state where the ladder already looks for
    /// it. Composing rather than re-rendering is the point: not one threshold
    /// is typed here, and the program the graph runs is the program
    /// `src/policy/` generates.
    fn routing_expression(&self) -> String {
        let rendered = ladder(&self.thresholds);
        let body = rendered.strip_prefix('=').unwrap_or(&rendered);
        format!("={{ item: .item.json }} | ({body})")
    }
}

/// One `main`-to-`main` edge.
fn edge(from: &str, to: &str) -> GraphEdge {
    GraphEdge {
        from_node: from.to_string(),
        from_port: "main".to_string(),
        to_node: to.to_string(),
        to_port: "main".to_string(),
    }
}

/// One named port.
fn port(name: &str) -> Port {
    Port {
        name: name.to_string(),
        label: None,
    }
}

/// A node body: one registered tool, named by its step.
///
/// Never a bare `agent_ref`. `NodeKind::Agent` loses the operator-directive
/// drain `attempt` performs, the salvage of an attempt its own cap killed, and
/// the arms opened beside the loop at a node a checkpoint can land on.
fn tool_call(id: &str, step: &str, args: Value) -> Node {
    let mut merged = json!({ "step": step });
    if let (Some(target), Some(extra)) = (merged.as_object_mut(), args.as_object()) {
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }
    Node {
        id: id.to_string(),
        kind: NodeKind::ToolCall,
        type_version: 1,
        name: step.to_string(),
        config: json!({
            "slug": RUN_LOOP_STEP,
            // `once`, not the kind's per-item default: a node handed several
            // items would otherwise run its step once per item and fold the
            // last one, which is a fan-out nobody asked for.
            "execution": "once",
            "args": merged,
        }),
        ports: Vec::new(),
        position: None,
    }
}

/// The gate that retires whatever `spawn` opened beside the loop.
fn stand_down(id: &str, from: &str) -> Node {
    Node {
        id: id.to_string(),
        kind: NodeKind::Gate,
        type_version: 1,
        name: "stand down".to_string(),
        config: json!({ "from": [from], "release": "all" }),
        ports: Vec::new(),
        position: None,
    }
}

/// The approval point `Autonomy::Assisted` emits.
fn approval(id: &str) -> Node {
    Node {
        id: id.to_string(),
        kind: NodeKind::Approval,
        type_version: 1,
        name: "approve the attempt".to_string(),
        config: json!({
            "subject": "the next attempt at the goal",
            "on_reject": "route",
        }),
        ports: vec![port("approved"), port("rejected")],
        position: None,
    }
}
