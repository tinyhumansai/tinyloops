//! A loop with every seam already filled in, ready to build or to drive.
//!
//! [`AssembledLoop`] is the batteries. It holds the thresholds, the closed step
//! set, the arms, the budget, and the node ids, and it does two things with
//! them: [`AssembledLoop::graph`] emits the `WorkflowGraph` an engine runs, and
//! [`AssembledLoop::drive`] runs the same loop in Rust.
//!
//! # Why both
//!
//! They are not two implementations of the routing. Both call
//! [`route`](crate::route); the graph's ladder is *generated* from the same
//! constants and proved against that function exhaustively by the parity sweep,
//! so the two agree by construction rather than by review. What differs is what
//! owns the concurrency and the durability: under the engine a pass's arms run
//! concurrently and a checkpoint survives a crash, while [`AssembledLoop::drive`]
//! runs them in order in one process.
//!
//! `drive` exists because the engine's mock capabilities are a dev-only
//! dependency here, so a shipped library cannot start a graph run, and because
//! a loop you can call from a test with no runtime, no scheduler, and no
//! provider is the loop most people should meet first.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use crate::arm::ArmSet;
use crate::budget::{Bound, Meter, RunBudget};
use crate::error::{Error, Result};
use crate::loops::{GraphSignature, LoopBuilder, NodeIds, STEP_MERGE};
use crate::observe::{Event, Movement, Recorder};
use crate::orchestrate::{
    Attempt, Decompose, DelegateSet, Orchestrator, Plan, Report as ReportStep, Specialists,
    Summarize,
};
use crate::policy::{Autonomy, Outcome, Route, Thresholds, route};
use crate::state::LoopState;
use crate::step::{STEP_ATTEMPT, STEP_PLAN, STEP_REPORT, STEP_RESEARCH, StepRegistry};
use crate::tools::ToolGrant;

use super::arms::{Judge, Reflect};
use super::steps::{Advance, ArmStep, Converge, Gather};
use super::types::Preset;

/// How a driven run came out.
///
/// Carries the whole final accumulator rather than a summary of it, because
/// every number a caller might want — the route history's last rung, what was
/// banked, which tasks are still open — is already in there, and a parallel
/// summary is a second thing to keep in step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Driven {
    /// The accumulator as the last pass left it.
    pub state: LoopState,
    /// How the run is classified.
    pub outcome: Outcome,
    /// The route each pass took, oldest first.
    pub routes: Vec<Route>,
    /// The bound that stopped the run, when one did.
    pub bound: Option<Bound>,
}

impl Driven {
    /// The run's final answer, as the `report` step composed it.
    #[must_use]
    pub fn answer(&self) -> &str {
        &self.state.answer
    }
}

/// A loop with every seam filled in.
///
/// Build one with [`research_loop`], or with [`Self::new`] when the defaults do
/// not fit.
#[derive(Debug)]
pub struct AssembledLoop {
    goal: String,
    preset: Preset,
    thresholds: Thresholds,
    arms: ArmSet,
    registry: StepRegistry,
    budget: RunBudget,
    ids: NodeIds,
    autonomy: Autonomy,
}

impl AssembledLoop {
    /// Assembles a loop from parts that are already validated.
    ///
    /// # Errors
    ///
    /// Whatever the parts raise. Nothing is validated here that was not
    /// validated where it was built: an [`ArmSet`] refused its own duplicates
    /// and its second concluding arm at construction, and a [`StepRegistry`]
    /// refused its own duplicate names.
    pub fn new(
        goal: impl Into<String>,
        preset: Preset,
        arms: ArmSet,
        registry: StepRegistry,
        budget: RunBudget,
    ) -> Result<Self> {
        Ok(Self {
            goal: goal.into(),
            preset,
            thresholds: preset.thresholds(),
            arms,
            registry,
            budget,
            ids: NodeIds::default(),
            autonomy: Autonomy::Unattended,
        })
    }

    /// Runs the loop at `autonomy` instead of unattended.
    #[must_use]
    pub fn autonomy(mut self, autonomy: Autonomy) -> Self {
        self.autonomy = autonomy;
        self
    }

    /// Runs the loop under different limits.
    ///
    /// Separate from the preset because a threshold and a bound answer
    /// different questions: a threshold decides where a pass routes, and a
    /// bound decides whether there is another pass at all. Tightening one
    /// should never silently move the other.
    #[must_use]
    pub fn with_budget(mut self, budget: RunBudget) -> Self {
        self.budget = budget;
        self
    }

