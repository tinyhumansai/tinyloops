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
//! # Why it is `Copy`
//!
//! Everything here is a handful of `u32`s and a unit enum, and the accumulator
//! is cloned on every fold. A profile that allocated would allocate once per
//! pass per arm for no gain.

use serde::{Deserialize, Serialize};

use super::Thresholds;
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LoopProfile {
    /// How many times this profile has been revised.
    ///
    /// Zero for a profile as constructed. Nothing in this crate moves it yet;
    /// it is here so a checkpoint written today deserializes unchanged once
    /// amendments land, rather than gaining a field and a new wire form at the
    /// same time.
    pub revision: u32,
    /// The counter bounds the routing ladder reads.
    pub thresholds: Thresholds,
    /// The preset this profile started from.
    ///
    /// Kept so a finished run can say which bet it was making, and so a report
    /// naming "the persistent preset" is reading a value rather than repeating
    /// what a caller told it.
    pub origin: Preset,
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
        }
    }
}
