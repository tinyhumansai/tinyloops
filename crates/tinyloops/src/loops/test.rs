//! Unit tests for the emitted graph.
//!
//! Every invariant `docs/specs/loop-kernel.md` states about the *shape* of a
//! run is asserted here against the emitted graph rather than described in a
//! comment. Where a property is only observable in a running graph — arm
//! staleness, replay idempotence — the test lives in
//! `crates/tinyloops/tests/loop_run.rs` instead, because it needs the engine.

// The workspace forbids `unwrap`/`expect`/`panic!` in library code; a test is
// where they belong, and the same allowance every other test module in this
// crate carries.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use serde_json::{Value, json};

use super::builder::STEP_MERGE;
use super::types::mentions;
use super::{GraphSignature, LoopBuilder, NodeIds, TerminationCondition, verify_resume};
use crate::arm::{Arm, ArmOutcome, ArmSet};
use crate::policy::{Autonomy, LoopProfile, Route, Thresholds};
use crate::state::LoopState;
use crate::step::{
    Advanced, CanWrite, NoWrite, STEP_ATTEMPT, STEP_JUDGE, STEP_PASS, STEP_PLAN, STEP_REFLECT,
    STEP_REPORT, STEP_RESEARCH, Step, StepContext, StepRegistry,
};
use crate::{Error, Result};
use tinyflows::model::{NodeKind, WorkflowGraph};

/// A step body that changes nothing: the graph's shape is what is under test.
struct Body(&'static str);

impl Step for Body {
    fn name(&self) -> &'static str {
        self.0
    }

    fn run(&self, state: LoopState, ctx: StepContext<'_, CanWrite>) -> Result<Advanced> {
        Ok(ctx.advance(state))
    }
}

/// An arm that contributes nothing, declared under its own name.
struct Evaluator(&'static str);

impl Arm for Evaluator {
    fn name(&self) -> &'static str {
        self.0
    }

    fn evaluate(
        &self,
        base: &LoopState,
        _report: &Value,
        _ctx: StepContext<'_, NoWrite>,
    ) -> Result<ArmOutcome> {
        Ok(ArmOutcome::unchanged(self.name(), base))
    }
}

/// Every step the kernel emits a node for.
fn registry() -> StepRegistry {
    let mut registry = StepRegistry::new();
    for name in [
        STEP_PLAN,
        STEP_RESEARCH,
        STEP_ATTEMPT,
        STEP_MERGE,
        STEP_PASS,
        STEP_REPORT,
        STEP_REFLECT,
        STEP_JUDGE,
    ] {
        registry
            .register(Arc::new(Body(name)))
            .expect("each fixture step is registered once");
    }
    registry
}

/// The two arms the specification names.
fn arms() -> ArmSet {
    ArmSet::new(vec![
        Arc::new(Evaluator(STEP_REFLECT)) as Arc<dyn Arm>,
        Arc::new(Evaluator(STEP_JUDGE)),
    ])
    .expect("two distinct arms are a valid set")
}

/// A graph that acts, at the given autonomy and profile.
fn graph_at(autonomy: Autonomy, profile: LoopProfile) -> WorkflowGraph {
    LoopBuilder::new(arms(), registry())
        .goal("ship the release")
        .autonomy(autonomy)
        .profile(profile)
        .build()
        .expect("the fixture builds a valid graph")
}

/// The default unattended graph.
fn graph() -> WorkflowGraph {
    graph_at(Autonomy::Unattended, LoopProfile::default())
}

/// A profile carrying `thresholds`, for the tests about what does and does not
/// move the signature.
fn profile(thresholds: Thresholds) -> LoopProfile {
    LoopProfile {
        thresholds,
        ..LoopProfile::default()
    }
}

#[test]
fn emits_the_specified_node_set() {
    let graph = graph();
    let ids = NodeIds::default();
    let emitted: Vec<&str> = graph.nodes.iter().map(|node| node.id.as_str()).collect();

    for expected in [
        ids.trigger,
        ids.plan,
        ids.research,
        ids.side_arms,
        ids.loop_head,
        ids.attempt,
        STEP_REFLECT,
        STEP_JUDGE,
        ids.merge,
        ids.route,
        ids.pass,
        ids.stand_down,
        ids.report,
    ] {
        assert!(emitted.contains(&expected), "{expected} is missing");
    }
    // Unattended asks nobody, so the approval point is not emitted.
    assert!(!emitted.contains(&ids.approval));
    assert_eq!(emitted.len(), 13);
}

