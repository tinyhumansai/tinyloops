//! The accumulator one goal run carries from turn to turn, and the signed
//! movement one arm contributes to it.
//!
//! Both types live here rather than in the module root because they are the
//! substance of the module: [`LoopState`] is what crosses the graph, and
//! [`Delta`] is the only thing that is ever merged into it.

use serde::{Deserialize, Serialize};

use crate::policy::Judgement;

/// What one goal run carries from turn to turn.
///
/// This is the loop's accumulator: the engine seeds it from `config.state.init`
/// and folds each pass into it through `config.state.update`, so it is
/// reachable from any expression in the graph as `=nodes.<loop id>.state`. The
/// routing ladder reads nothing else, which is why every field a route depends
/// on is a plain counter rather than a nested structure.
///
/// # The consecutive counters
///
/// [`Self::unproductive`], [`Self::blocked`], [`Self::computational`], and
/// [`Self::unverified`] count *consecutive* passes, not totals. A pass that
/// breaks the streak sets its counter back to zero, which is what makes a
/// threshold like "two unproductive passes in a row" mean what it says: a run
/// that alternates between progress and stalling is making progress, and must
/// not accumulate its way into the diversify branch.
///
/// # Two fields that are not about a model's opinion
///
/// [`Self::established`] is the one field read off the *workspace* rather than
/// off a model reply. Without it, a verdict that resets [`Self::unproductive`]
/// is just a word: a model can keep a run out of the diversify branch
/// indefinitely by asserting progress it did not make, and nothing in the loop
/// would ever contradict it. Counting what actually landed is the check on
/// that.
///
/// [`Self::expired`] is the only field about the clock rather than about the
/// work. Everything else answers "how is this going"; that one answers "is
/// there still time", and a run can be going well and still be out of it.
///
/// # Wire form
///
/// The struct carries `#[serde(default)]` at the container level, which applies
/// to every field: an accumulator written by an older revision that lacks a
/// field still deserializes, taking the field's default. Field names *are* the
/// wire format — the graph's jq addresses them by name — so a rename is a
/// decode error at runtime, not a compile error. `src/state/test.rs` pins the
/// representation for exactly that reason.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LoopState {
    /// What the run is trying to achieve, in the words it was given.
    pub goal: String,
    /// How many passes round the loop have completed.
    pub passes: u32,
    /// How many attempts at the goal have been spent, across every arm.
    pub attempts: u32,
    /// Consecutive passes that did not advance the work.
    pub unproductive: u32,
    /// Consecutive passes whose only outcome was an infrastructure failure.
    ///
    /// A tool that would not start, a sandbox that died, a fetch that never
    /// answered: the run learned nothing about the goal, so these passes are
    /// counted apart from the ones that tried and failed.
    pub blocked: u32,
    /// Consecutive passes whose only gain was a larger instance of prior work.
    ///
    /// More output of the same shape is not progress. Counting it separately is
    /// what lets the loop tell "grinding" apart from "stuck".
    pub computational: u32,
    /// Consecutive passes reaching an answer supported by exactly one route.
    ///
    /// An answer nothing corroborates is a claim. The counter is what makes a
    /// run stop and report rather than bank it.
    pub unverified: u32,
    /// How many times the run has been restarted from a fresh approach.
    pub restarts: u32,
    /// Units of progress read off the workspace, never off a model reply.
    pub established: u32,
    /// Units of progress accepted into the run's result.
    pub banked: u32,
    /// Whether the run reached an answer it is prepared to stand behind.
    pub solved: bool,
    /// Whether the run has spent its wall clock.
    pub expired: bool,
    /// The most recent attempt, as the loop recorded it.
    pub last_attempt: String,
    /// What the run learned, oldest first.
    pub lessons: Vec<String>,
    /// The correction handed to the next pass, empty when there is none.
    pub steer: String,
    /// The score each pass was given, oldest first.
    pub scores: Vec<u8>,
    /// The verdict the most recent pass was given.
    pub judged: Judgement,
}

/// The signed movement one arm contributes to a [`LoopState`].
///
/// A delta carries the *counters* and the two latching flags, and nothing else.
/// That is deliberate: counters have an order-independent merge (add them) and
/// latches have one (a set wins), while the narrative fields — the goal, the
/// last attempt, the lessons, the steer, the scores, the verdict — have none.
/// Two arms writing different text can only be merged by picking one, and
/// picking one is exactly the arrival-order dependence the fold exists to
/// avoid. Those fields therefore keep the base's values, and the loop head
/// stays their sole writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Delta {
    /// Movement in [`LoopState::passes`].
    pub passes: i64,
    /// Movement in [`LoopState::attempts`].
    pub attempts: i64,
    /// Movement in [`LoopState::unproductive`].
    pub unproductive: i64,
    /// Movement in [`LoopState::blocked`].
    pub blocked: i64,
    /// Movement in [`LoopState::computational`].
    pub computational: i64,
    /// Movement in [`LoopState::unverified`].
    pub unverified: i64,
    /// Movement in [`LoopState::restarts`].
    pub restarts: i64,
    /// Movement in [`LoopState::established`].
    pub established: i64,
    /// Movement in [`LoopState::banked`].
    pub banked: i64,
    /// A vote on [`LoopState::solved`], or `None` for "no opinion".
    ///
    /// Resolved by [`LoopState::apply`] with `Some(true)` winning, so an arm
    /// that reached an answer is never undone by a concurrent arm that did not.
    pub solved: Option<bool>,
    /// A vote on [`LoopState::expired`], resolved the same way as
    /// [`Self::solved`].
    pub expired: Option<bool>,
}
