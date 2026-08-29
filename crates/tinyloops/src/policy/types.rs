//! The numbers a loop is configured with, and the small closed vocabularies a
//! turn's outcome is expressed in.
//!
//! Everything here is data. The decisions made from it live in the module root,
//! and the jq translation of those decisions lives in `ladder.rs`.

use serde::{Deserialize, Serialize};

use crate::state::LoopState;
use crate::{Error, Result};

/// The limits and trigger points one loop runs under.
///
/// Every field is a *bet* about when one strategy stops paying and another
/// starts, not a fact about the work. They are gathered into one struct so the
/// bets are written down in a single place, versioned with the code, and read
/// by both the Rust router and the generated jq — see
/// [`ladder`](super::ladder).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Thresholds {
    /// How many attempts a goal gets before the run reports what it has.
    ///
    /// The bet: past roughly eight attempts, further attempts on the same goal
    /// are answering a question the run has already failed to answer, and the
    /// cheaper move is to hand a human what was learned.
    pub max_attempts: u32,
    /// Consecutive unproductive passes before the loop diversifies.
    ///
    /// The bet, and the most consequential one here: two consecutive
    /// unproductive passes is an *estimator* of the point where sequential
    /// revision stops beating parallel sampling. Revising one attempt over and
    /// over conditions every next attempt on the same failed framing; drawing
    /// several independent attempts does not. Two is where this framework
    /// commits to the crossover being behind it.
    ///
    /// It is a methodological commitment, not a measurement, and the failure
    /// worth naming is leaving it unrecorded — a threshold nobody wrote down is
    /// a threshold nobody can argue with, revise, or tune per domain. It is
    /// written here so it can be all three.
    pub stuck: u32,
    /// Consecutive infrastructure-only failures before the run stops.
    ///
    /// The bet: two passes that learned nothing about the goal because the
    /// machinery would not run are not evidence about the goal, and continuing
    /// to spend attempts on them buys nothing. Infrastructure failure is not
    /// the work, and the loop should say so rather than absorb it.
    pub blocked: u32,
    /// Consecutive passes gaining only a larger instance of prior work.
    ///
    /// The bet: two passes producing more of the same shape means the approach
    /// has been fully exploited, and scale is now standing in for insight.
    /// Diversifying is worth more than another lap.
    pub computational: u32,
    /// Consecutive single-route answers before the run reports rather than
    /// banks.
    ///
    /// The bet: an answer reached twice by exactly one route is still one
    /// route's worth of evidence. Reporting it as unverified is honest; banking
    /// it as solved is a claim the run cannot support.
    pub unverified: u32,
    /// How many fresh starts a run gets.
    ///
    /// The bet: a third restart is not a new approach, it is the first one
    /// again with different words. Two bounds the cost of thrashing.
    pub max_restarts: u32,
    /// How often the run stops to re-plan, in passes.
    ///
    /// The bet: every third pass is frequent enough that a wrong plan is caught
    /// before it consumes the attempt budget, and rare enough that planning
    /// does not become the work.
    pub plan_interval: u32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            max_attempts: 8,
            stuck: 2,
            blocked: 2,
            computational: 2,
            unverified: 2,
            max_restarts: 2,
            plan_interval: 3,
        }
    }
}

impl Thresholds {
    /// Whether the run should re-plan after `passes` passes.
    ///
    /// A [`Self::plan_interval`] of zero disables planning rather than dividing
    /// by zero. Guarding here is deliberate: a configuration read from a host
    /// can hold anything, and a panic inside a graph node is far worse than a
    /// loop that never re-plans.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tinyloops::Thresholds;
    /// let thresholds = Thresholds::default();
    /// assert!(!thresholds.plans_on(0));
    /// assert!(thresholds.plans_on(3));
    /// assert!(!thresholds.plans_on(4));
    /// ```
    #[must_use]
    pub fn plans_on(&self, passes: u32) -> bool {
        self.plan_interval != 0 && passes != 0 && passes.is_multiple_of(self.plan_interval)
    }
}

/// Where a turn's outcome sends the run next.
///
/// The variants are ordered as the ladder tests them, most specific first; see
/// [`route`](super::route) for why that order is load-bearing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Route {
    /// The machinery failed, repeatedly. Stop; this is not about the goal.
    Blocked,
    /// The run has an answer, or has spent its attempts trying to get one.
    Solved,
    /// There is an answer, but only one route reached it. Hand it over as a
    /// report rather than banking it.
    Reported,
    /// Sequential revision has stopped paying. Sample a different approach.
    Diversify,
    /// Nothing special happened. Go round again.
    #[default]
    Retry,
}

impl Route {
    /// The wire name of this route.
    ///
    /// Written out by hand, never derived from [`Debug`]. A `Debug` rendering
    /// is a diagnostic, not a wire format: it changes when a variant is
    /// renamed, and the generated jq — which must emit exactly these strings —
    /// would then produce names [`Self::parse`] no longer recognises, reading
    /// back as the default and quietly re-routing the loop.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::Solved => "solved",
            Self::Reported => "reported",
            Self::Diversify => "diversify",
            Self::Retry => "retry",
        }
    }

    /// Reads a route name, leniently.
    ///
    /// Anything unrecognised — a typo, a name from a newer revision, an empty
    /// string where a node produced nothing — resolves to [`Self::Retry`],
    /// which is the *cheap* outcome: one more pass round a loop that is already
    /// bounded by its attempt budget.
    ///
    /// The corollary matters more than the rule. A lenient parse must never
    /// fall through to a *terminal* value, because a misspelling would then end
    /// runs: the run would stop, report, and look for all the world like it had
    /// decided to. Falling through to a cheap value costs one pass and is
    /// visible in the counters.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tinyloops::Route;
    /// assert_eq!(Route::parse("diversify"), Route::Diversify);
    /// assert_eq!(Route::parse("  BLOCKED "), Route::Blocked);
    /// assert_eq!(Route::parse("divrsify"), Route::Retry);
    /// ```
    #[must_use]
    pub fn parse(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "blocked" => Self::Blocked,
            "solved" => Self::Solved,
            "reported" => Self::Reported,
            "diversify" => Self::Diversify,
            // "retry", and everything the loop cannot read.
            _ => Self::Retry,
        }
    }

    /// Whether this route ends the run.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Blocked | Self::Solved | Self::Reported)
    }
}

