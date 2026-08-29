//! The two evaluation arms the research loop ships with.
//!
//! [`Reflect`] answers *is the answer right*, and is the only arm allowed to
//! end the run. [`Judge`] answers *was the pass conducted acceptably*, and can
//! only correct or restart. Splitting the question in two is what keeps a
//! grading verdict from becoming a stopping verdict: an arm that can say "good
//! work" and an arm that can say "we are done" answering the same prompt is one
//! arm with two names.
//!
//! # Why both of them read values
//!
//! Self-correction without an external signal makes models worse, not better:
//! they flip correct answers to incorrect more often than the reverse. So
//! neither arm here is allowed to conclude on a claim. Both read the typed
//! [`AttemptReport`] — endings, artifact counts, directives — and the
//! anti-confabulation rules below are checks against those values rather than
//! instructions in a prompt.
//!
//! The arms also receive the attempt report as *input* rather than as their own
//! prior turn. Relabelling an identical erroneous claim away from the assistant
//! role raises the explicit correction rate by 23 to 93 percentage points
//! across most model and domain pairs, and the fan-out shape is what makes that
//! natural. It must not be optimised away into a follow-up turn.

use serde_json::Value;

use crate::arm::{Arm, ArmOutcome};
use crate::error::Result;
use crate::orchestrate::AttemptReport;
use crate::policy::Judgement;
use crate::state::LoopState;
use crate::step::{NoWrite, StepContext};

/// The marker a specialist writes when it believes the goal is met.
///
/// A literal string rather than a parsed sentiment, because the marker is the
/// *cheap* half of the test and the expensive half is the evidence beside it.
pub const SOLVED_MARKER: &str = "SOLVED";

/// Reads the pass's report, or `None` when there is nothing readable there.
///
/// An unreadable report is not an error. It is the case both arms below resolve
/// toward the cheap outcome, for the reason stated on each.
fn report_of(report: &Value) -> Option<AttemptReport> {
    serde_json::from_value(report.clone()).ok()
}

/// *Is the answer right?* The only arm that may end the run.
///
/// # The three conditions for `solved`
///
/// A verdict of solved requires all of:
///
/// 1. the literal [`SOLVED_MARKER`] in some specialist's reply;
/// 2. at least one artifact on disk from this pass;
/// 3. internal consistency: the specialist that claimed it also left something
///    behind.
///
/// Any one of them alone is a claim. The marker without an artifact is a model
/// asserting completion; an artifact without the marker is ordinary work. It is
/// the conjunction that is evidence, and requiring the conjunction is the one
/// control a loop has over a verifier that is itself a model — a verifier whose
/// self-preference and position bias *grow* as the quality gap narrows, which
/// is precisely the regime a converging run is in.
#[derive(Debug, Clone, Copy, Default)]
pub struct Reflect;

impl Reflect {
    /// The arm's name, and the id of its node.
    pub const NAME: &'static str = crate::step::STEP_REFLECT;
}

impl Arm for Reflect {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn may_conclude(&self) -> bool {
        true
    }

