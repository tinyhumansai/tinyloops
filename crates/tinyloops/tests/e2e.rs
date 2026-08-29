//! The assembled preset, run through the engine, with real steps and real arms.
//!
//! Everything else in this crate's suite tests a piece. This file tests the
//! whole thing the way a host runs it: `LoopBuilder` emits a graph, the engine
//! executes it, and every node body is the real registered step reached through
//! `run_loop_step`. Nothing here substitutes a step's answer, so a wiring
//! mistake shows up as the wrong number in the accumulator rather than as a
//! mock that was never consulted.
//!
//! # What it is really for
//!
//! The merge. `AssembledLoop::drive` and the engine execute the same step
//! bodies, but only the engine exercises the part that used to be missing: the
//! merge node is handed each arm's whole returned accumulator through its node
//! arguments, addressed by jq, and has to fold them. A `null` there would be
//! indistinguishable from an arm that contributed nothing, so several tests
//! below assert the merge's *output*, not that it ran.
//!
//! # Why the runs here are assembled rather than driven by `TestHarness`
//!
//! `tinyflows::testkit` hands every node activation a freshly constructed
//! `TokioTaskRunner`, so a ticket `side_arms` issues is unknown to `stand_down`
//! when it collects, and the run dies on `unknown task ticket`. That is a
//! property of the double rather than of the graph: with one set of
//! capabilities for the whole run — which is what a host provides — the spawn
//! and the gate agree. See `loop_run.rs`, which documents the same deviation.

// The workspace forbids `unwrap`/`expect`/`panic!` in library code; a test is
// where they belong.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::Mutex;

use serde_json::{Value, json};
use tinyflows::caps::{Capabilities, ToolInvoker};
use tinyflows::engine::{CancellationToken, RunInput, run_intercepted};
use tinyflows::error::Result as EngineResult;
use tinyflows::interception::StepInterceptor;
use tinyflows::model::WorkflowGraph;
use tinyflows::observability::RunObserver;
use tinyflows::testkit::{MockCaps, RunTrace, RunTracer};

use tinyloops::{
    Artifact, DelegateSet, Error, FixedPlan, Inline, LineSink, LoopState, Preset, RUN_LOOP_STEP,
    Recorder, SOLVED_MARKER, Scripted, StepRegistry, Thresholds, research_loop, run_loop_step,
};

// ------------------------------------------------------------------ fixtures

fn delegates() -> DelegateSet {
    DelegateSet::of(["prover", "refuter"])
}

fn plan() -> Arc<FixedPlan> {
    Arc::new(FixedPlan::of([
        ("bound", "bound the error term", "a proved bound on disk"),
        (
            "edge",
            "check the n = 0 edge case",
            "a proof or a counterexample",
        ),
    ]))
}

/// A preset assembled over a script, returning both the graph and the registry
/// the engine will reach through the tool.
fn assembled(
    preset: Preset,
    script: Vec<(&str, Vec<Scripted>)>,
) -> (WorkflowGraph, StepRegistry) {
    let loop_ = research_loop(
        "bound the error term in the partial sum",
        preset,
        delegates(),
        plan(),
        Arc::new(Inline::of(
            delegates(),
            script
                .into_iter()
                .map(|(role, outcomes)| (role.to_owned(), outcomes))
                .collect::<Vec<_>>(),
        )),
    )
    .expect("the preset assembles");

    let graph = loop_.graph().expect("the emitted graph validates");
    (graph, loop_.registry().clone())
}

fn answers(reply: &str, artifacts: Vec<Artifact>) -> Scripted {
    Scripted::Answers {
        reply: reply.to_owned(),
        artifacts,
    }
}