#[test]
fn every_node_in_the_body_is_a_tool_call_naming_one_step() {
    let graph = graph();
    let ids = NodeIds::default();
    for id in [ids.attempt, STEP_REFLECT, STEP_JUDGE, ids.merge, ids.pass] {
        let node = graph.node(id).expect("the body node is emitted");
        assert_eq!(node.kind, NodeKind::ToolCall, "{id} is not a tool call");
        assert_eq!(node.config["slug"], json!(crate::RUN_LOOP_STEP));
        assert!(
            node.config["args"]["step"].is_string(),
            "{id} names no step"
        );
        assert!(
            node.config.get("agent_ref").is_none(),
            "{id} names an agent"
        );
    }
}

#[test]
fn the_work_beside_the_loop_is_spawned_and_gated() {
    let graph = graph();
    let ids = NodeIds::default();
    assert_eq!(
        graph.node(ids.side_arms).map(|node| node.kind.clone()),
        Some(NodeKind::Spawn),
    );
    assert_eq!(
        graph.node(ids.stand_down).map(|node| node.kind.clone()),
        Some(NodeKind::Gate),
    );
    // The gate collects the spawn by name, which is what makes standing down a
    // node the graph reaches rather than a cleanup call somebody remembers.
    let gate = graph.node(ids.stand_down).expect("the gate is emitted");
    assert_eq!(gate.config["from"], json!([ids.side_arms]));
}

#[test]
fn pass_is_the_only_node_with_an_edge_back_to_the_head() {
    let graph = graph();
    let ids = NodeIds::default();
    let closing: Vec<&str> = graph
        .edges
        .iter()
        .filter(|edge| edge.to_node == ids.loop_head)
        .map(|edge| edge.from_node.as_str())
        .collect();
    assert_eq!(closing, [ids.side_arms, ids.pass]);
}

#[test]
fn every_route_port_enters_pass() {
    let graph = graph();
    let ids = NodeIds::default();
    for route in [
        Route::Blocked,
        Route::Solved,
        Route::Reported,
        Route::Diversify,
        Route::Retry,
    ] {
        let target = graph
            .edges
            .iter()
            .find(|edge| edge.from_node == ids.route && edge.from_port == route.as_str())
            .map(|edge| edge.to_node.as_str());
        assert_eq!(target, Some(ids.pass), "{} strayed", route.as_str());
    }
    // A ladder that fails to compile yields `null`, which routes here; sending
    // it to `pass` costs a pass rather than stranding the run mid-body.
    assert!(
        graph
            .edges
            .iter()
            .any(|edge| edge.from_node == ids.route && edge.from_port == "default"),
    );
    // Nothing routes back to the attempt: an inner cycle the head never sees
    // cannot be bounded by `config.max_iterations`.
    assert!(
        !graph
            .edges
            .iter()
            .any(|edge| edge.from_node == ids.route && edge.to_node == ids.attempt),
    );
}

#[test]
fn report_is_reachable_only_after_stand_down() {
    let graph = graph();
    let ids = NodeIds::default();
    let into_report: Vec<&str> = graph
        .edges
        .iter()
        .filter(|edge| edge.to_node == ids.report)
        .map(|edge| edge.from_node.as_str())
        .collect();
    assert_eq!(into_report, [ids.stand_down]);
}

#[test]
fn node_ids_are_declared_not_positional() {
    let ids = NodeIds {
        loop_head: "outer",
        attempt: "try",
        ..NodeIds::default()
    };
    let graph = LoopBuilder::new(arms(), registry())
        .autonomy(Autonomy::Unattended)
        .ids(ids)
        .build()
        .expect("renamed ids still build");

    assert!(graph.node("outer").is_some());
    assert!(graph.node("try").is_some());
    assert!(graph.node("loop").is_none());
    // Renaming the head renames the address the attempt reads it at, because
    // the address is derived from the declaration rather than typed.
    let attempt = graph.node("try").expect("the attempt is emitted");
    assert!(mentions(&attempt.config, "=nodes.outer.state"));
}

