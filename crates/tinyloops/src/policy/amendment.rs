//! What a run may change about itself, and the record of having changed it.
//!
//! An [`Amendment`] is one proposed move of one field of the run's
//! [`LoopProfile`](super::LoopProfile), carrying the evidence for it. It is the
//! only way a profile moves.
//!
//! # Why [`Change`] is a closed enum and not a patch
//!
//! A JSON patch, or anything else that addresses the accumulator by path, can
//! reach the counters the routing ladder reads. A tuner able to emit one is a
//! tuner able to write `solved`, and the loop would have no way to tell a
//! configuration change from a claim about the work. The variants here are the
//! whole vocabulary, and adding to it is a deliberate edit rather than a
//! consequence of a proposer getting more expressive.
//!
//! There is no variant for the re-plan cadence, because there does not need to
//! be: `plan_interval` is a [`Thresholds`](super::Thresholds) field like any
//! other, so [`ThresholdField::PlanInterval`] already names it.
//!
//! # Why every change is total
//!
//! [`Change::apply_to`] cannot fail. Validation happens once, in
//! [`Bounds::check`](super::Bounds), before anything is applied — so a proposal
//! is either refused whole, with its reason recorded, or applied whole. A change
//! that could half-apply would leave a profile nobody chose and no event
//! describing it.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{LoopProfile, Thresholds};
use crate::budget::Caps;

/// A field of [`Thresholds`] an amendment can move.
///
/// One variant per field, so a proposal names a field rather than a path. The
/// wire names are the field names, which is what lets a reader of a run's events
/// match an amendment to the threshold it moved without a lookup table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThresholdField {
    /// [`Thresholds::max_attempts`].
    MaxAttempts,
    /// [`Thresholds::stuck`].
    Stuck,
    /// [`Thresholds::blocked`].
    Blocked,
    /// [`Thresholds::computational`].
    Computational,
    /// [`Thresholds::unverified`].
    Unverified,
    /// [`Thresholds::max_restarts`].
    MaxRestarts,
    /// [`Thresholds::plan_interval`].
    PlanInterval,
}

impl ThresholdField {
    /// Every field, in declaration order.
    pub const ALL: [Self; 7] = [
        Self::MaxAttempts,
        Self::Stuck,
        Self::Blocked,
        Self::Computational,
        Self::Unverified,
        Self::MaxRestarts,
        Self::PlanInterval,
    ];

    /// The field's name, as it appears on the wire and in an event.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MaxAttempts => "max_attempts",
            Self::Stuck => "stuck",
            Self::Blocked => "blocked",
            Self::Computational => "computational",
            Self::Unverified => "unverified",
            Self::MaxRestarts => "max_restarts",
            Self::PlanInterval => "plan_interval",
        }
    }

    /// Reads this field out of `thresholds`.
    #[must_use]
    pub const fn read(self, thresholds: &Thresholds) -> u32 {
        match self {
            Self::MaxAttempts => thresholds.max_attempts,
            Self::Stuck => thresholds.stuck,
            Self::Blocked => thresholds.blocked,
            Self::Computational => thresholds.computational,
            Self::Unverified => thresholds.unverified,
            Self::MaxRestarts => thresholds.max_restarts,
            Self::PlanInterval => thresholds.plan_interval,
        }
    }

    /// Writes `value` into this field of `thresholds`.
    pub const fn write(self, thresholds: &mut Thresholds, value: u32) {
        match self {
            Self::MaxAttempts => thresholds.max_attempts = value,
            Self::Stuck => thresholds.stuck = value,
            Self::Blocked => thresholds.blocked = value,
            Self::Computational => thresholds.computational = value,
            Self::Unverified => thresholds.unverified = value,
            Self::MaxRestarts => thresholds.max_restarts = value,
            Self::PlanInterval => thresholds.plan_interval = value,
        }
    }
}

impl std::fmt::Display for ThresholdField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A field of [`Caps`] an amendment can move.
///
/// The clocks are absent, and deliberately: a run that could extend its own
/// wall clock has no wall clock. What is here is the counted work — calls,
/// tokens, retries — where lowering the ceiling is the useful move and raising
/// it is what [`Bounds`](super::Bounds) is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapField {
    /// [`Caps::max_model_calls`].
    MaxModelCalls,
    /// [`Caps::max_tool_calls`].
    MaxToolCalls,
    /// [`Caps::max_tokens`].
    MaxTokens,
    /// [`Caps::max_retries`].
    MaxRetries,
}

impl CapField {
    /// Every field, in declaration order.
    pub const ALL: [Self; 4] = [
        Self::MaxModelCalls,
        Self::MaxToolCalls,
        Self::MaxTokens,
        Self::MaxRetries,
    ];

    /// The field's name, as it appears on the wire and in an event.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MaxModelCalls => "max_model_calls",
            Self::MaxToolCalls => "max_tool_calls",
            Self::MaxTokens => "max_tokens",
            Self::MaxRetries => "max_retries",
        }
    }

    /// Reads this field out of `caps`.
    #[must_use]
    pub const fn read(self, caps: &Caps) -> u64 {
        match self {
            Self::MaxModelCalls => caps.max_model_calls as u64,
            Self::MaxToolCalls => caps.max_tool_calls as u64,
            Self::MaxTokens => caps.max_tokens,
            Self::MaxRetries => caps.max_retries as u64,
        }
    }

    /// Writes `value` into this field of `caps`, saturating at the field's own
    /// width.
    ///
    /// Saturating rather than refusing, because the range check already
    /// happened: [`Bounds::check`](super::Bounds) rejects anything above the
    /// declared ceiling, and no declared ceiling can exceed the field it bounds.
    pub fn write(self, caps: &mut Caps, value: u64) {
        let narrowed = u32::try_from(value).unwrap_or(u32::MAX);
        match self {
            Self::MaxModelCalls => caps.max_model_calls = narrowed,
            Self::MaxToolCalls => caps.max_tool_calls = narrowed,
            Self::MaxTokens => caps.max_tokens = value,
            Self::MaxRetries => caps.max_retries = narrowed,
        }
    }
}