/// A run that solves on its second attempt, with the artifact to back it.
///
/// The prover's queue has **three** entries and the run has two passes, because
/// `research` briefs the first declared specialist once before the loop starts
/// and consumes an entry doing it. Spelling that out here rather than letting
/// the cursor land where it lands is the difference between a fixture that says
/// what it means and one that happens to work.
fn solving_script() -> Vec<(&'static str, Vec<Scripted>)> {
    vec![
        (
            "prover",
            vec![
                // Consumed by `research`, before the first attempt.
                answers("the second term is the hard one", Vec::new()),
                // Pass 0: work, and an artifact, but no claim.
                answers(
                    "no bound yet",
                    vec![Artifact::new("attempt-1.md", "the failed approach")],
                ),
                // Pass 1: the claim, with the artifact that makes it evidence.
                answers(
                    &format!("{SOLVED_MARKER}: the bound holds"),
                    vec![Artifact::new("bound.md", "the proof")],
                ),
            ],
        ),
        (
            "refuter",
            vec![Scripted::Capped {
                artifacts: vec![Artifact::new("search.log", "the partial search")],
            }],
        ),
    ]
}

/// Dispatches `run_loop_step` into the real registry, and records every call
/// and every answer.
///
/// No step's answer is substituted. A test that mocked the merge would prove
/// the graph reaches a node, which is what the static tests already prove; this
/// proves the node computes the right thing from what the graph handed it.
struct Steps {
    registry: StepRegistry,
    thresholds: Thresholds,
    calls: Mutex<Vec<(String, Value, Value)>>,
}

