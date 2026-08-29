//! The emitted graph, run.
//!
//! A graph that validates and compiles is not a graph that works. Under this
//! engine a binding that reads a key nothing writes resolves to `null`, the
//! node runs, the field is empty, and the run reports success — so the
//! assertion that carries this file is `assert_no_null_bindings`, and the rest
//! measures behaviour a static reading of the JSON cannot see.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use tinyflows::caps::Capabilities;
use tinyflows::data::Item;
use tinyflows::engine::{CancellationToken, RunInput, run_intercepted};
use tinyflows::interception::{StepAction, StepFrame, StepInterceptor, StepPhase};
use tinyflows::model::WorkflowGraph;
use tinyflows::observability::RunObserver;
use tinyflows::testkit::{MockCaps, Respond, TestHarness};

use tinyloops::{
    Advanced, Arm, ArmOutcome, ArmSet, Autonomy, CanWrite, LoopBuilder, LoopState, NoWrite,
    NodeIds, Result, STEP_MERGE, Step, StepContext, StepRegistry, Thresholds,
};

/// A step body that changes nothing: every node's answer comes from a mock or
/// an interceptor, so the graph's own wiring is what is under test.
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

/// Three attempts is enough to tell "current" from "one pass behind".
fn thresholds() -> Thresholds {
    Thresholds {
        max_attempts: 3,
        ..Thresholds::default()
    }
}

/// The graph under test.
fn graph() -> WorkflowGraph {
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

    LoopBuilder::new(thresholds(), arms, registry)
        .goal("ship the release")
        .autonomy(Autonomy::Unattended)
        .build()
        .expect("the fixture builds a valid graph")
}

/// A `LoopState` as JSON, with `edit` applied.
fn state_with(edit: impl FnOnce(&mut LoopState)) -> Value {
    let mut state = LoopState::new("ship the release");
    edit(&mut state);
    serde_json::to_value(state).expect("a state encodes")
}

/// A tool-call envelope around `state`, as the engine would wrap it.
fn envelope(state: Value) -> Value {
    json!({ "json": state.clone(), "text": Value::Null, "raw": state })
}

/// The states `pass` returns, one per iteration, ending on the attempt cap.
fn pass_sequence() -> Respond {
    Respond::sequence((1..=thresholds().max_attempts).map(|attempts| {
        Respond::value(state_with(|state| {
            state.attempts = attempts;
            state.passes = attempts;
        }))
    }))
}

/// A harness with every node answered.
///
/// Node-scoped rules come first: the doubles answer with the first matching
/// rule, so a general fallback registered ahead of them would swallow every
/// call.
fn harness(graph: &WorkflowGraph) -> TestHarness {
    TestHarness::new(graph)
        .mock_tool("run_loop_step", pass_sequence())
        .only_from("pass")
        .mock_tool("run_loop_step", Respond::value(state_with(|_| {})))
}

#[tokio::test]
async fn a_run_completes_and_binds_every_expression() {
    let graph = graph();
    let run = harness(&graph).run().await.expect("the run compiles and runs");
    run.assert_completed();
    // The assertion a green run hides: a generated ladder that failed to
    // compile, or an address the engine's `nodes` scope does not project,
    // produces `null` rather than an error, so a clean outcome is not by itself
    // evidence that the wiring resolved.
    run.assert_no_null_bindings();

    let ids = NodeIds::default();
    for node in [
        ids.plan,
        ids.research,
        ids.side_arms,
        ids.loop_head,
        ids.attempt,
        "reflect",
        "judge",
        ids.merge,
        ids.route,
        ids.pass,
        ids.stand_down,
        ids.report,
    ] {
        run.assert_node_ran(node);
    }
}

#[tokio::test]
async fn pass_runs_exactly_once_per_iteration() {
    let graph = graph();
    let run = harness(&graph).run().await.expect("the run compiles and runs");
    run.assert_completed();

    let iterations = usize::try_from(thresholds().max_attempts).expect("a small cap fits");
    assert_eq!(run.node_output("pass").len(), iterations);
    assert_eq!(run.trace().steps_for("attempt").len(), iterations);
    assert_eq!(run.trace().steps_for("merge").len(), iterations);
    // The head runs once more than the body: the extra activation is the one
    // that folds the last pass and takes the `done` port.
    assert_eq!(run.trace().steps_for("loop").len(), iterations + 1);
}

