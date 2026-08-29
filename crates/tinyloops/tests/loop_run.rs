//! The emitted graph, run.
//!
//! A graph that validates and compiles is not a graph that works. Under this
//! engine a binding that reads a key nothing writes resolves to `null`, the
//! node runs, the field is empty, and the run reports success — so the
//! assertion that carries this file is that no `=`-binding resolved to null,
//! and the rest measures behaviour a static reading of the JSON cannot see.
//!
//! # Why the runs here are assembled rather than driven by `TestHarness`
//!
//! `tinyflows::testkit` hands **every node activation a freshly constructed
//! `TokioTaskRunner`** (`vendor/tinyflows/src/testkit/mocks.rs`), so a ticket
//! `side_arms` issues is unknown to `stand_down` when it comes to collect it,
//! and the run dies on `unknown task ticket`. That is a property of the double,
//! not of the graph: with one set of capabilities for the whole run — which is
//! what a host provides — the spawn and the gate agree. So these tests supply
//! one `Capabilities`, drive it with `run_intercepted`, and use `RunTracer`
//! (with no mocks attached, so it does not substitute per-node capabilities)
//! for the trace. `a_run_completes_under_the_test_harness` below is the one
//! that goes through `TestHarness` proper, for `assert_no_null_bindings`
//! itself, and says what it had to patch to get there.

// The workspace forbids `unwrap`/`expect`/`panic!` in library code; a test is
// where they belong, and the same allowance every other test module in this
// crate carries.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use tinyflows::caps::{Capabilities, ToolInvoker};
use tinyflows::data::Item;
use tinyflows::engine::{CancellationToken, RunInput, RunOutcome, run_intercepted};
use tinyflows::error::Result as EngineResult;
use tinyflows::interception::{StepAction, StepFrame, StepInterceptor, StepPhase};
use tinyflows::model::{NodeKind, WorkflowGraph};
use tinyflows::observability::RunObserver;
use tinyflows::testkit::{MockCaps, Respond, RunTrace, RunTracer, TestHarness};

use tinyloops::{
    Advanced, Arm, ArmOutcome, ArmSet, Autonomy, CanWrite, LoopBuilder, LoopState, NoWrite,
    NodeIds, RUN_LOOP_STEP, Result, STEP_MERGE, Step, StepContext, StepRegistry, Thresholds,
};

/// A step body that changes nothing: every node's answer comes from the tool
/// double below, so the graph's own wiring is what is under test.
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
fn envelope(state: &Value) -> Value {
    json!({ "json": state.clone(), "text": Value::Null, "raw": state.clone() })
}

/// Answers `run_loop_step`, records every call, and — for the attempt —
/// answers **differently on every call**.
///
/// The varying answer is not a flourish. It is the only thing that makes an
/// arm's staleness observable: a double that returns a constant produces the
/// same report on every pass, so "one pass behind" and "current" are
/// indistinguishable and every wiring passes.
#[derive(Default)]
struct Steps {
    calls: Mutex<Vec<(String, Value)>>,
    attempts: AtomicUsize,
    passes: AtomicUsize,
}

impl Steps {
    /// Every call made for `step`, in order.
    fn calls_for(&self, step: &str) -> Vec<Value> {
        self.calls
            .lock()
            .expect("the call log is not poisoned")
            .iter()
            .filter(|(name, _)| name == step)
            .map(|(_, args)| args.clone())
            .collect()
    }

    fn count(&self, step: &str) -> usize {
        self.calls_for(step).len()
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
        self.calls
            .lock()
            .expect("the call log is not poisoned")
            .push((step.clone(), args));

        Ok(match step.as_str() {
            "attempt" => {
                let nth = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
                state_with(|state| state.last_attempt = format!("attempt {nth}"))
            }
            "pass" => {
                // Walks the accumulator up to the cap, so the head's `until`
                // fires on the last pass rather than the run hitting its
                // iteration ceiling.
                let nth = self.passes.fetch_add(1, Ordering::SeqCst) + 1;
                let nth = u32::try_from(nth).unwrap_or(u32::MAX);
                state_with(|state| {
                    state.attempts = nth;
                    state.passes = nth;
                })
            }
            _ => state_with(|_| {}),
        })
    }
}

