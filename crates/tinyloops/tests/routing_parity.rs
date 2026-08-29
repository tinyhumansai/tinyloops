//! The exhaustive parity sweep: the emitted graph's jq and the crate's Rust
//! router must agree on every state either of them can read.
//!
//! `src/policy/test.rs` already sweeps [`ladder`] against [`route`] in-crate.
//! This is the *graph's* copy of that decision — the program a `switch` node
//! actually branches on, read back off the emitted `WorkflowGraph` through the
//! public surface the way a reviewer would — because the builder reshapes the
//! ladder's input before piping it in, and a reshaping that lost the
//! accumulator would leave a program that still answers, just always the same
//! way.
//!
//! # What this proves, and what it cannot
//!
//! It proves the **translation**, never the answer. Both sides read the same
//! `Thresholds`, so a wrong threshold is wrong in both and agrees with itself;
//! `docs/specs/routing-and-policy.md` is where the numbers are argued for. What
//! the sweep removes is the class of failure where the two disagree about a
//! *comparison* — a `>` where the Rust reads `>=` changes when a run
//! diversifies and fails nothing.
//!
//! # Why exhaustive rather than sampled
//!
//! Both sides are pure functions of a handful of small-range integers, so the
//! whole space is cheap. Sampling would buy nothing and could miss exactly the
//! off-by-one the sweep exists to catch.
//!
//! # Why it fails closed on `null`
//!
//! Under this engine a compile error, a run error, non-JSON output, and empty
//! output all yield `null`, silently. A sweep that read `null` as "no route"
//! would pass for a program that never compiled, so a non-string answer is
//! counted as a disagreement rather than as an absence.

use std::sync::Arc;

use serde_json::{Value, json};
use tinyflows::model::WorkflowGraph;

use tinyloops::{
    Advanced, Arm, ArmOutcome, ArmSet, Autonomy, CanWrite, LoopBuilder, LoopState, NoWrite, Result,
    STEP_MERGE, Step, StepContext, StepRegistry, Thresholds, route,
};

/// A step body that changes nothing; only the emitted program is under test.
struct Body(&'static str);

impl Step for Body {
    fn name(&self) -> &'static str {
        self.0
    }

    fn run(&self, state: LoopState, ctx: StepContext<'_, CanWrite>) -> Result<Advanced> {
        Ok(ctx.advance(state))
    }
}

/// An arm that contributes nothing.
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

/// The graph a preset emits.
fn graph(thresholds: Thresholds) -> WorkflowGraph {
    let mut registry = StepRegistry::new();
    for name in [
        "plan",
        "research",
        "attempt",
        STEP_MERGE,
        "pass",
        "report",
        "reflect",
        "judge",
    ] {
        registry
            .register(Arc::new(Body(name)))
            .expect("each step is registered once");
    }
    let arms = ArmSet::new(vec![
        Arc::new(Evaluator("reflect")) as Arc<dyn Arm>,
        Arc::new(Evaluator("judge")),
    ])
    .expect("two distinct arms are a valid set");

    LoopBuilder::new(thresholds, arms, registry)
        .goal("ship the release")
        .autonomy(Autonomy::Unattended)
        .build()
        .expect("a preset emits a valid graph")
}

/// The program the emitted routing switch branches on.
fn routing_program(graph: &WorkflowGraph) -> String {
    graph
        .node("route")
        .expect("the switch is emitted")
        .config
        .get("expression")
        .and_then(Value::as_str)
        .expect("the switch keys on an expression")
        .to_string()
}

/// The scope the switch node sees: its input item is the barrier's envelope.
fn scope(state: &LoopState) -> Value {
    json!({ "item": { "json": serde_json::to_value(state).expect("state encodes") } })
}