/// A judge's verdict on one pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Judgement {
    /// The pass advanced the work. Carry on as planned.
    #[default]
    Proceed,
    /// The pass was heading the wrong way. Carry on, with a correction.
    Steer,
    /// The approach is wrong, not the execution. Begin again.
    Restart,
}

impl Judgement {
    /// The wire name of this verdict.
    ///
    /// Hand-written for the same reason as [`Route::as_str`]: a `Debug`
    /// rendering is not a wire format, and a rename would silently read back as
    /// the default.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proceed => "proceed",
            Self::Steer => "steer",
            Self::Restart => "restart",
        }
    }

    /// Reads a verdict, leniently.
    ///
    /// Anything unrecognised resolves to [`Self::Proceed`], the cheap outcome:
    /// a verdict the loop cannot read must not throw work away by accident, and
    /// [`Self::Restart`] discards everything the pass built.
    ///
    /// As with [`Route::parse`], the fallback is deliberately not a terminal or
    /// destructive value — a misspelled verdict costs nothing, while a
    /// misspelled verdict that restarted the run would cost the run.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tinyloops::Judgement;
    /// assert_eq!(Judgement::parse("Restart"), Judgement::Restart);
    /// assert_eq!(Judgement::parse("restrt"), Judgement::Proceed);
    /// ```
    #[must_use]
    pub fn parse(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "steer" => Self::Steer,
            "restart" => Self::Restart,
            // "proceed", and everything the loop cannot read.
            _ => Self::Proceed,
        }
    }
}

/// How much a run may do without asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Autonomy {
    /// Decide nothing; describe what would be done.
    ///
    /// The default, because the conservative setting is the one that is safe to
    /// get wrong.
    #[default]
    Report,
    /// Act, pausing at the decisions a human asked to keep.
    Assisted,
    /// Act to the end of the budget without pausing.
    Unattended,
}

/// How a finished run came out.
///
/// The invariant this type exists to hold: an error or an exhausted budget is
/// never [`Self::Success`]. It is enforced by construction —
/// [`Self::classify`] never returns `Success` for such a run, and
/// [`Self::success`] refuses to build one — rather than written as a comment
/// that the next caller to assemble an `Outcome` by hand would not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The goal was reached and something was banked.
    Success,
    /// The goal was reached and there was legitimately nothing to change.
    CleanNoOp,
    /// The machinery failed; the run learned nothing about the goal.
    Blocked,
    /// The run ran out of ideas before it ran out of budget.
    Stalled,
    /// The run ran out of budget: attempts, or the clock.
    Exhausted,
}

impl Outcome {
    /// Classifies a finished run.
    ///
    /// Total, and ordered so the disqualifying conditions are read first: a
    /// blocked run is blocked whatever else it claims, and a run that is out of
    /// attempts or out of time is exhausted even if the last pass sounded
    /// hopeful.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tinyloops::{LoopState, Outcome, Thresholds};
    /// let mut state = LoopState::new("goal");
    /// state.solved = true;
    /// state.banked = 1;
    /// assert_eq!(Outcome::classify(&state, &Thresholds::default()), Outcome::Success);
    ///
    /// state.expired = true;
    /// assert_eq!(Outcome::classify(&state, &Thresholds::default()), Outcome::Exhausted);
    /// ```
    #[must_use]
    pub fn classify(state: &LoopState) -> Self {
        let thresholds = &state.profile.thresholds;
        if state.blocked >= thresholds.blocked {
            Self::Blocked
        } else if state.expired || state.attempts >= thresholds.max_attempts {
            Self::Exhausted
        } else if state.solved {
            if state.banked == 0 {
                Self::CleanNoOp
            } else {
                Self::Success
            }
        } else {
            Self::Stalled
        }
    }

    /// Builds [`Self::Success`] for `state`, or refuses.
    ///
    /// The checked conversion the invariant is spent on: a caller that wants to
    /// declare a run successful has to pass the run, and a run that was blocked,
    /// expired, or out of attempts cannot be declared successful at all.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnearnedSuccess`] when [`Self::classify`] does not
    /// classify `state` as a success.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tinyloops::{Error, LoopState, Outcome, Thresholds};
    /// let mut state = LoopState::new("goal");
    /// state.solved = true;
    /// state.banked = 1;
    /// assert_eq!(Outcome::success(&state)?, Outcome::Success);
    ///
    /// state.expired = true;
    /// assert_eq!(
    ///     Outcome::success(&state).unwrap_err(),
    ///     Error::UnearnedSuccess,
    /// );
    /// # Ok::<(), tinyloops::Error>(())
    /// ```
    pub fn success(state: &LoopState) -> Result<Self> {
        match Self::classify(state) {
            Self::Success => Ok(Self::Success),
            _ => Err(Error::UnearnedSuccess),
        }
    }
}
