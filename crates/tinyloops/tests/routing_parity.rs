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
//! # What "exhaustive" means here, exactly
//!
//! The counter space is swept **exhaustively**, per threshold tuple, over a
//! range that reaches one past every threshold in that tuple — so the rung that
//! fires at the bound and the state just past it are both tested.
//!
//! The *threshold* space is not exhaustive, and saying so plainly matters more
//! than the word. Since thresholds are read out of the accumulator rather than
//! rendered into the graph, the ladder is one constant program and the set of
//! threshold tuples a run can reach is far larger than the shipped presets.
//! Crossing that set in full with the counter space runs to millions of
//! evaluations, each a fresh jq compile. So the sweep covers a **declared box**:
//! every shipped preset, every corner of `{0, 3}^5`, and the legacy tuples this
//! harness has always carried. That is a genuine widening over sweeping four
//! preset tuples and nothing between them, and it is chosen to contain the
//! boundaries — an operator bug, `>` where the Rust reads `>=`, shows at a
//! boundary or not at all. It is not a proof over the whole space.
//!
//! # Why it fails closed on `null`
//!
//! Under this engine a compile error, a run error, non-JSON output, and empty
//! output all yield `null`, silently. A sweep that read `null` as "no route"
//! would pass for a program that never compiled, so a non-string answer is
//! counted as a disagreement rather than as an absence.

// The workspace forbids `unwrap`/`expect`/`panic!` in library code; a test is
// where they belong, and the same allowance every other test module in this
// crate carries.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use serde_json::{Value, json};
use tinyflows::model::WorkflowGraph;