/// Runs `graph` with one set of capabilities, optionally under `interceptor`.
async fn run(
    graph: &WorkflowGraph,
    steps: &Arc<Steps>,
    interceptor: Option<Arc<dyn StepInterceptor>>,
) -> (RunOutcome, RunTrace) {
    let compiled = tinyflows::compiler::compile(graph).expect("the emitted graph compiles");
    let mocks = Arc::new(MockCaps::new());
    let capabilities = Capabilities {
        tools: steps.clone(),
        ..mocks.capabilities()
    };
    let tracer = Arc::new(RunTracer::new(Some(graph.clone())));
    let observer: Arc<dyn RunObserver> = tracer.clone();
    let interceptor = interceptor.unwrap_or_else(|| tracer.clone() as Arc<dyn StepInterceptor>);

    let (outcome, _resumable) = run_intercepted(
        &compiled,
        RunInput::new(Value::Null),
        &capabilities,
        &observer,
        CancellationToken::new(),
        interceptor,
    )
    .await
    .expect("the run completes");
    (outcome, tracer.trace())
}

#[tokio::test]
async fn a_run_completes_and_binds_every_expression() {
    let graph = graph();
    let steps = Arc::new(Steps::default());
    let (_outcome, trace) = run(&graph, &steps, None).await;

    assert!(
        trace.failed().is_empty(),
        "nodes failed: {:?}",
        trace
            .failed()
            .iter()
            .map(|s| &s.node_id)
            .collect::<Vec<_>>(),
    );
    // The check a green run hides. A generated ladder that failed to compile,
    // or an address the engine's `nodes` scope does not project, yields `null`
    // rather than an error, so a clean outcome is not by itself evidence.
    assert!(
        trace.null_bindings().is_empty(),
        "these bindings resolved to null: {:?}",
        trace
            .null_bindings()
            .iter()
            .map(|(node, binding)| (*node, binding.expression.clone()))
            .collect::<Vec<_>>(),
    );

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
        assert!(trace.ran(node), "{node} never ran");
    }
}

#[tokio::test]
async fn a_run_completes_under_the_test_harness() {
    // `assert_no_null_bindings` is the acceptance criterion, so it is called
    // here rather than reimplemented. The one patch is the gate: the testkit
    // gives every activation its own `TokioTaskRunner`, so the ticket
    // `side_arms` issued cannot be found by `stand_down`. Emptying the gate's
    // wait list is the smallest change that lets the *rest* of the graph be
    // exercised by the harness; the emitted wiring is asserted in
    // `src/loops/test.rs` instead.
    let mut graph = graph();
    let ids = NodeIds::default();
    let gate = graph
        .nodes
        .iter_mut()
        .find(|node| node.id == ids.stand_down)
        .expect("the gate is emitted");
    gate.config = json!({ "tickets": [], "release": "all" });

    let run = TestHarness::new(&graph)
        .mock_tool(RUN_LOOP_STEP, Respond::value(state_with(|_| {})))
        .run()
        .await
        .expect("the run compiles and runs");

    run.assert_completed();
    run.assert_no_null_bindings();
    run.assert_node_ran(ids.report);
}

#[tokio::test]
async fn pass_runs_exactly_once_per_iteration() {
    let graph = graph();
    let steps = Arc::new(Steps::default());
    let (_outcome, trace) = run(&graph, &steps, None).await;

    let iterations = usize::try_from(thresholds().max_attempts).expect("a small cap fits");
    assert_eq!(trace.steps_for("pass").len(), iterations);
    assert_eq!(steps.count("pass"), iterations);
    assert_eq!(trace.steps_for("attempt").len(), iterations);
    assert_eq!(trace.steps_for("merge").len(), iterations);
    // The head runs once more than the body: the extra activation is the one
    // that folds the last pass and takes the `done` port.
    assert_eq!(trace.steps_for("loop").len(), iterations + 1);
}