impl std::fmt::Display for CapField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One move of one field of a [`LoopProfile`].
///
/// # Wire form
///
/// Internally tagged on `change`, so an event reads as
/// `{"change": "threshold", "field": "stuck", "to": 3}`. The tag and the field
/// names are a wire format: an amendment survives a checkpoint and is rendered
/// into a finished run's report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum Change {
    /// Move a routing threshold.
    Threshold {
        /// Which threshold.
        field: ThresholdField,
        /// Its new value.
        to: u32,
    },
    /// Move a counted limit.
    Cap {
        /// Which limit.
        field: CapField,
        /// Its new value.
        to: u64,
    },
    /// Stop paying for a declared evaluation arm.
    MuteArm {
        /// The arm, which the run's `ArmSet` must already declare.
        arm: String,
    },
    /// Start paying for a muted arm again.
    UnmuteArm {
        /// The arm, which the run's `ArmSet` must already declare.
        arm: String,
    },
}

impl Change {
    /// Applies this change to `profile`.
    ///
    /// Total: every variant lands. Whether it *should* land is
    /// [`Bounds::check`](super::Bounds)'s question, asked before this is
    /// called, so that a refused amendment leaves the profile untouched rather
    /// than half-moved.
    pub fn apply_to(&self, profile: &mut LoopProfile) {
        match self {
            Self::Threshold { field, to } => field.write(&mut profile.thresholds, *to),
            Self::Cap { field, to } => field.write(&mut profile.caps, *to),
            Self::MuteArm { arm } => {
                profile.muted.insert(arm.clone());
            }
            Self::UnmuteArm { arm } => {
                profile.muted.remove(arm);
            }
        }
    }

    /// The arm this change names, if it names one.
    #[must_use]
    pub fn arm(&self) -> Option<&str> {
        match self {
            Self::MuteArm { arm } | Self::UnmuteArm { arm } => Some(arm),
            Self::Threshold { .. } | Self::Cap { .. } => None,
        }
    }
}

impl std::fmt::Display for Change {
    /// One line, as it appears in an event and in a run's report.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Threshold { field, to } => write!(f, "{field} := {to}"),
            Self::Cap { field, to } => write!(f, "{field} := {to}"),
            Self::MuteArm { arm } => write!(f, "mute {arm}"),
            Self::UnmuteArm { arm } => write!(f, "unmute {arm}"),
        }
    }
}

/// One proposed change, with who proposed it, when, and why.
///
/// The `because` is not decoration. A run that quietly retuned itself and then
/// succeeded is indistinguishable in its report from a run that succeeded as
/// configured, so the evidence travels with the change and is rendered beside
/// it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Amendment {
    /// The arm that proposed it. Exactly one arm in a set may.
    pub proposer: String,
    /// The pass it was proposed on. It takes effect on the next one.
    pub pass: u32,
    /// What it moves.
    pub change: Change,
    /// The evidence, in the proposer's words.
    pub because: String,
}

impl Amendment {
    /// A proposal from `proposer` on `pass`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tinyloops::{Amendment, Change, ThresholdField};
    /// let amendment = Amendment::new(
    ///     "tune",
    ///     3,
    ///     Change::Threshold { field: ThresholdField::Stuck, to: 3 },
    ///     "diversifying made the run worse",
    /// );
    /// assert_eq!(amendment.pass, 3);
    /// assert_eq!(amendment.to_string(), "tune @3: stuck := 3 — diversifying made the run worse");
    /// ```
    #[must_use]
    pub fn new(
        proposer: impl Into<String>,
        pass: u32,
        change: Change,
        because: impl Into<String>,
    ) -> Self {
        Self {
            proposer: proposer.into(),
            pass,
            change,
            because: because.into(),
        }
    }
}

impl std::fmt::Display for Amendment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} @{}: {} — {}",
            self.proposer, self.pass, self.change, self.because
        )
    }
}

/// The arms a profile is no longer paying for.
///
/// A type alias rather than a newtype: it is a set of names, the ordering is
/// what makes a profile's wire form stable, and nothing about it needs
/// defending beyond that.
pub type Muted = BTreeSet<String>;

/// What became of a proposed amendment.
///
/// Both outcomes are kept, and that is the point. A run that quietly retuned
/// itself and then succeeded is indistinguishable in its report from a run that
/// succeeded as configured; a tuner proposing forty refused amendments is a
/// broken tuner reporting nothing. Recording only the acceptances would hide
/// the second failure completely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Verdict {
    /// The amendment was folded into the profile.
    Applied,
    /// The amendment was refused, and the profile is untouched.
    Refused {
        /// Why, in the words of the check that refused it.
        reason: String,
    },
}

impl Verdict {
    /// Whether the profile moved.
    #[must_use]
    pub const fn applied(&self) -> bool {
        matches!(self, Self::Applied)
    }
}

/// One amendment and what became of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recorded {
    /// What was proposed.
    pub amendment: Amendment,
    /// What became of it.
    pub verdict: Verdict,
}

impl std::fmt::Display for Recorded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.verdict {
            Verdict::Applied => write!(f, "{}", self.amendment),
            Verdict::Refused { reason } => write!(f, "{} [refused: {reason}]", self.amendment),
        }
    }
}
