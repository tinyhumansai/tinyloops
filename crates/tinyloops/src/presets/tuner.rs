//! The shipped tuner: a pure function of the counters, and why it is not a
//! model.
//!
//! [`Rules`] reads the accumulator and proposes at most one [`Amendment`] a
//! pass. Every rule is arithmetic over fields the run already carries, so the
//! whole behavior is testable at every boundary by driving a counter sequence
//! and asserting the exact passes it fires on.
//!
//! # Why the default is rules and not a model
//!
//! A model asked mid-run whether its own configuration is wrong has no ground
//! truth to answer from and every incentive to answer yes — the same pressure
//! that makes a model claim the goal is met on the eighth pass. Worse, the
//! tuner and the loop it tunes are the same system, so a model tuner rewarded
//! by the loop's own signals can improve the signal instead of the work.
//!
//! A model tuner is still permitted: it implements the same [`Tuner`] trait and
//! is bounded by the same [`Bounds`](crate::Bounds), which is precisely why the
//! bounds live outside the proposer.
//!
//! # Why the muting rule is conservative
//!
//! The obvious way to retire an arm is to score arms against each other and
//! drop the worst, which is what bandit arm-elimination does. It needs a
//! measured reward per arm, and this loop has none: an arm contributes a delta
//! and a narrative, not a score of its own. So [`Rules`] fires on *silence* —
//! an arm whose signal has not varied for a window of passes — and never on
//! "scored worse", because it has no such comparison to make.

use serde_json::Value;

use crate::arm::Tuner;
use crate::policy::{Amendment, CapField, Change, LoopProfile, ThresholdField};
use crate::state::LoopState;
use crate::step::{NoWrite, StepContext};
use crate::{Result, step::STEP_JUDGE};

/// How many identical scores in a row read as a judge carrying no signal,
/// when nothing says otherwise.
///
/// Three, matching [`DEFAULT_MUTING_WINDOW`](crate::DEFAULT_MUTING_WINDOW).
/// Two is a coincidence; three is a pattern cheap enough to act on and cheap
/// enough to be wrong about, since the only cost of a wrong mute is one arm's
/// work on a run that was going to spend it anyway.
pub const SILENT_SCORES: usize = 3;

/// The rule-based tuner.
///
/// Carries the muting window a deployment's [`Bounds`](crate::Bounds)
/// configured, and nothing else it needs from the accumulator: everything is
/// in the state handed to [`Tuner::propose`], which is what makes it safe to
/// run under a replay — proposing twice from the same state proposes the same
/// thing, and the `pass` step folds one proposal once.
#[derive(Debug, Clone, Copy)]
pub struct Rules {
    window: usize,
}

impl Default for Rules {
    /// A tuner that mutes on [`SILENT_SCORES`] consecutive identical scores —
    /// the shipped presets' declared window, matched here so a caller who
    /// wires up [`Rules::default`] directly sees the same behavior the
    /// presets do.
    fn default() -> Self {
        Self {
            window: SILENT_SCORES,
        }
    }
}

impl Rules {
    /// The arm's name, and the id of its node.
    pub const NAME: &'static str = "tune";

    /// A tuner that mutes an arm after `window` consecutive identical judge
    /// scores, rather than the default [`SILENT_SCORES`].
    ///
    /// `window` should come from the same [`Bounds::muting_window`] the
    /// assembled loop folds amendments within — otherwise the tuner proposes
    /// on a cadence the bounds were never asked about.
    ///
    /// [`Bounds::muting_window`]: crate::Bounds::muting_window
    #[must_use]
    pub fn new(window: u32) -> Self {
        Self {
            window: usize::try_from(window).unwrap_or(usize::MAX),
        }
    }

    /// Whether `profile` has already been asked to move `field`.
    ///
    /// Proposing the same move twice is how a tuner spends its whole amendment
    /// budget arriving where one proposal would have put it. Refusals count
    /// too: a bound that said no once will say no again, and re-asking buys a
    /// second identical refusal in the run's record.
    fn already_asked(profile: &LoopProfile, field: ThresholdField) -> bool {
        profile.history.iter().any(|recorded| {
            matches!(
                recorded.amendment.change,
                Change::Threshold { field: asked, .. } if asked == field
            )
        })
    }

