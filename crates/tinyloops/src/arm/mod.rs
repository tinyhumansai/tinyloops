//! The evaluation arms, the one list both their edge sets come from, and the
//! merge that folds them.
//!
//! A pass fans out from `attempt` to every arm, waits for all of them at a
//! merge barrier, and folds what they returned into one state the routing
//! ladder then reads. This module owns all three: the declaration
//! ([`ArmSet`]), the two derived edge sets ([`ArmSet::fan_out`] and
//! [`ArmSet::converge`]), and the fold ([`ArmSet::merge`]).
//!
//! # One list, two edge sets
//!
//! Invariant 6 of `docs/specs/loop-kernel.md`: the arm list is declared once,
//! and the builder derives *both* the fan-out edges and the convergence edges
//! from it. "Every arm converges" and "every arm is folded" have to be the same
//! fact. As two facts they can drift, and the drift is silent — an arm added to
//! the fan-out but not to the fold runs, costs its budget, and changes nothing,
//! and no test, log, or error reports it. There is no constructor here that
//! accepts the two lists separately, which is what makes the drift
//! unrepresentable rather than merely unlikely.
//!
//! # Arms read the previous step, never the accumulator
//!
//! Invariant 3. [`upstream_address`] is the helper that builds an arm's input
//! address, and it is the only address an arm should be wired to. Reading
//! `=.nodes.<loop>.state` from an arm is the bug it exists to prevent: the head
//! folds at the *top* of a pass, so mid-body the accumulator holds the state as
//! of the *previous* pass and the arm routes on a stale answer.
//!
//! That bug is invisible to the obvious test. A mock attempt returning a
//! constant produces the same report on every pass, so "one pass behind" and
//! "current" are indistinguishable and both wirings pass. Only a mock whose
//! answer varies per call can fail on it.
//!
//! # The fold, and its law
//!
//! Every arm is handed the same base and returns a whole state, so the merge
//! can take each arm's movement — [`LoopState::delta_from`] — and sum every
//! movement onto the base at once. A reset and an increment in the same
//! superstep therefore compose rather than race.
//!
//! The narrative an arm produces merges under a different law: exclusive
//! ownership, arbitrated by [`LoopState::merge`], which refuses a field two
//! arms both wrote rather than picking a winner.
//!
//! Both laws are commutative and associative, and that is a stated law rather
//! than an observation. Arms complete in whatever order their work takes, and
//! `tinyflows` folds channel updates in deterministic *active-set* order —
//! deterministic means reproducible, not order-independent. The active set
//! changes whenever an arm is added, removed, or renamed, so a fold that
//! depended on arrival order would answer differently after an unrelated edit
//! with nothing to report it. [`ArmSet::merge`] additionally sorts by arm name
//! before folding, which is a belt to those braces: it makes a contested-field
//! report name the same two arms every time, and it neither replaces the law
//! nor excuses the permutation tests in `test.rs`.

mod types;

pub use types::{Arm, ArmOutcome, ArmSet, Edge};

use crate::state::LoopState;
use crate::{Error, Result};

/// The expression address of `node`'s output.
///
/// This is what an arm reads: the node immediately upstream of it. Wiring an
/// arm to `=.nodes.<loop>.state` instead is invariant 3's bug — the head folds
/// at the top of a pass, so the accumulator is one pass behind anywhere inside
/// the body — and this function exists so that address is never typed by hand
/// at an arm's input.
///
/// # Examples
///
/// ```
/// # use tinyloops::upstream_address;
/// assert_eq!(upstream_address("attempt"), "=.nodes.attempt.output");
/// ```
#[must_use]
pub fn upstream_address(node: &str) -> String {
    format!("=.nodes.{node}.output")
}

impl ArmSet {
    /// The edges from `from` to every arm.
    ///
    /// One half of invariant 6, derived from the declared list. Its counterpart
    /// is [`Self::converge`], derived from the same one.
    #[must_use]
    pub fn fan_out(&self, from: &str) -> Vec<Edge> {
        self.names()
            .into_iter()
            .map(|arm| Edge::new(from, arm))
            .collect()
    }