#[test]
fn no_arm_reads_the_accumulator() {
    let graph = graph();
    let ids = NodeIds::default();
    for arm in arms().names() {
        let node = graph.node(arm).expect("every declared arm is emitted");
        assert!(
            !mentions(&node.config, &ids.accumulator_address()),
            "{arm} reads the accumulator",
        );
        assert!(
            !mentions(&node.config, "=.nodes.loop.state"),
            "{arm} reads the accumulator in the jq spelling",
        );
        // It reads the node immediately upstream instead.
        assert!(mentions(&node.config, "=nodes.attempt.item.json"));
    }
}

#[test]
fn the_attempt_reads_the_accumulator_the_head_just_folded() {
    let graph = graph();
    let ids = NodeIds::default();
    let attempt = graph.node(ids.attempt).expect("the attempt is emitted");
    // Legal precisely here and nowhere further into the body: the head folds at
    // the top of the pass, so this is the only point at which the accumulator
    // is current.
    assert!(mentions(&attempt.config, &ids.accumulator_address()));
}

#[test]
fn fan_out_and_fold_inputs_name_the_same_arms() {
    let graph = graph();
    let ids = NodeIds::default();
    let set = arms();

    let mut fanned: Vec<&str> = graph
        .edges
        .iter()
        .filter(|edge| edge.from_node == ids.attempt)
        .map(|edge| edge.to_node.as_str())
        .collect();
    fanned.sort_unstable();

    let merge = graph.node(ids.merge).expect("the barrier is emitted");
    let mut folded: Vec<&str> = merge.config["args"]["arms"]
        .as_object()
        .expect("the fold names its inputs")
        .keys()
        .map(String::as_str)
        .collect();
    folded.sort_unstable();

    let mut declared = set.names();
    declared.sort_unstable();

    assert_eq!(fanned, folded);
    assert_eq!(fanned, declared);
    assert_eq!(set.fold_inputs().len(), folded.len());
}

#[test]
fn removing_an_arm_removes_it_from_both_the_fan_out_and_the_fold() {
    let one = ArmSet::new(vec![Arc::new(Evaluator(STEP_REFLECT)) as Arc<dyn Arm>])
        .expect("one arm is a valid set");
    let graph = LoopBuilder::new(one, registry())
        .autonomy(Autonomy::Unattended)
        .build()
        .expect("a one-armed loop builds");
    let ids = NodeIds::default();

    assert!(graph.node(STEP_JUDGE).is_none());
    assert!(
        !graph
            .edges
            .iter()
            .any(|edge| edge.from_node == STEP_JUDGE || edge.to_node == STEP_JUDGE),
    );
    let merge = graph.node(ids.merge).expect("the barrier is emitted");
    assert!(merge.config["args"]["arms"].get(STEP_JUDGE).is_none());
}

#[test]
fn no_threshold_reaches_the_builder_at_all() {
    // The guard this replaces asserted that every threshold was rendered into
    // the emitted programs, and that the defaults' numbers were absent because
    // they had not been typed. Both halves flip: no threshold is rendered, and
    // the programs address the accumulator instead. What is still being guarded
    // is the same thing — a second copy of a constant, free to drift.
    let graph = graph_at(
        Autonomy::Unattended,
        profile(Thresholds {
            max_attempts: 41,
            blocked: 37,
            unverified: 29,
            stuck: 23,
            computational: 19,
            max_restarts: 17,
            plan_interval: 13,
        }),
    );
    let ids = NodeIds::default();

    let head = graph.node(ids.loop_head).expect("the head is emitted");
    // The head's cap is the budget's runaway backstop, never `max_attempts`:
    // an amendment raising the attempt ceiling would otherwise fold, read back
    // as raised, and buy nothing.
    assert_eq!(
        head.config["max_iterations"],
        json!(crate::budget::Caps::default().max_iterations)
    );
    let until = head.config["until"].as_str().expect("`until` is a program");
    assert!(until.contains(".profile.thresholds"), "{until}");

    let route = graph.node(ids.route).expect("the switch is emitted");
    let program = route.config["expression"]
        .as_str()
        .expect("the switch keys on a program");
    assert!(program.contains(".profile.thresholds"), "{program}");
    for rendered in [">= 37", ">= 41", ">= 29", ">= 23", ">= 19", ">= 8"] {
        assert!(
            !program.contains(rendered),
            "{rendered} was rendered into {program}"
        );
    }
}