use tinyloops::{
    Advanced, Arm, ArmOutcome, ArmSet, Autonomy, CanWrite, LoopBuilder, LoopProfile, LoopState,
    NoWrite, Preset, Result, STEP_MERGE, Step, StepContext, StepRegistry, Thresholds, route,
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

/// The graph the kernel emits.
///
/// It takes no thresholds, and that is the point: they are addressed out of the
/// accumulator rather than rendered in, so one graph — one routing program —
/// serves every preset and every revision of one.
fn graph() -> WorkflowGraph {
    let mut registry = StepRegistry::new();
    for name in [
        "plan", "research", "attempt", STEP_MERGE, "pass", "report", "reflect", "judge",
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

    LoopBuilder::new(arms, registry)
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
fn first_disagreement(
    program: &str,
    thresholds: &Thresholds,
) -> Option<(LoopState, String, String)> {
    let compiled = Value::String(program.to_string());
    let profile = LoopProfile {
        revision: 0,
        thresholds: *thresholds,
        origin: Preset::Balanced,
    };
    for attempts in 0..=thresholds.max_attempts + 1 {
        for blocked in 0..=thresholds.blocked + 1 {
            for unverified in 0..=thresholds.unverified + 1 {
                for unproductive in 0..=thresholds.stuck + 1 {
                    for computational in 0..=thresholds.computational + 1 {
                        for solved in [false, true] {
                            let mut state = LoopState::with_profile("goal", profile);
                            state.attempts = attempts;
                            state.blocked = blocked;
                            state.unverified = unverified;
                            state.unproductive = unproductive;
                            state.computational = computational;
                            state.solved = solved;

                            let expected = route(&state).as_str().to_string();
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

/// The threshold tuples the sweep covers, each named so a failure says which
/// one disagreed.
///
/// Three sources, and each is there for a different reason. **Every shipped
/// preset**, because a preset nobody swept is a preset whose routing nobody
/// proved. **Every corner of `{0, 3}^5`**, because thresholds are now values a
/// run can move and the corners are where a comparison bug shows — a zero
/// threshold makes its rung fire on the first pass, and both sides have to
/// agree that it does. **The two legacy tuples**, because they were the
/// coverage this harness shipped with and removing them would be a silent
/// narrowing.
fn threshold_sets() -> Vec<(String, Thresholds)> {
    let mut sets: Vec<(String, Thresholds)> = Preset::ALL
        .into_iter()
        .map(|preset| (preset.to_string(), preset.thresholds()))
        .collect();

    for corner in 0..32_u32 {
        let at = |bit: u32| if corner & (1 << bit) == 0 { 0 } else { 3 };
        sets.push((
            format!("corner-{corner:02}"),
            Thresholds {
                max_attempts: at(0),
                stuck: at(1),
                blocked: at(2),
                computational: at(3),
                unverified: at(4),
                max_restarts: 2,
                plan_interval: 3,
            },
        ));
    }

    sets.push((
        "impatient".to_string(),
        Thresholds {
            max_attempts: 4,
            stuck: 1,
            blocked: 1,
            computational: 1,
            unverified: 1,
            max_restarts: 1,
            plan_interval: 2,
        },
    ));
    sets.push((
        "patient".to_string(),
        Thresholds {
            max_attempts: 6,
            stuck: 2,
            blocked: 2,
            computational: 2,
            unverified: 2,
            max_restarts: 3,
            plan_interval: 5,
        },
    ));
    sets
}

#[test]
fn the_sweep_covers_every_preset_and_every_corner() {
    let swept = threshold_sets();
    for preset in Preset::ALL {
        assert!(
            swept.iter().any(|(_, t)| *t == preset.thresholds()),
            "{preset} is not in the parity sweep",
        );
    }
    // 4 presets + 32 corners + 2 legacy tuples. Asserted rather than counted by
    // eye, so a corner dropped from the loop above fails here.
    assert_eq!(swept.len(), 38);
}

#[test]
fn the_rendered_ladder_and_the_rust_router_agree_over_the_box() {
    let program = routing_program(&graph());
    let sets = threshold_sets();

    // One thread per tuple: every evaluation is an independent jq compile, and
    // the box is large enough that running them in sequence would make the
    // suite something people skip.
    std::thread::scope(|scope| {
        for (name, thresholds) in &sets {
            let program = program.as_str();
            scope.spawn(move || {
                if let Some((state, expected, actual)) = first_disagreement(program, thresholds) {
                    panic!(
                        "threshold set {name:?} disagreed: the Rust router said {expected:?} and \
                         the emitted graph said {actual:?} for attempts={} blocked={} \
                         unverified={} unproductive={} computational={} solved={}",
                        state.attempts,
                        state.blocked,
                        state.unverified,
                        state.unproductive,
                        state.computational,
                        state.solved,
                    );
                }
            });
        }
    });
}

#[test]
fn the_sweep_reaches_past_every_threshold() {
    // A tuple with a higher cap gets a longer sweep rather than a fixed range
    // that stops short and calls the untested room agreement.
    let program = routing_program(&graph());
    for (name, thresholds) in threshold_sets() {
        let mut state = LoopState::with_profile(
            "goal",
            LoopProfile {
                revision: 0,
                thresholds,
                origin: Preset::Balanced,
            },
        );
        state.attempts = thresholds.max_attempts + 1;
        let answered = tinyflows::expr::evaluate(&Value::String(program.clone()), &scope(&state));
        assert_eq!(
            answered.as_str(),
            Some(route(&state).as_str()),
            "{name}: the state one past the cap is inside the swept range and still agrees",
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
fn the_emitted_program_addresses_the_accumulator_and_is_not_a_second_copy() {
    let program = routing_program(&graph());

    // No threshold is rendered in. The program reads them out of the state the
    // switch is handed, at the one address `route` reads them from.
    assert!(program.contains(".profile.thresholds"), "{program}");
    for rendered in [">= 8", ">= 2", ">= 12", ">= 4", ">= 1"] {
        assert!(
            !program.contains(rendered),
            "the emitted program renders a threshold: {program}",
        );
    }
    // The sentinel that makes a state with no profile fall through to `retry`
    // rather than fire the first rung on `0 >= null`.
    assert!(program.contains("4294967295"), "{program}");
    assert!(
        program.contains(&tinyloops::ladder()[1..]),
        "the switch runs the generated ladder verbatim: {program}",
    );
}

#[test]
fn one_graph_serves_every_preset() {
    // The graph no longer varies with the thresholds, which is what lets a run
    // that revises its own resume from a checkpoint taken before it did.
    let first = routing_program(&graph());
    let again = routing_program(&graph());
    assert_eq!(first, again);
}
