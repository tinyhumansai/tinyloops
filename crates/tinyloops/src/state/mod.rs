//! The loop's accumulator and the order-independent fold that merges it.
//!
//! One goal run carries a single [`LoopState`]. Each pass round the loop
//! produces a new one, and the engine writes it back into the loop head's slot
//! as `=nodes.<loop id>.state`. Everything the routing ladder decides on is a
//! field of that struct.
//!
//! # Why a delta fold and not a merge
//!
//! A superstep can activate more than one arm, and each arm computes its result
//! from the *same* base accumulator. One arm may conclude the pass was
//! productive and zero [`LoopState::unproductive`]; another may record a
//! restart and increment it. Both are correct, and both must land.
//!
//! A "last writer wins" merge silently loses one of them, and which one it
//! loses depends on the order the engine happened to deliver the updates —
//! which this class of engine explicitly does not promise. So an arm does not
//! hand back a state; it hands back the *movement* it caused, as a [`Delta`]
//! computed against the shared base with [`LoopState::delta_from`], and
//! [`LoopState::apply`] sums every arm's movement onto that base at once. The
//! reset and the increment compose instead of racing.
//!
//! # The two properties this buys, and why they are tested
//!
//! **Commutative.** Applying the same deltas in any order gives the same state.
//! Channel update ordering is arbitrary, so a fold that depended on arrival
//! order would produce a different run on a different day and no test, log, or
//! error would report it.
//!
//! **Idempotent under replay.** The engine's fold is at-least-once: an
//! activation replayed after a resume applies its update a second time. That is
//! survivable only because a pass's update is expressed as an *assignment* —
//! the resulting state — rather than as `+= 1`. Re-applying the same pass's
//! result to a state that already holds it is a no-op, because the movement
//! from that state to itself is zero.
//!
//! # Arithmetic
//!
//! Every counter is a `u32` and every merge saturates. A counter must not wrap
//! — a wrapped `unproductive` would read as a fresh, healthy run — and it must
//! not panic, because this code runs inside a node the engine is not able to
//! unwind sensibly.

mod types;

pub use types::{Delta, LoopState};

/// Sums one field across every delta, saturating rather than overflowing.
fn total(deltas: &[Delta], pick: impl Fn(&Delta) -> i64) -> i64 {
    deltas
        .iter()
        .fold(0_i64, |acc, delta| acc.saturating_add(pick(delta)))
}

/// Moves `base` by `movement`, clamped to the range of a counter.
///
/// Below zero is zero: a counter is a count, and a reset that overshoots is
/// still a reset. Above `u32::MAX` is `u32::MAX`, which is a run that has
/// long since hit every threshold it has.
fn shift(base: u32, movement: i64) -> u32 {
    let moved = i64::from(base)
        .saturating_add(movement)
        .clamp(0, i64::from(u32::MAX));
    u32::try_from(moved).unwrap_or(u32::MAX)
}

/// Resolves the votes on a latching flag.
///
/// A `Some(true)` anywhere wins, then a `Some(false)` anywhere, then the base.
/// Order-independent by construction, which is the whole requirement.
fn latch(base: bool, deltas: &[Delta], pick: impl Fn(&Delta) -> Option<bool>) -> bool {
    if deltas.iter().any(|delta| pick(delta) == Some(true)) {
        true
    } else if deltas.iter().any(|delta| pick(delta) == Some(false)) {
        false
    } else {
        base
    }
}

impl LoopState {
    /// Starts a run on `goal`, with every counter at zero.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tinyloops::LoopState;
    /// let state = LoopState::new("ship the release");
    /// assert_eq!(state.goal, "ship the release");
    /// assert_eq!(state.passes, 0);
    /// ```
    #[must_use]
    pub fn new(goal: impl Into<String>) -> Self {
        Self {
            goal: goal.into(),
            ..Self::default()
        }
    }

    /// Returns the movement from `base` to `self`.
    ///
    /// Only the counters and the two latching flags move; see [`Delta`] for why
    /// the narrative fields do not.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tinyloops::LoopState;
    /// let base = LoopState::new("goal");
    /// let mut next = base.clone();
    /// next.passes = 1;
    ///
    /// assert_eq!(next.delta_from(&base).passes, 1);
    /// assert_eq!(base.delta_from(&next).passes, -1);
    /// ```
    #[must_use]
    pub fn delta_from(&self, base: &Self) -> Delta {
        let moved = |now: u32, was: u32| i64::from(now) - i64::from(was);
        let voted = |now: bool, was: bool| (now != was).then_some(now);

        Delta {
            passes: moved(self.passes, base.passes),
            attempts: moved(self.attempts, base.attempts),
            unproductive: moved(self.unproductive, base.unproductive),
            blocked: moved(self.blocked, base.blocked),
            computational: moved(self.computational, base.computational),
            unverified: moved(self.unverified, base.unverified),
            restarts: moved(self.restarts, base.restarts),
            established: moved(self.established, base.established),
            banked: moved(self.banked, base.banked),
            solved: voted(self.solved, base.solved),
            expired: voted(self.expired, base.expired),
        }
    }

    /// Sums every delta in `deltas` onto `self`.
    ///
    /// This is the fold. Each arm's [`Delta`] is computed against the same base
    /// — `self` — so a reset from one arm and an increment from another land
    /// together instead of overwriting one another, and the result does not
    /// depend on the order the deltas arrived in.
    ///
    /// Counters saturate at `0` and `u32::MAX`. The narrative fields are
    /// carried through from `self` untouched.
    ///
    /// # Examples
    ///
    /// A productive arm and a restart arm, computed from one base, both land:
    ///
    /// ```
    /// # use tinyloops::LoopState;
    /// let mut base = LoopState::new("goal");
    /// base.unproductive = 1;
    ///
    /// let mut productive = base.clone();
    /// productive.unproductive = 0;
    ///
    /// let mut restarted = base.clone();
    /// restarted.unproductive = 2;
    /// restarted.restarts = 1;
    ///
    /// let deltas = [productive.delta_from(&base), restarted.delta_from(&base)];
    /// let merged = base.apply(&deltas);
    ///
    /// assert_eq!(merged.unproductive, 1); // -1 and +1, both applied
    /// assert_eq!(merged.restarts, 1);
    /// ```
    #[must_use]
    pub fn apply(&self, deltas: &[Delta]) -> Self {
        Self {
            passes: shift(self.passes, total(deltas, |d| d.passes)),
            attempts: shift(self.attempts, total(deltas, |d| d.attempts)),
            unproductive: shift(self.unproductive, total(deltas, |d| d.unproductive)),
            blocked: shift(self.blocked, total(deltas, |d| d.blocked)),
            computational: shift(self.computational, total(deltas, |d| d.computational)),
            unverified: shift(self.unverified, total(deltas, |d| d.unverified)),
            restarts: shift(self.restarts, total(deltas, |d| d.restarts)),
            established: shift(self.established, total(deltas, |d| d.established)),
            banked: shift(self.banked, total(deltas, |d| d.banked)),
            solved: latch(self.solved, deltas, |d| d.solved),
            expired: latch(self.expired, deltas, |d| d.expired),
            goal: self.goal.clone(),
            last_attempt: self.last_attempt.clone(),
            lessons: self.lessons.clone(),
            steer: self.steer.clone(),
            scores: self.scores.clone(),
            judged: self.judged,
        }
    }
}

#[cfg(test)]
mod test;