#[test]
fn the_accumulator_update_is_an_assignment_not_an_increment() {
    let graph = graph();
    let ids = NodeIds::default();
    let head = graph.node(ids.loop_head).expect("the head is emitted");
    let update = head.config["state"]["update"]
        .as_str()
        .expect("the head folds through an expression");

    // The whole state the pass returned, assigned. A replayed activation lands
    // on the same value; `+ 1` twice is wrong by one and nothing reports it.
    assert!(
        update.starts_with("=.nodes[\"pass\"].item.json"),
        "{update}"
    );
    assert!(!update.contains('+'), "{update}");
    assert!(!update.contains(".state"), "{update}");
    assert_eq!(head.config["on_exceeded"], json!("continue"));
    assert_eq!(head.config["emit"], json!("state"));
}

#[test]
fn the_emitted_graph_validates_and_compiles() {
    for autonomy in [Autonomy::Report, Autonomy::Assisted, Autonomy::Unattended] {
        let graph = graph_at(autonomy, LoopProfile::default());
        tinyflows::validate::validate(&graph).expect("the emitted graph validates");
        tinyflows::compiler::compile(&graph).expect("the emitted graph compiles");
    }
}

#[test]
fn building_twice_emits_byte_identical_json() {
    let first = serde_json::to_string(&graph()).expect("a graph serializes");
    let second = serde_json::to_string(&graph()).expect("a graph serializes");
    assert_eq!(first, second);
}

#[test]
fn a_step_absent_from_the_registry_is_a_build_error() {
    let mut missing = StepRegistry::new();
    for name in [
        STEP_PLAN,
        STEP_RESEARCH,
        STEP_ATTEMPT,
        STEP_PASS,
        STEP_REPORT,
        STEP_REFLECT,
        STEP_JUDGE,
    ] {
        missing
            .register(Arc::new(Body(name)))
            .expect("each step is registered once");
    }
    let error = LoopBuilder::new(arms(), missing)
        .autonomy(Autonomy::Unattended)
        .build()
        .expect_err("a node naming an unregistered step cannot build");
    assert_eq!(
        error,
        Error::UnknownStep {
            name: STEP_MERGE.to_string(),
        },
    );
}

#[test]
fn a_graph_that_fails_validation_is_a_named_error() {
    // Two loops sharing one set of ids collide on every node. The engine's own
    // validator is what says so, and its message is carried through rather than
    // re-classified.
    let graph = graph();
    let mut doubled = graph.clone();
    doubled.nodes.extend(graph.nodes.clone());
    let error = tinyflows::validate::validate(&doubled).expect_err("duplicate ids are invalid");
    let wrapped = Error::InvalidLoopGraph {
        reason: error.to_string(),
    };
    assert!(
        wrapped
            .to_string()
            .starts_with("the emitted loop graph is not valid")
    );
}

#[test]
fn assisted_emits_an_approval_point_and_unattended_does_not() {
    let ids = NodeIds::default();
    let assisted = graph_at(Autonomy::Assisted, LoopProfile::default());
    let approval = assisted.node(ids.approval).expect("assisted asks");
    assert_eq!(approval.kind, NodeKind::Approval);
    // The attempt is reachable only through it.
    let into_attempt: Vec<&str> = assisted
        .edges
        .iter()
        .filter(|edge| edge.to_node == ids.attempt)
        .map(|edge| edge.from_node.as_str())
        .collect();
    assert_eq!(into_attempt, [ids.approval]);

    let unattended = graph_at(Autonomy::Unattended, LoopProfile::default());
    assert!(unattended.node(ids.approval).is_none());
    let into_attempt: Vec<&str> = unattended
        .edges
        .iter()
        .filter(|edge| edge.to_node == ids.attempt)
        .map(|edge| edge.from_node.as_str())
        .collect();
    assert_eq!(into_attempt, [ids.loop_head]);
}

