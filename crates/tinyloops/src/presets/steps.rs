//! The node bodies the preset supplies for the kernel nodes that are not the
//! orchestrator's.
//!
//! [`crate::orchestrate`] supplies `plan`, `attempt`, and `report`. The graph
//! reaches five more names, and the closed step set is checked at build time,
//! so a loop assembled here has to answer to all of them. Each type below says
//! what it does, and [`Converge`] says plainly what it cannot do yet and why.

use std::sync::Arc;

use crate::arm::Arm;
use crate::error::{Error, Result};
use crate::harness::{Brief, Ending};
use crate::orchestrate::{DelegateSet, Specialists};
use crate::state::LoopState;
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
/// It does **not** apply the arm's [`Contribution`](crate::Contribution). Those
/// are folded by [`ArmSet::merge`](crate::ArmSet::merge) across every arm at
/// once, because a lesson and a steer written by two arms have to be checked
/// for collision, and an arm that applied its own would have made that check
/// impossible before it ran.
#[derive(Debug)]
pub struct ArmStep {
    arm: Arc<dyn Arm>,
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
        Ok(ctx.advance(outcome.state))
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

/// `merge`: the barrier every arm converges on.
///
/// **What it does here: nothing to the state.** That is a limitation worth
/// stating rather than hiding. The emitted merge node is handed each arm's
/// output through its tool arguments — `{"base": …, "arms": {"reflect": …,
/// "judge": …}}` — but [`run_loop_step`](crate::run_loop_step) passes a step
/// only the decoded `state`, so a [`Step`] implementation cannot reach the arm
/// outputs it would need to fold. Widening that interface is the loop kernel's
/// decision, not this module's.
///
/// The fold itself is written and tested: it is
/// [`ArmSet::merge`](crate::ArmSet::merge), which
/// [`AssembledLoop::drive`](super::AssembledLoop::drive) calls with every arm's
/// outcome. So a driven loop folds correctly today, and a loop run through an
/// engine will fold correctly once the step interface carries the node's
/// arguments. Registering this rather than leaving `merge` unregistered is what
/// keeps the graph buildable and the gap visible in one place instead of
/// surfacing as an `UnknownStep` nobody can act on.
#[derive(Debug, Clone, Copy, Default)]
pub struct Converge;

impl Step for Converge {
    fn name(&self) -> &'static str {
        crate::loops::STEP_MERGE
    }

    fn run(&self, state: LoopState, ctx: StepContext<'_, CanWrite>) -> Result<Advanced> {
        Ok(ctx.advance(state))
    }
}