    fn evaluate(
        &self,
        base: &LoopState,
        report: &Value,
        _ctx: StepContext<'_, NoWrite>,
    ) -> Result<ArmOutcome> {
        let mut outcome = ArmOutcome::unchanged(Self::NAME, base);
        let Some(report) = report_of(report) else {
            // An unreadable report cannot support a conclusion, and the
            // expensive mistake here is ending a run on one.
            outcome.contribution.lesson =
                Some("the attempt report was unreadable; nothing was concluded from it".to_owned());
            return Ok(outcome);
        };

        let claimed = report.outcomes.iter().any(|delegation| {
            delegation
                .reply
                .as_deref()
                .is_some_and(|reply| reply.contains(SOLVED_MARKER))
        });
        let corroborated = report.outcomes.iter().any(|delegation| {
            delegation
                .reply
                .as_deref()
                .is_some_and(|reply| reply.contains(SOLVED_MARKER))
                && !delegation.artifacts.is_empty()
        });

        if claimed && corroborated && !report.artifacts.is_empty() {
            outcome.state.solved = true;
            outcome.state.banked = base.banked.saturating_add(1);
            outcome.contribution.lesson = Some(format!(
                "solved, on {} artifact(s) left by the specialist that claimed it",
                report.artifacts.len()
            ));
            return Ok(outcome);
        }

        if claimed {
            // The confabulation case, and the one worth naming in the ledger:
            // the marker is there and the evidence is not.
            outcome.state.unverified = base.unverified.saturating_add(1);
            outcome.contribution.lesson = Some(
                "a specialist claimed the goal was met and left nothing behind; not banked"
                    .to_owned(),
            );
            return Ok(outcome);
        }

        if report.artifacts.is_empty() {
            outcome.contribution.lesson =
                Some("the pass produced no artifact to reason about".to_owned());
        }
        Ok(outcome)
    }
}

/// *Was the pass conducted acceptably?* It corrects; it never concludes.
///
/// The score is read off the report's shape — how many specialists answered,
/// how much they left behind — rather than off anybody's opinion of the work.
/// A cruder measure that is mechanical beats a finer one that is asserted,
/// because this is the number the loop's `unproductive` streak and therefore
/// its diversify rung ultimately rest on.
#[derive(Debug, Clone, Copy, Default)]
pub struct Judge;

impl Judge {
    /// The arm's name, and the id of its node.
    pub const NAME: &'static str = crate::step::STEP_JUDGE;

    /// The score for a report: answered specialists and artifacts, capped.
    ///
    /// Deliberately coarse. It is an ordering over passes, not a grade, and a
    /// scale with more resolution than the evidence supports invites reading
    /// significance into noise.
    #[must_use]
    pub fn score(report: &AttemptReport) -> u8 {
        let answered = report
            .outcomes
            .iter()
            .filter(|delegation| delegation.ending.is_answered())
            .count();
        let evidence = report.artifacts.len();
        u8::try_from((answered + evidence).min(10)).unwrap_or(10)
    }
}

impl Arm for Judge {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn evaluate(
        &self,
        base: &LoopState,
        report: &Value,
        _ctx: StepContext<'_, NoWrite>,
    ) -> Result<ArmOutcome> {
        let mut outcome = ArmOutcome::unchanged(Self::NAME, base);
        let Some(report) = report_of(report) else {
            // An unreadable verdict reads as the *cheap* outcome. Reading it as
            // `Restart` would let a serialization slip throw away a run's work,
            // which is a far worse failure than one wasted pass.
            outcome.contribution.judged = Some(Judgement::Proceed);
            outcome.contribution.score = Some(0);
            return Ok(outcome);
        };

        let score = Self::score(&report);
        outcome.contribution.score = Some(score);

        if report.is_blocked() {
            // Infrastructure, not method. Steering the approach would be
            // advice about a question the pass never reached.
            outcome.contribution.judged = Some(Judgement::Proceed);
            outcome.contribution.steer =
                Some("the machinery did not run; nothing about the approach was tested".to_owned());
            return Ok(outcome);
        }

        if report.is_informative() {
            outcome.contribution.judged = Some(Judgement::Proceed);
            return Ok(outcome);
        }

        // Nothing came back and nothing failed to start: the briefs were the
        // problem. That is a steer, and past the restart threshold it is an
        // approach problem rather than a briefing one.
        if base.unproductive.saturating_add(1) >= base.restarts.saturating_add(2) {
            outcome.contribution.judged = Some(Judgement::Restart);
            outcome.contribution.steer =
                Some("repeated passes returned nothing; begin from a different approach".to_owned());
        } else {
            outcome.contribution.judged = Some(Judgement::Steer);
            outcome.contribution.steer =
                Some("the pass returned nothing; narrow the brief and name the evidence wanted"
                    .to_owned());
        }
        Ok(outcome)
    }
}
