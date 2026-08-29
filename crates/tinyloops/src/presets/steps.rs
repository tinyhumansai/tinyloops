//! The node bodies the preset supplies for the kernel nodes that are not the
//! orchestrator's.
//!
//! [`crate::orchestrate`] supplies `plan`, `attempt`, and `report`. The graph
//! reaches five more names, and the closed step set is checked at build time,
//! so a loop assembled here has to answer to all of them. Each type below says
//! what it does, and [`Converge`] says plainly what it cannot do yet and why.

use std::sync::Arc;

use crate::arm::{Arm, ArmOutcome, ArmSet};
use crate::error::{Error, Result};
use crate::harness::{Brief, Ending};
use crate::orchestrate::{DelegateSet, Specialists};
use crate::state::{Contribution, LoopState};
use crate::step::{Advanced, CanWrite, STEP_PASS, STEP_RESEARCH, Step, StepContext};

/// `research`: one round of context gathering, before anything is attempted.
///
/// It runs once, ahead of the loop, so the first attempt already has something
/// to work from rather than spending itself acquiring it. What comes back is
/// appended to [`LoopState::lessons`], which is the field every subsequent
/// brief carries forward.
///
/// It deliberately moves no counter. Research is not an attempt at the goal,
/// and letting it reset `unproductive` would let a run stay out of the
/// diversify branch by looking things up.
#[derive(Debug)]
pub struct Gather {
    delegates: DelegateSet,
    specialists: Arc<dyn Specialists>,
}

impl Gather {
    /// The research step, briefing the first declared specialist.
    #[must_use]
    pub fn new(delegates: DelegateSet, specialists: Arc<dyn Specialists>) -> Self {
        Self {
            delegates,
            specialists,
        }
    }
}

impl Step for Gather {
    fn name(&self) -> &'static str {
        STEP_RESEARCH
    }

    fn run(&self, state: LoopState, ctx: StepContext<'_, CanWrite>) -> Result<Advanced> {
        let mut state = state;
        let Some(role) = self.delegates.names().next().map(str::to_owned) else {
            return Err(Error::EmptyDelegateSet);
        };

        let brief = Brief::new(format!("gather context for: {}", state.goal));
        let outcomes = self.specialists.dispatch(vec![(role, brief)])?;
        for outcome in outcomes {
            if outcome.ending == Ending::Failed {
                continue;
            }
            if let Some(reply) = outcome.reply {
                state.lessons.push(format!("from research: {reply}"));
            }
        }
        Ok(ctx.advance(state))
    }
}

/// An evaluation arm, as the step its node runs.
///
/// The graph addresses an arm's node by the arm's own name and invokes it
/// through the same `run_loop_step` tool as every other node, so an arm needs a
/// [`Step`] as well as an [`Arm`]. This adapter is that, and nothing else: it
/// reads the pass's one attempt report out of [`LoopState::last_attempt`],
/// hands it to the arm, and returns the candidate state the arm computed.
///
/// It **does** apply the arm's [`Contribution`](crate::Contribution) to the
/// state it returns, and that is load-bearing rather than a convenience. A
/// node returns one accumulator; there is nowhere else for a lesson or a steer
/// to ride. [`Converge`] reads the claims back out with
/// [`Contribution::claimed_from`](crate::Contribution::claimed_from) — a field
/// that differs from the shared base is a field this arm claimed — so two arms
/// writing the same one is still [`Error::ContestedField`] at the merge rather
/// than a winner picked by arrival order.
///
/// The two halves are inverses and are tested as such. If they ever stop being
/// inverses, an arm's contribution silently stops reaching the accumulator,
/// which is exactly the class of failure this crate is built to refuse.
pub struct ArmStep {
    arm: Arc<dyn Arm>,
}

impl std::fmt::Debug for ArmStep {
    /// Hand-written because [`Arm`] is not [`Debug`]: an arm holds whatever a
    /// deployment's evaluator needs, and requiring it to be renderable would
    /// put a bound on the seam for the sake of a diagnostic. The name is the
    /// part worth printing anyway.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArmStep")
            .field("arm", &self.arm.name())
            .finish()
    }
}

impl ArmStep {
    /// The step that runs `arm`.
    #[must_use]
    pub fn new(arm: Arc<dyn Arm>) -> Self {
        Self { arm }
    }
}

impl Step for ArmStep {
    fn name(&self) -> &'static str {
        self.arm.name()
    }

    fn run(&self, state: LoopState, ctx: StepContext<'_, CanWrite>) -> Result<Advanced> {
        // An arm reads the attempt report, never the accumulator: the head
        // folds at the top of a pass, so mid-body the accumulator is one pass
        // behind and an arm reading it routes on a stale answer.
        let report = if state.last_attempt.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str(&state.last_attempt).unwrap_or(serde_json::Value::Null)
        };

        let observing = StepContext::observing(ctx.pass(), ctx.thresholds());
        let outcome = self.arm.evaluate(&state, &report, observing)?;

        let mut advanced = outcome.state;
        outcome.contribution.apply_to(&mut advanced);
        Ok(ctx.advance(advanced))
    }
}

