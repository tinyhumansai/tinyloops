//! The room a run has to revise itself, and the check that keeps it there.
//!
//! A tuner without bounds has one strategy available for every difficulty,
//! which is to raise whichever threshold is complaining. A run that can raise
//! `max_attempts` has no attempt ceiling; a run that can raise `stuck` never
//! diversifies; a run that can raise a cap has no budget. Each of those runs
//! completes, reports plausibly, and cost more than the run that was configured
//! correctly.
//!
//! [`Bounds`] is what makes that unavailable rather than discouraged. It is
//! separate from the proposer on purpose: a rule-based tuner and a model-based
//! one are bounded by the same value, so swapping one for the other cannot
//! widen what a run may do to itself.
//!
//! # Refused, never clamped
//!
//! [`Bounds::check`] returns an error rather than a nearest legal value. A
//! clamped proposal reads as accepted at the proposer and as a no-op in the
//! state, and nothing joins the two — so a tuner proposing the same impossible
//! change forty times looks, from every angle, like a tuner that is working.
//! The refusal is the signal.
//!
//! # The preset owns them; a deployment may narrow them
//!
//! [`Bounds`] ships with the preset, because the room a run has to revise
//! itself is part of the methodological bet the preset already states: choosing
//! a preset is choosing the bet *and* the room. [`Bounds::narrow`] lets a
//! deployment tighten one it distrusts, field by field, and cannot loosen one —
//! the same shape [`RunBudget::narrow`](crate::RunBudget::narrow) already has
//! for caps.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::amendment::{CapField, Change, ThresholdField};
use crate::{Error, Result};

/// How many amendments a run may fold when nothing says otherwise.
///
/// Four is a working budget and not a limit anyone should reach: a run that
/// wants a fifth revision of its own configuration is a run whose preset was
/// wrong, and the right repair is choosing a different preset rather than
/// arriving at one four moves at a time.
pub const DEFAULT_MAX_AMENDMENTS: u32 = 4;

/// How many consecutive silent passes an arm gets before it may be muted.
pub const DEFAULT_MUTING_WINDOW: u32 = 3;

/// An inclusive range one field may be moved within.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    /// The lowest value an amendment may set.
    pub low: u64,
    /// The highest value an amendment may set.
    pub high: u64,
}

impl Range {
    /// A range from `low` to `high`, inclusive.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tinyloops::Range;
    /// assert!(Range::new(1, 4).holds(4));
    /// assert!(!Range::new(1, 4).holds(5));
    /// ```
    #[must_use]
    pub const fn new(low: u64, high: u64) -> Self {
        Self { low, high }
    }

    /// Whether `value` is inside the range.
    #[must_use]
    pub const fn holds(self, value: u64) -> bool {
        value >= self.low && value <= self.high
    }

    /// The tighter of two ranges.
    ///
    /// Raises the floor and lowers the ceiling, so narrowing can only ever
    /// remove room. An inverted result — a floor above its ceiling — is a field
    /// nothing may move, which is the honest reading of two bounds that do not
    /// overlap.
    #[must_use]
    pub fn narrow(self, other: Self) -> Self {
        Self {
            low: self.low.max(other.low),
            high: self.high.min(other.high),
        }
    }
}

/// The room a run has to revise its own profile.
///
/// A field with no entry cannot be moved at all. That is the safe default and
/// the deliberate one: a `Bounds` written without thinking about a field is a
/// `Bounds` that does not let a tuner touch it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Bounds {
    /// The thresholds an amendment may move, and how far.
    pub thresholds: BTreeMap<ThresholdField, Range>,
    /// The caps an amendment may move, and how far.
    pub caps: BTreeMap<CapField, Range>,
    /// The arms an amendment may mute or unmute.
    pub mutable_arms: super::Muted,
    /// Consecutive silent passes before an arm may be muted.
    pub muting_window: u32,
    /// How many amendments the whole run may fold.
    pub max_amendments: u32,
}

impl Bounds {
    /// Bounds that permit nothing.
    ///
    /// The starting point for building one, and the right answer for a preset
    /// that does not want to be tuned at all.
    #[must_use]
    pub fn none() -> Self {
        Self {
            muting_window: DEFAULT_MUTING_WINDOW,
            max_amendments: 0,
            ..Self::default()
        }
    }

    /// Lets an amendment move `field` within `range`.
    #[must_use]
    pub fn threshold(mut self, field: ThresholdField, range: Range) -> Self {
        self.thresholds.insert(field, range);
        self
    }