#[tokio::test]
async fn an_arm_reading_the_accumulator_is_handed_a_one_pass_stale_report() {
    // **This is invariant 3's only real test, and it only works because the
    // attempt mock's answer varies per call.** A mock returning a constant
    // produces the same report on every pass, so "one pass behind" and
    // "current" are indistinguishable and both wirings pass — the weaker test
    // is not coverage.
    let attempts = Respond::sequence((1..=thresholds().max_attempts).map(|n| {
        Respond::value(state_with(|state| {
            state.last_attempt = format!("attempt {n}");
        }))
    }));

    let correct = graph();
    let mut rewired = correct.clone();
    let arm = rewired
        .nodes
        .iter_mut()
        .find(|node| node.id == "judge")
        .expect("the arm is emitted");
    // The bug, written out: the head folds at the *top* of a pass, so mid-body
    // the accumulator holds the state as of the previous one.
    arm.config["args"]["report"] = json!(NodeIds::default().accumulator_address());

    let mut seen = Vec::new();
    for graph in [&correct, &rewired] {
        let run = TestHarness::new(graph)
            .mock_tool("run_loop_step", attempts.clone())
            .only_from("attempt")
            .mock_tool("run_loop_step", pass_sequence())
            .only_from("pass")
            .mock_tool("run_loop_step", Respond::value(state_with(|_| {})))
            .run()
            .await
            .expect("the run compiles and runs");
        run.assert_completed();

        let reports: Vec<String> = run
            .trace()
            .calls_from("judge")
            .iter()
            .map(|call| {
                call.args["report"]["last_attempt"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        seen.push(reports);
    }

    let [correct_reports, stale_reports] = <[Vec<String>; 2]>::try_from(seen)
        .expect("two runs produce two histories");

    assert_eq!(
        correct_reports,
        ["attempt 1", "attempt 2", "attempt 3"],
        "a correctly wired arm reads this pass's attempt",
    );
    assert_ne!(
        stale_reports, correct_reports,
        "an arm reading the accumulator sees a different history",
    );
    assert_eq!(
        stale_reports.first().map(String::as_str),
        Some(""),
        "on the first pass the accumulator holds only the seed",
    );
}

/// Replaces every tool call's output with a fixed enveloped state, and counts
/// how often it saw the `pass` node.
///
/// A [`StepInterceptor`], not a mock capability: what an interceptor returns is
/// **obeyed**, before and after every non-trigger activation, while a
/// `RunObserver` callback returns `()` and can only watch.
struct ReplayPass {
    /// The state every tool call reports, so the fold's input is constant and
    /// the only variable left is how many times it is applied.
    state: Value,
    passes: AtomicUsize,
}

#[async_trait::async_trait]
impl StepInterceptor for ReplayPass {
    async fn intercept(&self, frame: StepFrame<'_>) -> StepAction {
        if frame.phase != StepPhase::Before {
            return StepAction::Continue { state_patch: None };
        }
        let is_tool_call = frame.node.config.get("slug").is_some();
        if !is_tool_call {
            return StepAction::Continue { state_patch: None };
        }
        if frame.node.id == "pass" {
            // The replay: the second activation is handed the *first* one's
            // output again, which is what a resume that re-runs a committed
            // activation does.
            self.passes.fetch_add(1, Ordering::SeqCst);
        }
        StepAction::Replace {
            items: vec![Item::new(envelope(self.state.clone()))],
            port: None,
        }
    }
}

/// A `RunObserver` that watches nothing; every callback has a default.
struct Silent;

impl RunObserver for Silent {}

#[tokio::test]
async fn replaying_one_activation_leaves_every_counter_unchanged() {
    let graph = graph();
    let compiled = tinyflows::compiler::compile(&graph).expect("the emitted graph compiles");
    let mocks = Arc::new(MockCaps::new());
    let capabilities: Capabilities = mocks.capabilities();
    let interceptor = Arc::new(ReplayPass {
        state: state_with(|state| state.attempts = 2),
        passes: AtomicUsize::new(0),
    });
    let observer: Arc<dyn RunObserver> = Arc::new(Silent);

    let (outcome, _resumable) = run_intercepted(
        &compiled,
        RunInput::new(Value::Null),
        &capabilities,
        &observer,
        CancellationToken::new(),
        interceptor.clone(),
    )
    .await
    .expect("the run completes");

    let applied = interceptor.passes.load(Ordering::SeqCst);
    assert!(
        applied > 1,
        "the fold has to run more than once for a replay to be observable",
    );

    // `config.state.update` assigns the whole state the pass returned, so
    // applying the same activation `applied` times lands on the same value as
    // applying it once. An increment would read `2 * applied` here, and nothing
    // in the engine would report it.
    let accumulator = &outcome.output["nodes"]["loop"]["state"];
    assert_eq!(accumulator["attempts"], json!(2));
    assert_eq!(accumulator["passes"], json!(0));
}