    /// Whether `profile` has already been asked to move a cap.
    fn already_capped(profile: &LoopProfile, field: CapField) -> bool {
        profile.history.iter().any(|recorded| {
            matches!(
                recorded.amendment.change,
                Change::Cap { field: asked, .. } if asked == field
            )
        })
    }

    /// The last `SILENT_SCORES` scores, when they exist and are all equal.
    fn scores_are_flat(state: &LoopState) -> bool {
        let scores = &state.scores;
        scores.len() >= SILENT_SCORES
            && scores[scores.len() - SILENT_SCORES..]
                .windows(2)
                .all(|pair| pair[0] == pair[1])
    }

    /// The infrastructure rule.
    ///
    /// A run one pass from its blocked ceiling is a run the machinery is
    /// failing, and more attempts cannot answer that. Halving the model-call
    /// allowance is the only move available that costs less rather than more.
    fn on_blocked(state: &LoopState) -> Option<Change> {
        let thresholds = &state.profile.thresholds;
        let near = thresholds.blocked.saturating_sub(1).max(1);
        if state.blocked < near || Self::already_capped(&state.profile, CapField::MaxModelCalls) {
            return None;
        }
        let halved = (state.profile.caps.max_model_calls / 2).max(1);
        Some(Change::Cap {
            field: CapField::MaxModelCalls,
            to: u64::from(halved),
        })
    }

    /// The patience rule.
    ///
    /// `unproductive` strictly past `stuck` means the run has already
    /// diversified once and the pass after it was unproductive too — so the
    /// variation the threshold bought did not pay, and the run's own estimate
    /// of where sequential revision stops beating sampling was too low for this
    /// domain.
    fn on_diversifying_badly(state: &LoopState) -> Option<Change> {
        let stuck = state.profile.thresholds.stuck;
        if state.unproductive <= stuck || Self::already_asked(&state.profile, ThresholdField::Stuck)
        {
            return None;
        }
        Some(Change::Threshold {
            field: ThresholdField::Stuck,
            to: stuck.saturating_add(1),
        })
    }

    /// The silence rule.
    ///
    /// A judge returning the same score for a window of passes is a judge whose
    /// score is carrying no information, and the run is paying for it every
    /// pass. Muting it is not a claim that it was wrong.
    fn on_silence(state: &LoopState) -> Option<Change> {
        if !Self::scores_are_flat(state) || state.profile.is_muted(STEP_JUDGE) {
            return None;
        }
        Some(Change::MuteArm {
            arm: STEP_JUDGE.to_owned(),
        })
    }

    /// The reason line that travels with `change`.
    fn because(state: &LoopState, change: &Change) -> String {
        match change {
            Change::Cap { .. } => format!(
                "{} consecutive blocked passes; the machinery is failing, not the work",
                state.blocked
            ),
            Change::Threshold { .. } => format!(
                "unproductive is {} against a stuck of {}: diversifying did not pay",
                state.unproductive, state.profile.thresholds.stuck
            ),
            Change::MuteArm { arm } => format!(
                "{arm} has scored {:?} for {SILENT_SCORES} passes and is carrying no signal",
                state.scores.last().copied().unwrap_or_default()
            ),
            Change::UnmuteArm { arm } => format!("{arm} is wanted again"),
        }
    }
}

impl Tuner for Rules {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn propose(
        &self,
        base: &LoopState,
        _report: &Value,
        ctx: StepContext<'_, NoWrite>,
    ) -> Result<Option<Amendment>> {
        // Ordered, and the order is the policy: infrastructure first, because a
        // run the machinery is failing has learned nothing about its own
        // patience; then patience; then what the run is paying for and not
        // reading. At most one proposal a pass, so a busy pass does not spend
        // the whole amendment budget at once.
        let change = Self::on_blocked(base)
            .or_else(|| Self::on_diversifying_badly(base))
            .or_else(|| Self::on_silence(base));

        Ok(change.map(|change| {
            let because = Self::because(base, &change);
            Amendment::new(Self::NAME, ctx.pass(), change, because)
        }))
    }
}