    /// The edges from every arm into `to`, the merge barrier.
    ///
    /// The other half of invariant 6, derived from the same list as
    /// [`Self::fan_out`].
    #[must_use]
    pub fn converge(&self, to: &str) -> Vec<Edge> {
        self.names()
            .into_iter()
            .map(|arm| Edge::new(arm, to))
            .collect()
    }

    /// The addresses the merge's fold expression reads, one per arm.
    ///
    /// Derived from the declared list, so the fold reads exactly the arms the
    /// fan-out started. Each is an arm node's *output* — see
    /// [`upstream_address`].
    #[must_use]
    pub fn fold_inputs(&self) -> Vec<String> {
        self.names()
            .into_iter()
            .map(|arm| upstream_address(arm))
            .collect()
    }

    /// Folds every arm's outcome onto the base state.
    ///
    /// The barrier's reducer. Counters fold by delta against the shared `base`,
    /// so a reset and an increment in the same superstep compose; narrative
    /// fields fold by exclusive ownership, so two arms writing the same one is
    /// refused rather than resolved.
    ///
    /// Outcomes are sorted by arm name first. The fold is commutative, so this
    /// changes no answer — it only makes a contested-field report deterministic.
    ///
    /// # Errors
    ///
    /// - [`Error::UnknownArm`] when an outcome names an arm this set does not
    ///   declare. Folding it would credit the run with evidence no declared arm
    ///   produced.
    /// - [`Error::ArmNotFolded`] when a declared arm produced no outcome. This
    ///   is invariant 6 checked at the barrier as well as at the edges: the
    ///   whole point of one list is that an arm cannot run and go unfolded.
    /// - [`Error::ContestedField`] when two arms wrote the same narrative
    ///   field.
    ///
    /// # Examples
    ///
    /// A reset arm and an increment arm, computed from one base, both land:
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use serde_json::Value;
    /// # use tinyloops::{
    /// #     Arm, ArmOutcome, ArmSet, LoopState, NoWrite, Result, StepContext,
    /// # };
    /// # struct Named(&'static str);
    /// # impl Arm for Named {
    /// #     fn name(&self) -> &'static str { self.0 }
    /// #     fn evaluate(
    /// #         &self,
    /// #         base: &LoopState,
    /// #         _report: &Value,
    /// #         _ctx: StepContext<'_, NoWrite>,
    /// #     ) -> Result<ArmOutcome> {
    /// #         Ok(ArmOutcome::unchanged(self.name(), base))
    /// #     }
    /// # }
    /// let set = ArmSet::new(vec![
    ///     Arc::new(Named("reflect")) as Arc<dyn Arm>,
    ///     Arc::new(Named("judge")),
    /// ])?;
    ///
    /// let mut base = LoopState::new("goal");
    /// base.unproductive = 3;
    ///
    /// let mut reflect = ArmOutcome::unchanged("reflect", &base);
    /// reflect.state.unproductive = 0; // a reset: −3
    /// let mut judge = ArmOutcome::unchanged("judge", &base);
    /// judge.state.unproductive = 4; // an increment: +1
    ///
    /// let merged = set.merge(&base, vec![reflect, judge])?;
    /// assert_eq!(merged.unproductive, 1);
    /// # Ok::<(), tinyloops::Error>(())
    /// ```
    pub fn merge(&self, base: &LoopState, outcomes: Vec<ArmOutcome>) -> Result<LoopState> {
        let declared = self.names();

        for outcome in &outcomes {
            if !declared.contains(&outcome.arm()) {
                return Err(Error::UnknownArm {
                    name: outcome.arm(),
                });
            }
        }

        for arm in declared {
            if !outcomes.iter().any(|outcome| outcome.arm() == arm) {
                return Err(Error::ArmNotFolded { name: arm });
            }
        }

        let mut outcomes = outcomes;
        outcomes.sort_by_key(ArmOutcome::arm);

        let deltas: Vec<_> = outcomes
            .iter()
            .map(|outcome| outcome.state.delta_from(base))
            .collect();
        let contributions: Vec<_> = outcomes
            .into_iter()
            .map(|outcome| outcome.contribution)
            .collect();

        base.merge(&deltas, &contributions)
    }
}

#[cfg(test)]
mod test;