#[test]
fn report_autonomy_emits_no_node_that_acts() {
    let graph = graph_at(Autonomy::Report, LoopProfile::default());
    let ids = NodeIds::default();
    for absent in [ids.loop_head, ids.attempt, ids.merge, ids.route, ids.pass] {
        assert!(graph.node(absent).is_none(), "{absent} acts");
    }
    for arm in arms().names() {
        assert!(graph.node(arm).is_none(), "{arm} acts");
    }
    // It still plans, researches, stands down, and reports.
    for present in [
        ids.plan,
        ids.research,
        ids.side_arms,
        ids.stand_down,
        ids.report,
    ] {
        assert!(graph.node(present).is_some(), "{present} is missing");
    }
}

#[test]
fn the_signature_is_stable_across_two_builds() {
    assert_eq!(GraphSignature::of(&graph()), GraphSignature::of(&graph()));
    assert!(verify_resume(&GraphSignature::of(&graph()), &graph()).is_ok());
}

/// The two programs a threshold used to be rendered into.
fn programs(graph: &WorkflowGraph) -> (String, String) {
    let ids = NodeIds::default();
    let head = graph.node(ids.loop_head).expect("the head is emitted");
    let route = graph.node(ids.route).expect("the switch is emitted");
    (
        head.config["until"]
            .as_str()
            .expect("`until` is a program")
            .to_string(),
        route.config["expression"]
            .as_str()
            .expect("the switch keys on a program")
            .to_string(),
    )
}

#[test]
fn the_routing_programs_are_the_same_under_every_preset() {
    // The inverse of the assertion this replaces, and the point of the change
    // it tests. A threshold used to be rendered into both of these programs, so
    // changing one changed the topology; now both address the accumulator and
    // one pair of programs routes every preset.
    let baseline = programs(&graph());
    for preset in crate::presets::Preset::ALL {
        assert_eq!(
            programs(&graph_at(Autonomy::Unattended, LoopProfile::of(preset))),
            baseline,
            "{preset} emitted a different routing program",
        );
    }

    let head = graph_at(
        Autonomy::Unattended,
        profile(Thresholds {
            max_attempts: 41,
            ..Thresholds::default()
        }),
    );
    assert_eq!(programs(&head), baseline);
    assert_eq!(
        head.node(NodeIds::default().loop_head)
            .expect("the head")
            .config["max_iterations"],
        json!(crate::budget::Caps::default().max_iterations),
    );
}

#[test]
fn revising_a_threshold_leaves_the_graph_untouched() {
    // The property the addressing change exists to buy, stated directly. A run
    // that revises its thresholds moves them in its *accumulator*; the graph is
    // built once and is not a function of them, so the signature a checkpoint
    // recorded still verifies and the resume is allowed.
    let built = graph();
    let recorded = GraphSignature::of(&built);

    let mut revised = LoopState::new("ship the release");
    revised.unproductive = 2;
    assert_eq!(crate::policy::route(&revised), Route::Diversify);
    revised.profile.thresholds.stuck = 4;
    revised.profile.revision = 1;
    assert_eq!(
        crate::policy::route(&revised),
        Route::Retry,
        "the revision reached the router",
    );

    assert_eq!(GraphSignature::of(&graph()), recorded);
    assert!(verify_resume(&recorded, &graph()).is_ok());
}

#[test]
fn a_different_preset_is_a_different_run_but_not_a_different_ladder() {
    // Worth pinning because it is the one place the change stops short of the
    // slogan. The starting profile is seeded into the accumulator the `plan`
    // node is handed, exactly as the goal is, so two presets do emit different
    // graphs — they are different runs. What they no longer differ in is the
    // routing, which is what a resume across a *revision* depends on.
    let balanced = graph_at(Autonomy::Unattended, LoopProfile::of(crate::presets::Preset::Balanced));
    let persistent =
        graph_at(Autonomy::Unattended, LoopProfile::of(crate::presets::Preset::Persistent));

    assert_ne!(
        GraphSignature::of(&balanced),
        GraphSignature::of(&persistent)
    );
    assert_eq!(programs(&balanced), programs(&persistent));
}