    /// Renames the loop's nodes, for a host running two loops in one workflow.
    #[must_use]
    pub fn ids(mut self, ids: NodeIds) -> Self {
        self.ids = ids;
        self
    }

    /// The preset this loop was assembled from.
    #[must_use]
    pub fn preset(&self) -> Preset {
        self.preset
    }

    /// The thresholds it routes on.
    #[must_use]
    pub fn thresholds(&self) -> &Thresholds {
        &self.thresholds
    }

    /// The limits it carries.
    #[must_use]
    pub fn budget(&self) -> &RunBudget {
        &self.budget
    }

    /// Emits the graph an engine would run.
    ///
    /// # Errors
    ///
    /// Whatever [`LoopBuilder::build`] raises: an unknown step, an accumulator
    /// that will not serialize, or a graph the engine's validator rejects.
    pub fn graph(&self) -> Result<tinyflows::model::WorkflowGraph> {
        LoopBuilder::new(self.thresholds, self.arms.clone(), self.registry.clone())
            .goal(self.goal.clone())
            .autonomy(self.autonomy)
            .ids(self.ids)
            .name(format!("tinyloops::{}", self.preset))
            .build()
    }

    /// The signature a checkpoint of this loop would carry.
    ///
    /// Two assemblies of the same preset produce the same signature, and a
    /// changed threshold produces a different one, which is what makes an
    /// incompatible resume an error rather than silent corruption.
    ///
    /// # Errors
    ///
    /// Whatever [`Self::graph`] raises.
    pub fn signature(&self) -> Result<GraphSignature> {
        Ok(GraphSignature::of(&self.graph()?))
    }

    /// Runs the loop to a terminal state, in this process.
    ///
    /// `plan` and `research` run once before the loop. One pass is then: `plan`
    /// on its cadence, `attempt`, every arm over the one attempt report, the
    /// merge, and the route. The loop stops when
    /// the route is terminal, the thresholds say the state is, or a bound
    /// trips — and a bound is never reported as success.
    ///
    /// `recorder` receives the loop's own vocabulary as it goes: pass
    /// boundaries, arm timings, the merge, the verdict, the route, and the
    /// bound that ended it.
    ///
    /// # Errors
    ///
    /// - Whatever a step or an arm raises. A step that cannot run is a failed
    ///   pass, not a silently skipped one.
    /// - [`Error::ContestedField`] when two arms claim the same narrative
    ///   field, which is a wiring mistake with no correct resolution.
    pub fn drive(&self, recorder: &Recorder) -> Result<Driven> {
        let mut state = LoopState::new(self.goal.clone());
        let mut meter = Meter::default();
        let mut routes = Vec::new();
        let mut bound = None;
        let caps = self.budget.caps();

        // `plan` and `research` run before the loop, in that order, so the
        // first attempt already has a decomposition and a context to work from
        // rather than spending itself acquiring them. `research` runs once and
        // only once: it is not an attempt at the goal, and repeating it every
        // pass would spend a specialist re-reading what the run already knows.
        state = self.run_step(STEP_PLAN, state, 0, recorder)?;
        state = self.run_step(STEP_RESEARCH, state, 0, recorder)?;

        for pass in 0..caps.max_iterations {
            recorder.record(Event::PassStarted { pass });
            state.passes = pass;

            state = self.run_step(STEP_PLAN, state, pass, recorder)?;
            state = self.run_step(STEP_ATTEMPT, state, pass, recorder)?;

            state = self.evaluate(&state, pass, recorder)?;

            let chosen = route(&state, &self.thresholds);
            routes.push(chosen);
            recorder.record(Event::Routed {
                pass,
                route: chosen,
                // The counters, not a sentence about them. A route that cannot
                // be re-derived from the reason beside it is a route nobody can
                // audit after the fact.
                reason: format!(
                    "unproductive={} blocked={} unverified={} attempts={}",
                    state.unproductive, state.blocked, state.unverified, state.attempts
                ),
            });
            recorder.record(Event::Judged {
                pass,
                judgement: state.judged,
                score: state.scores.last().copied().unwrap_or_default(),
            });

            meter.pass(!state.last_attempt.is_empty());
            state.passes = pass.saturating_add(1);
            recorder.record(Event::PassFinished {
                pass,
                duration: Duration::ZERO,
            });

            if let Some(tripped) = self.budget.tripped(&meter) {
                bound = Some(tripped);
                recorder.record(Event::BoundTripped {
                    pass,
                    bound: tripped,
                });
                break;
            }
            if crate::policy::is_terminal(&state, &self.thresholds) {
                break;
            }
        }

        if bound.is_none() && state.passes >= caps.max_iterations {
            bound = Some(Bound::Iterations);
            recorder.record(Event::BoundTripped {
                pass: state.passes,
                bound: Bound::Iterations,
            });
        }

        let last = state.passes;
        state = self.run_step(STEP_REPORT, state, last, recorder)?;

        // A run stopped by a bound is never `Success`, whatever its last pass
        // claimed. `classify` reads `expired` and the attempt cap; an iteration
        // or a token cap has to be folded in here, and folding it in *after*
        // the classification is what keeps that rule in one place.
        let classified = Outcome::classify(&state, &self.thresholds);
        let outcome = match bound {
            Some(tripped) if !tripped.is_graceful() => Outcome::Exhausted,
            Some(_) if classified == Outcome::Stalled => Outcome::Exhausted,
            _ => classified,
        };
        recorder.record(Event::LoopFinished {
            pass: state.passes,
            outcome,
        });

        Ok(Driven {
            state,
            outcome,
            routes,
            bound,
        })
    }

