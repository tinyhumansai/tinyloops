//! The configuration one run operates under, carried in its own accumulator.
//!
//! A [`LoopProfile`] holds the numbers the routing ladder reads. It lives in
//! the loop's accumulator rather than in the emitted graph, and that placement
//! is the whole point of the type.
//!
//! # Why the thresholds are state and not topology
//!
//! The ladder is jq the graph runs, so its thresholds have to reach it somehow.
//! Rendering them into the program — the original design — makes the emitted
//! graph a function of the thresholds, and
//! [`GraphSignature`](crate::GraphSignature) hashes each node's config whole.
//! A threshold change was therefore a *topology* change, and a resume across
//! one was refused. That is correct for a run whose configuration is fixed
//! before it starts and fatal to one that revises it: a run that retuned itself
//! at pass three recorded a signature describing a graph that no longer exists.
//!
//! Addressing them out of the accumulator keeps the half of the old rule that
//! was load-bearing — one source, never a literal typed into graph JSON — and
//! drops the half that was not. One graph now serves every preset and every
//! revision of every preset. See
//! `docs/adr/0006-thresholds-addressed-from-run-state.md`.
//!
//! # What a run may change about it
//!
//! Nothing, until a [`Tuner`](crate::Tuner) is wired in. When one is, it
//! proposes an [`Amendment`], the head folds it at the start of the next pass,
//! and [`LoopProfile::revision`] and [`LoopProfile::history`] record that it
//! did. What it may propose is [`Bounds`], which the preset owns.

use serde::{Deserialize, Serialize};

use super::{Amendment, Muted, Thresholds};
use crate::budget::Caps;
use crate::presets::Preset;

/// The configuration a run is operating under.
///
/// Seeded from a [`Preset`] at construction and carried in
/// [`LoopState::profile`](crate::LoopState::profile), where the routing ladder
/// and [`route`](crate::route) both read it from the same address.
///
/// # Wire form
///
/// `#[serde(default)]` at the container level, so an accumulator written by a
/// revision that lacked a field still deserializes and takes that field's
/// default. Field names *are* the wire format — the graph's jq addresses
/// `.profile.thresholds.<field>` by name — so a rename is a decode error at
/// run time rather than a compile error. `src/policy/test.rs` pins the
/// representation for exactly that reason.
///
/// # Examples
///
/// ```
/// # use tinyloops::{LoopProfile, Preset};
/// let profile = LoopProfile::of(Preset::Persistent);
/// assert_eq!(profile.revision, 0);
/// assert_eq!(profile.thresholds.stuck, 4);
/// assert_eq!(profile.origin, Preset::Persistent);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LoopProfile {
    /// How many times this profile has been revised.
    ///
    /// Zero for a profile as constructed, and bumped by exactly one every time
    /// an amendment is folded.
    pub revision: u32,
    /// The counter bounds the routing ladder reads.
    pub thresholds: Thresholds,
    /// The preset this profile started from.
    ///
    /// Kept so a finished run can say which bet it was making, and so a report
    /// naming "the persistent preset" is reading a value rather than repeating
    /// what a caller told it.
    pub origin: Preset,
    /// The counted limits an amendment may lower.
    ///
    /// A copy of the run's caps, carried here so a `Cap` amendment has
    /// somewhere to land that survives a checkpoint. The [`RunBudget`] the
    /// driver meters against is built from it, so lowering one here lowers what
    /// the run may spend.
    ///
    /// [`RunBudget`]: crate::RunBudget
    pub caps: Caps,
    /// The declared arms this run has stopped paying for.
    ///
    /// A muted arm's node still runs and still converges — it returns
    /// unchanged. Removing its edge would leave the merge barrier waiting on an
    /// arm nothing will activate, which is a hung pass rather than a saved one.
    pub muted: Muted,
    /// Every amendment folded into this profile, oldest first.
    ///
    /// The run's own account of what it changed about itself and why. Rendered
    /// into the report, and the data a cross-run layer would score — this crate
    /// scores nothing.
    pub history: Vec<Amendment>,
}

impl LoopProfile {
    /// The profile a run starts on when it takes `preset`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tinyloops::{LoopProfile, Preset, Thresholds};
    /// assert_eq!(LoopProfile::of(Preset::Balanced).thresholds, Thresholds::default());
    /// ```
    #[must_use]
    pub fn of(preset: Preset) -> Self {
        Self {
            revision: 0,
            thresholds: preset.thresholds(),
            origin: preset,
            caps: Caps::default(),
            muted: Muted::new(),
            history: Vec::new(),
        }
    }

    /// Whether `arm` is one this run has stopped paying for.
    #[must_use]
    pub fn is_muted(&self, arm: &str) -> bool {
        self.muted.contains(arm)
    }

    /// Folds `amendment` in, bumping the revision and recording it.
    ///
    /// Assumed checked: [`Bounds::check`](super::Bounds::check) and the run's
    /// amendment budget are the caller's to consult, and the caller is the
    /// `pass` step, which is the loop's single exit. Applying here rather than
    /// where the amendment was proposed is what makes "it takes effect on the
    /// *next* pass" a property of the code's position rather than a rule
    /// someone remembers.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tinyloops::{Amendment, Change, LoopProfile, Preset, ThresholdField};
    /// let mut profile = LoopProfile::of(Preset::Balanced);
    /// profile.fold(Amendment::new(
    ///     "tune",
    ///     2,
    ///     Change::Threshold { field: ThresholdField::Stuck, to: 3 },
    ///     "diversifying made the run worse",
    /// ));
    ///
    /// assert_eq!(profile.thresholds.stuck, 3);
    /// assert_eq!(profile.revision, 1);
    /// assert_eq!(profile.history.len(), 1);
    /// ```
    pub fn fold(&mut self, amendment: Amendment) {
        amendment.change.clone().apply_to(self);
        self.revision = self.revision.saturating_add(1);
        self.history.push(amendment);
    }
}