#[test]
fn the_emitted_graph_renders_no_threshold_into_a_program() {
    let graph = graph_at(
        Autonomy::Unattended,
        profile(Thresholds {
            max_attempts: 41,
            stuck: 37,
            blocked: 29,
            computational: 23,
            unverified: 19,
            max_restarts: 17,
            plan_interval: 13,
        }),
    );
    let (until, expression) = programs(&graph);

    for rendered in [
        ">= 41", ">= 37", ">= 29", ">= 23", ">= 19", ">= 17", ">= 8", ">= 2",
    ] {
        assert!(!until.contains(rendered), "{rendered} in {until}");
        assert!(!expression.contains(rendered), "{rendered} in {expression}");
    }
    assert!(until.contains(".profile.thresholds"));
    assert!(expression.contains(".profile.thresholds"));
}

#[test]
fn adding_an_arm_changes_the_signature() {
    let one = ArmSet::new(vec![Arc::new(Evaluator(STEP_REFLECT)) as Arc<dyn Arm>])
        .expect("one arm is a valid set");
    let smaller = LoopBuilder::new(one, registry())
        .goal("ship the release")
        .autonomy(Autonomy::Unattended)
        .build()
        .expect("a one-armed loop builds");
    assert_ne!(GraphSignature::of(&smaller), GraphSignature::of(&graph()));
}

#[test]
fn resuming_against_a_mismatched_signature_is_a_named_error() {
    // Topology still moves the hash; only values left it. A graph emitted at a
    // different autonomy has different nodes, which is a resume that must be
    // refused.
    let recorded = GraphSignature::of(&graph_at(Autonomy::Unattended, LoopProfile::default()));
    let current = graph_at(Autonomy::Assisted, LoopProfile::default());
    let error = verify_resume(&recorded, &current).expect_err("a changed topology refuses");
    match error {
        Error::GraphSignatureMismatch {
            recorded: named,
            current: now,
        } => {
            assert_eq!(named, recorded.as_str());
            assert_eq!(now, GraphSignature::of(&current).as_str());
            assert_ne!(named, now);
        }
        other => panic!("expected a signature mismatch, got {other:?}"),
    }
}

#[test]
fn an_exhausted_budget_is_never_success() {
    let thresholds = Thresholds::default();
    let mut condition = TerminationCondition::terminal();
    let mut state = LoopState::new("goal");
    // Everything a hopeful last pass would set.
    state.solved = true;
    state.banked = 3;
    state.attempts = thresholds.max_attempts;

    let outcome = condition
        .evaluate(&state)
        .expect("a spent budget stops the run");
    assert_eq!(outcome, crate::Outcome::Exhausted);
    assert_ne!(outcome, crate::Outcome::Success);
}

#[test]
fn a_provider_failure_reports_blocked() {
    let thresholds = Thresholds::default();
    let mut condition = TerminationCondition::terminal();
    let mut state = LoopState::new("goal");
    state.blocked = thresholds.blocked;
    assert_eq!(
        condition.evaluate(&state),
        Some(crate::Outcome::Blocked),
    );
}

#[test]
fn conditions_compose_with_and_and_or() {
    let mut state = LoopState::new("goal");
    state.expired = true;

    let mut either = TerminationCondition::solved() | TerminationCondition::expired();
    assert!(either.evaluate(&state).is_some());

    let mut both = TerminationCondition::solved() & TerminationCondition::expired();
    assert_eq!(both.evaluate(&state), None);

    state.solved = true;
    let mut both = TerminationCondition::solved() & TerminationCondition::expired();
    assert!(both.evaluate(&state).is_some());

    // The identities of the two operators.
    let mut none_of = TerminationCondition::any(Vec::new());
    assert_eq!(none_of.evaluate(&LoopState::new("g")), None);
    let mut all_of = TerminationCondition::all(Vec::new());
    assert!(all_of.evaluate(&LoopState::new("g")).is_some());
    assert!(all_of.expression().contains("true"));
    assert!(
        TerminationCondition::any(Vec::new())
            .expression()
            .contains("false"),
    );
}