    /// Runs one named step, announcing entry and exit.
    ///
    /// Every step announces both, without exception. A live run of this design
    /// printed no orchestrator line for 62 minutes, and which node was holding
    /// could only be inferred from which sub-agents happened to spawn: "the run
    /// stalled" has to be a question the log answers.
    fn run_step(
        &self,
        name: &'static str,
        state: LoopState,
        pass: u32,
        recorder: &Recorder,
    ) -> Result<LoopState> {
        recorder.record(Event::StepEntered {
            pass,
            step: name.to_owned(),
        });
        let advanced = self.registry.run(name, state, &self.thresholds)?;
        recorder.record(Event::StepFinished {
            pass,
            step: name.to_owned(),
            duration: Duration::ZERO,
        });
        Ok(advanced)
    }

    /// Runs every arm over one report and folds the answers.
    ///
    /// **This is the graph's own path, in one process.** Each arm is run
    /// through its registered step, its whole returned accumulator is collected
    /// under its name, and the merge step is invoked with exactly the arguments
    /// the emitted `merge` node is addressed with. There is no second fold and
    /// no shortcut: a loop driven here and the same loop run through an engine
    /// execute the same step bodies over the same values.
    ///
    /// The arms all read the one attempt report and none reads another's
    /// output, which is what makes them independent and therefore concurrent
    /// under an engine. Running them in order here changes the wall clock and
    /// nothing else: the fold is by delta against one shared base, so the
    /// result does not depend on the order they finished in.
    fn evaluate(&self, state: &LoopState, pass: u32, recorder: &Recorder) -> Result<LoopState> {
        let mut returned = serde_json::Map::new();
        let mut deltas = Vec::new();

        for arm in self.arms.arms() {
            recorder.record(Event::ArmStarted {
                pass,
                arm: arm.name().to_owned(),
            });
            let candidate =
                self.registry
                    .run(arm.name(), state.clone(), &self.thresholds)?;
            deltas.push(candidate.delta_from(state));
            returned.insert(
                arm.name().to_owned(),
                serde_json::to_value(&candidate).map_err(|_| Error::StateEncoding)?,
            );
            recorder.record(Event::ArmFinished {
                pass,
                arm: arm.name().to_owned(),
                duration: Duration::ZERO,
            });
        }

        // One `Merged` per pass carrying the summed movement, because that is
        // what the fold actually applied. Reporting each arm's delta separately
        // would invite a reader to add them up by hand and get a different
        // answer to the one the accumulator took.
        let summed = deltas.iter().fold(Movement::default(), |acc, delta| {
            let movement = Movement::from(delta);
            Movement {
                passes: acc.passes + movement.passes,
                attempts: acc.attempts + movement.attempts,
                unproductive: acc.unproductive + movement.unproductive,
                blocked: acc.blocked + movement.blocked,
                computational: acc.computational + movement.computational,
                unverified: acc.unverified + movement.unverified,
                restarts: acc.restarts + movement.restarts,
                established: acc.established + movement.established,
                banked: acc.banked + movement.banked,
                solved: movement.solved.or(acc.solved),
                expired: movement.expired.or(acc.expired),
            }
        });
        recorder.record(Event::Merged {
            pass,
            arms: deltas.len(),
            movement: summed,
        });

        self.registry.run_with(
            STEP_MERGE,
            state.clone(),
            &self.thresholds,
            &json!({ "arms": Value::Object(returned) }),
        )
    }
}