#[tokio::test]
async fn an_arm_reading_the_accumulator_is_handed_a_one_pass_stale_report() {
    // **This is invariant 3's only real test, and it works only because the
    // attempt's answer varies per call.** A double returning a constant
    // produces the same report on every pass, so "one pass behind" and
    // "current" are indistinguishable and both wirings pass — the weaker
    // version is not coverage, and is why this comment is here.
    let correct = graph();
    let mut rewired = correct.clone();
    let arm = rewired
        .nodes
        .iter_mut()
        .find(|node| node.id == "judge")
        .expect("the arm is emitted");
    // The bug, written out: the head folds at the *top* of a pass, so mid-body
    // the accumulator holds the state as of the previous one.
    arm.config["args"]["state"] = json!(NodeIds::default().accumulator_address());

    let mut histories = Vec::new();
    for graph in [&correct, &rewired] {
        let steps = Arc::new(Steps::default());
        let (_outcome, _trace) = run(graph, &steps, None).await;
        let seen: Vec<String> = steps
            .calls_for("judge")
            .iter()
            .map(|args| {
                args["state"]["last_attempt"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        histories.push(seen);
    }

    let stale = histories.pop().expect("two runs produce two histories");
    let current = histories.pop().expect("two runs produce two histories");

    assert_eq!(
        current,
        ["attempt 1", "attempt 2", "attempt 3"],
        "a correctly wired arm reads this pass's attempt",
    );
    assert_ne!(
        stale, current,
        "an arm reading the accumulator sees a different history",
    );
    assert_eq!(
        stale.first().map(String::as_str),
        Some(""),
        "on the first pass the accumulator holds only what the seed carried",
    );
}

/// Replaces every `tool_call`'s output with one fixed enveloped state, so the
/// only variable left is how many times the head folds it.
///
/// A [`StepInterceptor`], not a mock capability: what an interceptor returns is
/// **obeyed**, before and after every non-trigger activation, while a
/// `RunObserver` callback returns `()` and can only watch. It deliberately does
/// not touch the `spawn` node, whose ticket the gate has to recognise.
struct ReplayPass {
    state: Value,
    folds: AtomicUsize,
}

#[async_trait::async_trait]
impl StepInterceptor for ReplayPass {
    async fn intercept(&self, frame: StepFrame<'_>) -> StepAction {
        if frame.phase != StepPhase::Before || frame.node.kind != NodeKind::ToolCall {
            return StepAction::Continue { state_patch: None };
        }
        if frame.node.id == "pass" {
            // Every activation is handed the *first* one's output again, which
            // is what a resume that re-runs a committed activation does.
            self.folds.fetch_add(1, Ordering::SeqCst);
        }
        StepAction::Replace {
            items: vec![Item::new(envelope(&self.state))],
            port: None,
        }
    }
}

#[tokio::test]
async fn replaying_one_activation_leaves_every_counter_unchanged() {
    let graph = graph();
    let steps = Arc::new(Steps::default());
    let interceptor = Arc::new(ReplayPass {
        state: state_with(|state| state.attempts = 2),
        folds: AtomicUsize::new(0),
    });
    let (outcome, _trace) = run(
        &graph,
        &steps,
        Some(interceptor.clone() as Arc<dyn StepInterceptor>),
    )
    .await;

    let folds = interceptor.folds.load(Ordering::SeqCst);
    assert!(
        folds > 1,
        "the fold has to run more than once for a replay to be observable",
    );

    // `config.state.update` assigns the whole state the pass returned, so
    // applying the same activation `folds` times lands where applying it once
    // would. An increment would read `2 * folds` here, and nothing in the
    // engine would report it.
    let accumulator = &outcome.output["nodes"]["loop"]["state"];
    assert_eq!(accumulator["attempts"], json!(2));
    assert_eq!(accumulator["passes"], json!(0));
}