#[test]
fn a_condition_round_trips_through_serde() {
    let mut condition = TerminationCondition::terminal() | TerminationCondition::expired();
    let mut state = LoopState::new("goal");
    state.expired = true;
    condition.evaluate(&state);

    let encoded = serde_json::to_string(&condition).expect("a condition serializes");
    let decoded: TerminationCondition =
        serde_json::from_str(&encoded).expect("a condition deserializes");
    assert_eq!(decoded, condition);
    assert_eq!(decoded.fired(), condition.fired());
}

#[test]
fn resetting_a_fired_condition_clears_it() {
    let mut condition = TerminationCondition::terminal() & TerminationCondition::expired();
    let mut state = LoopState::new("goal");
    state.expired = true;
    assert!(condition.evaluate(&state).is_some());
    assert!(condition.fired().is_some());

    condition.reset();
    assert_eq!(condition.fired(), None);
    assert_eq!(
        condition.evaluate(&LoopState::new("goal")),
        None,
        "a reset condition re-decides rather than replaying its latch",
    );
}

#[test]
fn a_composed_termination_is_what_the_head_runs() {
    let condition = TerminationCondition::solved() | TerminationCondition::expired();
    let graph = LoopBuilder::new(arms(), registry())
        .autonomy(Autonomy::Unattended)
        .termination(condition.clone())
        .build()
        .expect("a composed condition still builds");
    let head = graph.node(NodeIds::default().loop_head).expect("the head");
    assert_eq!(
        head.config["until"],
        json!(condition.expression())
    );
}

#[test]
fn the_termination_expression_evaluates_rather_than_yielding_null() {
    // Under this engine a program that fails to compile yields `null`, and
    // `null` is falsey — so "it produced a boolean" is itself the assertion.
    let mut state = LoopState::new("goal");
    state.expired = true;
    let condition = TerminationCondition::terminal() | TerminationCondition::solved();
    let program = Value::String(condition.expression());
    let scope = json!({ "state": serde_json::to_value(&state).expect("state encodes") });
    assert_eq!(tinyflows::expr::evaluate(&program, &scope), json!(true));

    let mut fresh = json!({ "state": serde_json::to_value(LoopState::new("g")).unwrap() });
    assert_eq!(tinyflows::expr::evaluate(&program, &fresh), json!(false));
    fresh["state"]["solved"] = json!(true);
    assert_eq!(tinyflows::expr::evaluate(&program, &fresh), json!(true));
}

#[test]
fn the_node_vocabulary_is_the_ids_the_builder_emits() {
    let ids = NodeIds::default();
    let vocabulary = ids.all();
    assert_eq!(vocabulary.len(), 12);
    // Every name is distinct: two nodes answering to one id would collide in
    // the graph and in every address derived from it.
    for (index, name) in vocabulary.iter().enumerate() {
        assert!(
            !vocabulary[..index].contains(name),
            "{name} is declared twice",
        );
    }
    // The approval is in the vocabulary but not in every graph.
    assert!(vocabulary.contains(&ids.approval));
    assert!(graph().node(ids.approval).is_none());
}

#[test]
fn the_accumulator_address_follows_the_declared_head() {
    assert_eq!(
        NodeIds::default().accumulator_address(),
        "=nodes.loop.state",
    );
    assert_eq!(
        NodeIds {
            loop_head: "outer",
            ..NodeIds::default()
        }
        .accumulator_address(),
        "=nodes.outer.state",
    );
}

#[test]
fn an_address_is_looked_for_at_every_depth_of_a_config() {
    // The invariant tests read the emitted JSON rather than the code that wrote
    // it, so the search has to reach a string wherever it is nested.
    assert!(mentions(
        &json!({ "a": { "b": ["x", "=nodes.loop.state"] } }),
        "=nodes.loop.state"
    ));
    assert!(mentions(&json!(["=nodes.loop.state"]), "=nodes.loop.state"));
    assert!(mentions(
        &json!("=nodes.loop.state.attempts"),
        "=nodes.loop.state"
    ));
    // Numbers, booleans, and nulls hold no address, and neither does an
    // unrelated string.
    assert!(!mentions(
        &json!({ "n": 1, "b": true, "z": Value::Null }),
        "=nodes.loop.state"
    ));
    assert!(!mentions(
        &json!("=nodes.attempt.item.json"),
        "=nodes.loop.state"
    ));
}