impl Steps {
    fn new(registry: StepRegistry, thresholds: Thresholds) -> Self {
        Self {
            registry,
            thresholds,
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Every `(args, answer)` pair for `step`, in order.
    fn calls_for(&self, step: &str) -> Vec<(Value, Value)> {
        self.calls
            .lock()
            .expect("the call log is not poisoned")
            .iter()
            .filter(|(name, _, _)| name == step)
            .map(|(_, args, answer)| (args.clone(), answer.clone()))
            .collect()
    }

    /// The accumulator `step` returned on its nth call.
    fn state_after(&self, step: &str, nth: usize) -> LoopState {
        let (_, answer) = self
            .calls_for(step)
            .into_iter()
            .nth(nth)
            .unwrap_or_else(|| panic!("{step} was not called {} times", nth + 1));
        serde_json::from_value(answer).expect("a step returns an accumulator")
    }
}

#[async_trait::async_trait]
impl ToolInvoker for Steps {
    async fn invoke(&self, slug: &str, args: Value, _conn: Option<&str>) -> EngineResult<Value> {
        assert_eq!(slug, RUN_LOOP_STEP, "every node body is the one tool");
        let step = args
            .get("step")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        let answer = run_loop_step(&self.registry, &args)
            .unwrap_or_else(|error| panic!("step {step} failed: {error}"));

        self.calls
            .lock()
            .expect("the call log is not poisoned")
            .push((step, args, answer.clone()));
        Ok(answer)
    }
}

async fn run(graph: &WorkflowGraph, steps: &Arc<Steps>) -> RunTrace {
    let compiled = tinyflows::compiler::compile(graph).expect("the emitted graph compiles");
    let mocks = Arc::new(MockCaps::new());
    let capabilities = Capabilities {
        tools: steps.clone(),
        ..mocks.capabilities()
    };
    let tracer = Arc::new(RunTracer::new(Some(graph.clone())));
    let observer: Arc<dyn RunObserver> = tracer.clone();

    let (_outcome, _resumable) = run_intercepted(
        &compiled,
        RunInput::new(Value::Null),
        &capabilities,
        &observer,
        CancellationToken::new(),
        tracer.clone() as Arc<dyn StepInterceptor>,
    )
    .await
    .expect("the run completes");

    tracer.trace()
}

// -------------------------------------------------------------- the merge

#[tokio::test]
async fn the_merge_node_is_handed_every_arm_and_folds_them() {
    let (graph, registry) = assembled(Preset::Balanced, solving_script());
    let steps = Arc::new(Steps::new(registry, thresholds));
    run(&graph, &steps).await;

    let merges = steps.calls_for("merge");
    assert!(!merges.is_empty(), "the merge node never ran");

    for (args, _) in &merges {
        let arms = args
            .get("arms")
            .expect("the merge is addressed with `arms`");
        // Both arms, and neither of them null. Under this engine a binding that
        // failed to resolve yields `null`, which is why the assertion is about
        // the value rather than about the key existing.
        for arm in ["reflect", "judge"] {
            let output = arms.get(arm).unwrap_or(&Value::Null);
            assert!(!output.is_null(), "arm {arm} resolved to null: {args}");
            serde_json::from_value::<LoopState>(output.clone())
                .unwrap_or_else(|error| panic!("arm {arm} is not an accumulator: {error}"));
        }
        // And the shared base, which is what every delta is computed against.
        assert!(args.get("state").is_some_and(|base| !base.is_null()));
    }
}

#[tokio::test]
async fn the_merge_carries_the_judges_verdict_into_the_accumulator() {
    // The narrative round trip, end to end through jq. The judge writes a score
    // and a verdict into the state its node returns; the merge reads them back
    // out as that arm's claim. If the round trip broke, this is the number that
    // would silently stay at its default.
    let (graph, registry) = assembled(Preset::Balanced, solving_script());
    let steps = Arc::new(Steps::new(registry, thresholds));
    run(&graph, &steps).await;

    let merged = steps.state_after("merge", 0);

    assert!(
        !merged.scores.is_empty(),
        "the judge's score did not survive the merge"
    );
    assert_eq!(merged.scores.len(), 1, "one pass, one score");
}

#[tokio::test]
async fn the_merge_folds_the_reflections_verdict_rather_than_dropping_it() {
    let (graph, registry) = assembled(Preset::Balanced, solving_script());
    let steps = Arc::new(Steps::new(registry, thresholds));
    run(&graph, &steps).await;

    // The second pass is the one whose specialist claims the goal with an
    // artifact behind it, so the reflection banks and the merge has to carry
    // both that flag and the judge's score out of the same superstep.
    let merged = steps.state_after("merge", 1);

    assert!(
        merged.solved,
        "the reflection's conclusion did not survive the merge"
    );
    assert_eq!(merged.banked, 1);
    // One score per pass, accumulating: `scores` is the history the report
    // renders, so a second pass adds to it rather than replacing it. A merge
    // that folded its own output back in would show four here.
    assert_eq!(merged.scores.len(), 2);
}

#[tokio::test]
async fn a_merge_output_is_never_the_state_it_was_handed() {
    // The regression this file exists for. The merge used to carry its input
    // through untouched, which produced a green run, a bound expression, and a
    // routing decision made on counters no arm had moved.
    let (graph, registry) = assembled(Preset::Balanced, solving_script());
    let steps = Arc::new(Steps::new(registry, thresholds));
    run(&graph, &steps).await;

    let (args, answer) = steps
        .calls_for("merge")
        .into_iter()
        .nth(1)
        .expect("a second pass merged");
    let base = args.get("state").expect("a base").clone();

    assert_ne!(answer, base, "the merge returned its input unchanged");
}

// --------------------------------------------------------------- the run

#[tokio::test]
async fn every_node_runs_and_no_expression_resolves_to_null() {
    // A graph that validates and compiles is not a graph that works: a binding
    // reading a key nothing writes resolves to `null`, the node runs, the field
    // is empty, and the run reports success.
    let (graph, registry) = assembled(Preset::Balanced, solving_script());
    let steps = Arc::new(Steps::new(registry, thresholds));
    let trace = run(&graph, &steps).await;

    assert!(
        trace.failed().is_empty(),
        "nodes failed: {:?}",
        trace
            .failed()
            .iter()
            .map(|s| &s.node_id)
            .collect::<Vec<_>>()
    );
    for step in [
        "plan", "research", "attempt", "reflect", "judge", "merge", "pass", "report",
    ] {
        assert!(!steps.calls_for(step).is_empty(), "{step} never ran");
    }
}

#[tokio::test]
async fn research_runs_once_and_the_arms_run_once_per_pass() {
    let (graph, registry) = assembled(Preset::Balanced, solving_script());
    let steps = Arc::new(Steps::new(registry, thresholds));
    run(&graph, &steps).await;

    let passes = steps.calls_for("pass").len();
    assert_eq!(
        steps.calls_for("research").len(),
        1,
        "research is not per-pass"
    );
    assert_eq!(steps.calls_for("attempt").len(), passes);
    assert_eq!(steps.calls_for("reflect").len(), passes);
    assert_eq!(steps.calls_for("judge").len(), passes);
    assert_eq!(steps.calls_for("merge").len(), passes);
    // Exactly one report, and it is the last thing that runs.
    assert_eq!(steps.calls_for("report").len(), 1);
}

#[tokio::test]
async fn every_arm_reads_the_attempt_and_never_the_accumulator() {
    // Invariant 3. The head folds at the top of a pass, so mid-body the
    // accumulator is one pass behind; an arm wired to it routes on a stale
    // answer. Asserted against the emitted arguments, because that is where the
    // wiring actually lives.
    let (graph, registry) = assembled(Preset::Balanced, solving_script());
    let steps = Arc::new(Steps::new(registry, thresholds));
    run(&graph, &steps).await;

    // The attempt's second call reports differently from its first, so an arm
    // reading a stale value would be visibly one pass behind.
    let first = steps.state_after("attempt", 0);
    let second = steps.state_after("attempt", 1);
    assert_ne!(first.last_attempt, second.last_attempt);

    let (_, reflect_on_second) = steps
        .calls_for("reflect")
        .into_iter()
        .nth(1)
        .expect("a second pass reflected");
    let reflected: LoopState = serde_json::from_value(reflect_on_second).expect("an accumulator");
    assert_eq!(
        reflected.last_attempt, second.last_attempt,
        "the arm read a stale attempt"
    );
}

#[tokio::test]
async fn the_run_ends_solved_with_the_report_composed_last() {
    let (graph, registry) = assembled(Preset::Balanced, solving_script());
    let steps = Arc::new(Steps::new(registry, thresholds));
    run(&graph, &steps).await;

    let final_state = steps.state_after("report", 0);

    assert!(final_state.solved);
    assert!(!final_state.answer.is_empty(), "report wrote no answer");
    assert!(final_state.answer.contains("bound the error term"));
}

#[tokio::test]
async fn a_run_whose_specialists_never_answer_stops_without_claiming_success() {
    let (graph, registry) = assembled(
        Preset::Balanced,
        vec![
            (
                "prover",
                vec![Scripted::NeverCompletes {
                    artifacts: Vec::new(),
                }],
            ),
            (
                "refuter",
                vec![Scripted::NeverCompletes {
                    artifacts: Vec::new(),
                }],
            ),
        ],
    );
    let steps = Arc::new(Steps::new(registry, thresholds));
    run(&graph, &steps).await;

    let final_state = steps.state_after("report", 0);

    assert!(!final_state.solved);
    assert_eq!(final_state.banked, 0);
    assert!(
        final_state.unproductive > 0,
        "an empty pass moved no counter"
    );
}

#[tokio::test]
async fn a_run_whose_machinery_never_starts_is_blocked_rather_than_stalled() {
    // Infrastructure failure is not evidence about the goal, and the ladder
    // exits on it far sooner than it exits on being stuck.
    let (graph, registry) = assembled(
        Preset::Balanced,
        vec![
            (
                "prover",
                vec![Scripted::Fails {
                    reason: "no sandbox".to_owned(),
                }],
            ),
            (
                "refuter",
                vec![Scripted::Fails {
                    reason: "no sandbox".to_owned(),
                }],
            ),
        ],
    );
    let steps = Arc::new(Steps::new(registry, thresholds));
    run(&graph, &steps).await;

    let final_state = steps.state_after("report", 0);

    assert!(final_state.blocked > 0);
    assert_eq!(final_state.unproductive, 0);
    assert!(!final_state.solved);
}

#[tokio::test]
async fn a_claim_with_no_artifact_behind_it_does_not_end_the_run() {
    // The anti-confabulation rule, end to end. The specialist says the magic
    // word on every pass and leaves nothing behind; the run must spend its
    // attempts rather than bank the claim.
    let (graph, registry) = assembled(
        Preset::Balanced,
        vec![
            (
                "prover",
                vec![answers(&format!("{SOLVED_MARKER}, trust me"), Vec::new())],
            ),
            ("refuter", vec![answers("nothing to add", Vec::new())]),
        ],
    );
    let steps = Arc::new(Steps::new(registry, thresholds));
    run(&graph, &steps).await;

    let final_state = steps.state_after("report", 0);

    assert!(!final_state.solved, "an unevidenced claim ended the run");
    assert_eq!(final_state.banked, 0);
    assert!(final_state.unverified > 0, "the near miss was not recorded");
}

#[tokio::test]
async fn a_salvaged_specialist_still_counts_as_work() {
    // A delegation killed at its own cap loses its reply and keeps its files.
    // Without salvage the pass reports nothing, `unproductive` increments on a
    // pass that produced work, and the ladder spends a diversify on a run that
    // was not stuck.
    let (graph, registry) = assembled(
        Preset::Balanced,
        vec![
            (
                "prover",
                vec![Scripted::Capped {
                    artifacts: vec![Artifact::new("partial.md", "as far as it got")],
                }],
            ),
            (
                "refuter",
                vec![Scripted::NeverCompletes {
                    artifacts: Vec::new(),
                }],
            ),
        ],
    );
    let steps = Arc::new(Steps::new(registry, thresholds));
    run(&graph, &steps).await;

    let final_state = steps.state_after("report", 0);

    assert_eq!(final_state.unproductive, 0, "salvaged work read as a stall");
    assert!(
        final_state.established > 0,
        "the artifacts were not counted"
    );
}

// ------------------------------------------- the engine and the driver agree

#[tokio::test]
async fn driving_the_loop_reaches_the_same_verdict_as_running_the_graph() {
    // The claim `presets/README.md` makes, checked rather than asserted. Both
    // paths execute the same step bodies over the same values; the engine adds
    // the graph, the jq addressing, and the concurrency, and must not add a
    // different answer.
    let script = solving_script();
    let (graph, registry) = assembled(Preset::Balanced, script.clone());
    let steps = Arc::new(Steps::new(registry, thresholds));
    run(&graph, &steps).await;
    let through_engine = steps.state_after("report", 0);

    let driven = research_loop(
        "bound the error term in the partial sum",
        Preset::Balanced,
        delegates(),
        plan(),
        Arc::new(Inline::of(
            delegates(),
            script
                .into_iter()
                .map(|(role, outcomes)| (role.to_owned(), outcomes))
                .collect::<Vec<_>>(),
        )),
    )
    .expect("assembles")
    .drive(&Recorder::new(
        "run",
        Arc::new(LineSink::new(std::io::sink())),
    ))
    .expect("the loop drives");

    assert_eq!(through_engine.solved, driven.state.solved);
    assert_eq!(through_engine.banked, driven.state.banked);
    assert_eq!(through_engine.established, driven.state.established);
    assert_eq!(through_engine.attempts, driven.state.attempts);
    assert_eq!(through_engine.answer, driven.state.answer);
}

#[tokio::test]
async fn every_preset_runs_the_same_graph_to_a_terminal_state() {
    for preset in Preset::ALL {
        let (graph, registry) = assembled(preset, solving_script());
        let steps = Arc::new(Steps::new(registry, thresholds));
        let trace = run(&graph, &steps).await;

        assert!(
            trace.failed().is_empty(),
            "{preset} failed: {:?}",
            trace
                .failed()
                .iter()
                .map(|s| &s.node_id)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            steps.calls_for("report").len(),
            1,
            "{preset} wrote no report"
        );
    }
}

#[tokio::test]
async fn a_step_the_registry_does_not_hold_is_an_error_rather_than_a_no_op() {
    // The closed step set, from the tool's side. A node naming a step nobody
    // registered runs green, changes nothing, and routes on a state nobody
    // advanced, which is the failure this returns an error for instead.
    let (_, registry) = assembled(Preset::Balanced, solving_script());

    let refused = run_loop_step(
        &registry,
        &json!({
            "step": "invented",
            "state": serde_json::to_value(LoopState::new("goal")).expect("encodes"),
        }),
    );

    assert!(matches!(refused, Err(Error::UnknownStep { ref name }) if name == "invented"));
}