/// The first state on which `program` and [`route`] disagree, if any.
///
/// `restarts` is not swept, and that is deliberate rather than an oversight: no
/// rung of the ladder reads it, so sweeping it multiplies the run time by the
/// restart allowance and can only ever confirm both sides ignoring the same
/// field.
fn first_disagreement(program: &str, thresholds: &Thresholds) -> Option<(LoopState, String, String)> {
    let compiled = Value::String(program.to_string());
    for attempts in 0..=thresholds.max_attempts + 1 {
        for blocked in 0..=thresholds.blocked + 1 {
            for unverified in 0..=thresholds.unverified + 1 {
                for unproductive in 0..=thresholds.stuck + 1 {
                    for computational in 0..=thresholds.computational + 1 {
                        for solved in [false, true] {
                            let mut state = LoopState::new("goal");
                            state.attempts = attempts;
                            state.blocked = blocked;
                            state.unverified = unverified;
                            state.unproductive = unproductive;
                            state.computational = computational;
                            state.solved = solved;

                            let expected = route(&state, thresholds).as_str().to_string();
                            // Fail closed: anything that is not a string is a
                            // program that did not answer, which is a
                            // disagreement rather than a route.
                            let actual = tinyflows::expr::evaluate(&compiled, &scope(&state))
                                .as_str()
                                .map_or_else(|| "<null>".to_string(), str::to_string);
                            if actual != expected {
                                return Some((state, expected, actual));
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

/// The presets the sweep covers, each named so a failure says which disagreed.
fn presets() -> Vec<(&'static str, Thresholds)> {
    vec![
        ("default", Thresholds::default()),
        (
            "impatient",
            Thresholds {
                max_attempts: 4,
                stuck: 1,
                blocked: 1,
                computational: 1,
                unverified: 1,
                max_restarts: 1,
                plan_interval: 2,
            },
        ),
        (
            "patient",
            Thresholds {
                max_attempts: 6,
                stuck: 2,
                blocked: 2,
                computational: 2,
                unverified: 2,
                max_restarts: 3,
                plan_interval: 5,
            },
        ),
    ]
}

#[test]
fn the_rendered_ladder_and_the_rust_router_agree_for_every_preset() {
    for (name, thresholds) in presets() {
        let program = routing_program(&graph(thresholds));
        if let Some((state, expected, actual)) = first_disagreement(&program, &thresholds) {
            panic!(
                "preset {name:?} disagreed: the Rust router said {expected:?} and the emitted \
                 graph said {actual:?} for attempts={} blocked={} unverified={} unproductive={} \
                 computational={} solved={}",
                state.attempts,
                state.blocked,
                state.unverified,
                state.unproductive,
                state.computational,
                state.solved,
            );
        }
    }
}

#[test]
fn the_sweep_reaches_past_every_threshold() {
    // A preset with a higher cap gets a longer sweep rather than a fixed range
    // that stops short and calls the untested room agreement.
    for (_, thresholds) in presets() {
        let mut state = LoopState::new("goal");
        state.attempts = thresholds.max_attempts + 1;
        let program = routing_program(&graph(thresholds));
        let answered = tinyflows::expr::evaluate(&Value::String(program), &scope(&state));
        assert_eq!(
            answered.as_str(),
            Some(route(&state, &thresholds).as_str()),
            "the state one past the cap is inside the swept range and still agrees",
        );
    }
}

#[test]
fn a_ladder_that_fails_to_compile_is_caught_by_the_sweep() {
    // The whole reason the comparison fails closed. This program does not
    // compile, so every evaluation of it yields `null` — which is falsey and
    // otherwise indistinguishable from a decision.
    let thresholds = Thresholds::default();
    let broken = "=this is not jq |||";
    let (_, expected, actual) =
        first_disagreement(broken, &thresholds).expect("a broken ladder must be caught");
    assert_eq!(actual, "<null>");
    assert_ne!(expected, actual);
}

#[test]
fn the_emitted_program_is_the_generated_ladder_and_not_a_second_copy() {
    let thresholds = Thresholds {
        blocked: 7,
        ..Thresholds::default()
    };
    let program = routing_program(&graph(thresholds));
    // Every threshold in the emitted program came from the constant.
    assert!(program.contains(">= 7"), "{program}");
    assert!(
        program.contains(&tinyloops::ladder(&thresholds)[1..]),
        "the switch runs the generated ladder verbatim: {program}",
    );
}