/// `pass`: the one node every route enters, and the only one closing the cycle.
///
/// It counts the pass and consumes the steer. Consuming it here rather than in
/// `attempt` is what stops a correction being applied twice: the steer is
/// written by the judge during a pass and read by the *next* pass's briefs, so
/// exactly one node has to clear it, and that node is the one every route goes
/// through.
///
/// The count is an assignment rather than an increment, because the fold is
/// at-least-once: a replayed activation after a resume applies it twice, and
/// `passes = n + 1` computed from a stale `n` is wrong in a way `passes += 1`
/// is not visibly wrong.
#[derive(Debug, Clone, Copy, Default)]
pub struct Advance;

impl Step for Advance {
    fn name(&self) -> &'static str {
        STEP_PASS
    }

    fn run(&self, state: LoopState, ctx: StepContext<'_, CanWrite>) -> Result<Advanced> {
        let mut state = state;
        state.passes = ctx.pass().saturating_add(1);
        state.steer = String::new();
        Ok(ctx.advance(state))
    }
}

/// `merge`: the barrier every arm converges on, and the fold.
///
/// It is the one node body handed more than one input. Its node is addressed
/// with `state` — the attempt's output, the shared base every arm was given —
/// and `arms`, an object mapping each arm's name to the whole accumulator that
/// arm returned. Both arrive as node arguments rather than in the state,
/// because the state is one value and a barrier reads several.
///
/// # The fold
///
/// Counters merge by addition, computed as a delta against the shared base, so
/// a reset from one arm and an increment from another land together instead of
/// overwriting one another and the result does not depend on the order the arms
/// finished in. Narrative merges by exclusive ownership: each field is claimed
/// by at most one arm, recovered with
/// [`Contribution::claimed_from`](crate::Contribution::claimed_from), and two
/// arms claiming the same one is [`Error::ContestedField`] rather than a winner
/// picked by arrival order.
///
/// Both laws live in [`ArmSet::merge`](crate::ArmSet::merge), which this calls.
/// There is deliberately no second implementation here: the driven path calls
/// the same function with the same base, so a loop run through the engine and
/// the same loop driven in-process fold identically.
///
/// # What it refuses
///
/// An arm's output that will not decode as an accumulator is
/// [`Error::MalformedStepPayload`], not a skipped arm. Under this engine an
/// expression that failed to resolve yields `null`, so silently dropping an
/// undecodable arm would turn a broken binding into a merge that quietly folded
/// fewer arms than the graph fanned out to — a route taken on evidence nobody
/// gathered. `ArmSet::merge` separately refuses a fold that is missing a
/// declared arm.
pub struct Converge {
    arms: ArmSet,
}

impl std::fmt::Debug for Converge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Converge")
            .field(
                "arms",
                &self
                    .arms
                    .arms()
                    .iter()
                    .map(|arm| arm.name())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl Converge {
    /// The merge step, folding exactly the arms `arms` declares.
    ///
    /// Taking the [`ArmSet`] rather than deriving the arm names from the
    /// arguments is what keeps "every arm converges" and "every arm is folded"
    /// one fact: an arm missing from the arguments is an error here rather than
    /// a smaller fold nobody notices.
    #[must_use]
    pub fn new(arms: ArmSet) -> Self {
        Self { arms }
    }

    /// Decodes one arm's returned accumulator out of the `arms` argument.
    fn outcome_of(
        arms: &serde_json::Value,
        arm: &'static str,
        base: &LoopState,
    ) -> Result<ArmOutcome> {
        let returned = arms
            .get(arm)
            .filter(|value| !value.is_null())
            .ok_or(Error::MalformedStepPayload { field: "arms" })?;
        let state: LoopState = serde_json::from_value(returned.clone())
            .map_err(|_| Error::MalformedStepPayload { field: "arms" })?;

        Ok(ArmOutcome {
            contribution: Contribution::claimed_from(arm, base, &state),
            state,
        })
    }
}

impl Step for Converge {
    fn name(&self) -> &'static str {
        crate::loops::STEP_MERGE
    }

    fn run(&self, state: LoopState, ctx: StepContext<'_, CanWrite>) -> Result<Advanced> {
        // The base is the state the node was handed: the attempt's output, and
        // the same value every arm was given. Computing each delta against it
        // is what makes the fold order-independent.
        let Some(arms) = ctx.arg("arms") else {
            return Err(Error::MalformedStepPayload { field: "arms" });
        };

        let outcomes = self
            .arms
            .arms()
            .iter()
            .map(|arm| Self::outcome_of(arms, arm.name(), &state))
            .collect::<Result<Vec<_>>>()?;

        Ok(ctx.advance(self.arms.merge(&state, outcomes)?))
    }
}
