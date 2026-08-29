//! Thresholds, verdicts, and the ladder that turns an accumulator into a route.
//!
//! A pass ends with a [`LoopState`]. This module answers the only question that
//! follows: what happens next. It answers it twice, in two languages — once in
//! Rust, with [`route`], and once in jq, with [`ladder`], because the loop's
//! branch is taken inside the graph and the graph evaluates jq.
//!
//! Two implementations of one decision is a liability unless something holds
//! them together. Three things do:
//!
//! 1. Both read the same [`Thresholds`] struct; [`ladder`] interpolates the
//!    fields rather than spelling the numbers out, so the Rust and the jq
//!    cannot disagree about a constant.
//! 2. Both speak the same vocabulary: the jq emits exactly [`Route::as_str`].
//! 3. `src/policy/test.rs` sweeps every combination of the counters that either
//!    side reads and asserts they agree on all of them.
//!
//! # The layers
//!
//! - `types.rs` — [`Thresholds`], [`Route`], [`Judgement`], [`Autonomy`], and
//!   [`Outcome`]: the numbers and the closed vocabularies.
//! - `mod.rs` — [`route`] and [`is_terminal`]: the decision itself.
//! - `ladder.rs` — [`ladder`] and [`terminal_condition`]: the same decision,
//!   emitted as the jq the graph runs.

mod ladder;
mod types;

pub use ladder::{
    evaluate_ladder, evaluate_terminal_condition, ladder, loop_id_scope, terminal_condition,
};
pub use types::{Autonomy, Judgement, Outcome, Route, Thresholds};

use crate::state::LoopState;

/// Chooses the route a run takes after a pass.
///
/// The arms are tested in this order, and the order is the policy — several
/// conditions are true at once in a real run, and which one is answered first
/// decides what the loop does:
///
/// 1. **[`Route::Blocked`]** — `blocked >= thresholds.blocked`. Tested first
///    because infrastructure failure is not the work. A run whose sandbox will
///    not start has learned nothing about its goal, and every other arm below
///    would misread that silence as evidence: as a lack of progress, as an
///    unverified answer, as attempts worth spending. Answer it first and the
///    run reports the real problem.
/// 2. **[`Route::Solved`]** — `solved`, or `attempts >= thresholds.max_attempts`.
///    Above the remaining arms because a finished run is finished: there is
///    nothing to diversify towards, and no point noticing it is unverified
///    when the budget is gone either way.
/// 3. **[`Route::Reported`]** — `unverified >= thresholds.unverified`. Above
///    both diversify triggers deliberately. A run that keeps arriving at the
///    same single-route answer *is* unproductive and *is* computational, so
///    the arms below would fire on it and send it round again to reach that
///    answer a third time. It does not need another approach; it needs a human
///    to look at the one it has.
/// 4. **[`Route::Diversify`]** — `unproductive >= thresholds.stuck`, or
///    `computational >= thresholds.computational`. Sequential revision has
///    stopped paying; sample a different approach instead of refining this one.
/// 5. **[`Route::Retry`]** — everything else. Go round again.
///
/// # Examples
///
/// ```
/// # use tinyloops::{LoopState, Route, Thresholds, route};
/// let thresholds = Thresholds::default();
/// let mut state = LoopState::new("goal");
///
/// assert_eq!(route(&state, &thresholds), Route::Retry);
///
/// state.unproductive = 2;
/// assert_eq!(route(&state, &thresholds), Route::Diversify);
///
/// // Blocked outranks everything below it.
/// state.blocked = 2;
/// assert_eq!(route(&state, &thresholds), Route::Blocked);
/// ```
#[must_use]
pub fn route(state: &LoopState, thresholds: &Thresholds) -> Route {
    if state.blocked >= thresholds.blocked {
        Route::Blocked
    } else if state.solved || state.attempts >= thresholds.max_attempts {
        Route::Solved
    } else if state.unverified >= thresholds.unverified {
        Route::Reported
    } else if state.unproductive >= thresholds.stuck
        || state.computational >= thresholds.computational
    {
        Route::Diversify
    } else {
        Route::Retry
    }
}

/// Whether the loop head should stop.
///
/// This is [`route`] reaching a terminal arm, plus the two budget conditions
/// the ladder does not read: the wall clock, and the restart allowance. Neither
/// is a *route* — there is no branch to take on expiry, the run simply ends —
/// so they live here rather than as arms above.
///
/// [`terminal_condition`] is the jq spelling of exactly this function.
///
/// # Examples
///
/// ```
/// # use tinyloops::{LoopState, Thresholds, is_terminal};
/// let thresholds = Thresholds::default();
/// let mut state = LoopState::new("goal");
/// assert!(!is_terminal(&state, &thresholds));
///
/// state.expired = true;
/// assert!(is_terminal(&state, &thresholds));
/// ```
#[must_use]
pub fn is_terminal(state: &LoopState, thresholds: &Thresholds) -> bool {
    state.expired
        || state.restarts >= thresholds.max_restarts
        || route(state, thresholds).is_terminal()
}

#[cfg(test)]
mod test;