/// The shipped research loop: an orchestrator, two arms, and a preset.
///
/// It assembles [`Plan`], [`Attempt`], and [`ReportStep`] over `decompose`,
/// `specialists`, and [`Summarize`], with [`Reflect`] and [`Judge`] as the
/// arms. The orchestrator holds a read-only grant, so the rule that keeps a
/// driver commissioning work rather than doing it is enforced by this function
/// rather than trusted to its caller.
///
/// # Errors
///
/// - [`Error::EmptyDelegateSet`] when `delegates` names nobody.
/// - [`Error::ExecutionToolInOrchestrator`] never, in practice: the grant is
///   fixed here.
/// - Whatever [`ArmSet::new`], [`StepRegistry::register`], or [`RunBudget`]
///   raise.
///
/// # Examples
///
/// ```
/// # use std::sync::Arc;
/// # use tinyloops::{
/// #     DelegateSet, FixedPlan, Inline, Preset, Recorder, Scripted, research_loop,
/// # };
/// let delegates = DelegateSet::of(["prover"]);
/// let assembled = research_loop(
///     "bound the error term",
///     Preset::Balanced,
///     delegates.clone(),
///     Arc::new(FixedPlan::of([("bound", "bound the error term", "a proved bound")])),
///     Arc::new(Inline::of(
///         delegates,
///         [("prover".to_owned(), vec![Scripted::Answers {
///             reply: "no luck".to_owned(),
///             artifacts: Vec::new(),
///         }])],
///     )),
/// )?;
///
/// // It emits a graph an engine can run, and drives itself without one.
/// assert!(assembled.graph().is_ok());
/// let sink = Arc::new(tinyloops::LineSink::new(std::io::sink()));
/// let driven = assembled.drive(&Recorder::new("run", sink))?;
/// assert!(!driven.answer().is_empty());
/// # Ok::<(), tinyloops::Error>(())
/// ```
pub fn research_loop(
    goal: impl Into<String>,
    preset: Preset,
    delegates: DelegateSet,
    decompose: Arc<dyn Decompose>,
    specialists: Arc<dyn Specialists>,
) -> Result<AssembledLoop> {
    let delegates_for_research = delegates.clone();
    let orchestrator = Orchestrator::new(ToolGrant::read_only(), delegates)?;
    let mailbox = Arc::new(crate::harness::Mailbox::new(
        crate::harness::DEFAULT_MAILBOX_CAPACITY,
    ));

    let reflect: Arc<dyn crate::arm::Arm> = Arc::new(Reflect);
    let judge: Arc<dyn crate::arm::Arm> = Arc::new(Judge);

    // Every name the emitted graph reaches, registered here rather than left to
    // a caller. The step set is closed and checked at build time, so a missing
    // one is a build error; supplying them is what makes this function a preset
    // rather than a constructor with homework attached.
    let mut registry = StepRegistry::new();
    registry.register(Arc::new(Plan::new(decompose)))?;
    registry.register(Arc::new(Gather::new(
        delegates_for_research,
        Arc::clone(&specialists),
    )))?;
    registry.register(Arc::new(Attempt::new(orchestrator, specialists, mailbox)))?;
    registry.register(Arc::new(ArmStep::new(Arc::clone(&reflect))))?;
    registry.register(Arc::new(ArmStep::new(Arc::clone(&judge))))?;
    let arms = ArmSet::new(vec![reflect, judge])?;
    registry.register(Arc::new(Converge::new(arms.clone())))?;
    registry.register(Arc::new(Advance))?;
    registry.register(Arc::new(ReportStep::new(Arc::new(Summarize))))?;

    AssembledLoop::new(goal, preset, arms, registry, RunBudget::default())
}
