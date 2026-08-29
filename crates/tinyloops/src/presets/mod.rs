//! Assembled loops, ready to run, and the threshold sets they route on.
//!
//! Everything else in this crate is a part. This module is the parts already
//! bolted together: [`research_loop`] hands back an [`AssembledLoop`] with an
//! orchestrator, a closed step set, two evaluation arms, a budget, and a
//! preset, and that loop both emits a graph and drives itself.
//!
//! # The two anti-confabulation rules the preset exists to carry
//!
//! A framework that shipped only traits would leave every consumer to make
//! these two decisions again, and they are the two most consequential ones.
//!
//! **A `solved` verdict needs the marker, an artifact, and internal
//! consistency.** Any one alone is a claim; the conjunction is evidence. See
//! [`Reflect`].
//!
//! **An unreadable verdict is the cheap outcome, never the expensive one.** A
//! judge that cannot read the report returns [`Proceed`](crate::Judgement),
//! because reading a serialization slip as a restart throws away a run's work.
//! See [`Judge`].
//!
//! # What a preset is for
//!
//! [`Preset`] is a named set of thresholds with its bet written down. The bet
//! is the interesting part: `stuck` is an estimator of the point where
//! sequential revision stops beating parallel sampling, and where that point
//! sits depends entirely on how accurate a domain's feedback is. A number with
//! no rationale beside it is a number nobody can argue with, revise, or tune,
//! and [`Preset::ALL`] is the list the exhaustive parity sweep reads, so a
//! preset cannot be added without being proved.

mod arms;
mod assembled;
mod steps;
mod types;

pub use arms::{Judge, Reflect, SOLVED_MARKER};
pub use assembled::{AssembledLoop, Driven, research_loop};
pub use steps::{Advance, ArmStep, Converge, Gather};
pub use types::Preset;

#[cfg(test)]
mod test;