    /// Lets an amendment move `field` within `range`.
    #[must_use]
    pub fn cap(mut self, field: CapField, range: Range) -> Self {
        self.caps.insert(field, range);
        self
    }

    /// Lets an amendment mute and unmute `arm`.
    #[must_use]
    pub fn mutable(mut self, arm: impl Into<String>) -> Self {
        self.mutable_arms.insert(arm.into());
        self
    }

    /// Sets how many amendments the run may fold.
    #[must_use]
    pub const fn amendments(mut self, max: u32) -> Self {
        self.max_amendments = max;
        self
    }

    /// Sets how many silent passes an arm gets before it may be muted.
    #[must_use]
    pub const fn window(mut self, passes: u32) -> Self {
        self.muting_window = passes;
        self
    }

    /// Whether `change` is inside these bounds.
    ///
    /// # Errors
    ///
    /// - [`Error::UnboundedAmendment`] when the field or arm has no entry at
    ///   all, which is how a `Bounds` that never mentioned a field refuses it.
    /// - [`Error::AmendmentOutOfBounds`] when the field has a range and the
    ///   proposed value sits outside it.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tinyloops::{Bounds, Change, Range, ThresholdField};
    /// let bounds = Bounds::none().threshold(ThresholdField::Stuck, Range::new(1, 4));
    ///
    /// assert!(bounds.check(&Change::Threshold { field: ThresholdField::Stuck, to: 4 }).is_ok());
    /// assert!(bounds.check(&Change::Threshold { field: ThresholdField::Stuck, to: 5 }).is_err());
    /// ```
    pub fn check(&self, change: &Change) -> Result<()> {
        match change {
            Change::Threshold { field, to } => {
                self.within(field.as_str(), self.thresholds.get(field), u64::from(*to))
            }
            Change::Cap { field, to } => self.within(field.as_str(), self.caps.get(field), *to),
            Change::MuteArm { arm } | Change::UnmuteArm { arm } => {
                if self.mutable_arms.contains(arm) {
                    Ok(())
                } else {
                    Err(Error::UnboundedAmendment {
                        field: arm.clone(),
                    })
                }
            }
        }
    }

    /// The shared helper behind the two numeric arms of [`Self::check`].
    fn within(&self, field: &str, range: Option<&Range>, value: u64) -> Result<()> {
        let Some(range) = range else {
            return Err(Error::UnboundedAmendment {
                field: field.to_owned(),
            });
        };
        if range.holds(value) {
            Ok(())
        } else {
            Err(Error::AmendmentOutOfBounds {
                field: field.to_owned(),
                value,
                low: range.low,
                high: range.high,
            })
        }
    }

    /// The tighter of two bounds.
    ///
    /// Every field narrows: a range present in both is intersected, a range
    /// present in only one is dropped, the mutable arms are intersected, and
    /// both counts take the lower value. So a deployment can restrict a preset
    /// it distrusts and cannot widen one, whatever it passes.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tinyloops::{Bounds, Range, ThresholdField};
    /// let preset = Bounds::none()
    ///     .threshold(ThresholdField::Stuck, Range::new(1, 6))
    ///     .amendments(4);
    /// let deployment = Bounds::none()
    ///     .threshold(ThresholdField::Stuck, Range::new(2, 3))
    ///     .amendments(9);
    ///
    /// let narrowed = preset.narrow(&deployment);
    /// assert_eq!(narrowed.thresholds[&ThresholdField::Stuck], Range::new(2, 3));
    /// assert_eq!(narrowed.max_amendments, 4);
    /// ```
    #[must_use]
    pub fn narrow(&self, other: &Self) -> Self {
        let thresholds = self
            .thresholds
            .iter()
            .filter_map(|(field, range)| {
                other
                    .thresholds
                    .get(field)
                    .map(|theirs| (*field, range.narrow(*theirs)))
            })
            .collect();
        let caps = self
            .caps
            .iter()
            .filter_map(|(field, range)| {
                other
                    .caps
                    .get(field)
                    .map(|theirs| (*field, range.narrow(*theirs)))
            })
            .collect();
        let mutable_arms = self
            .mutable_arms
            .intersection(&other.mutable_arms)
            .cloned()
            .collect();

        Self {
            thresholds,
            caps,
            mutable_arms,
            muting_window: self.muting_window.max(other.muting_window),
            max_amendments: self.max_amendments.min(other.max_amendments),
        }
    }
}
