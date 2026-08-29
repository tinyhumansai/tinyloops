//! Stopping, as a value rather than an `if`.
//!
//! Invariant 10 of `docs/specs/loop-kernel.md`: a run's stopping rule is
//! stateful, resettable, serializable, and composable, and it reports *which*
//! of five reasons ended the run. The natural shape a hand-written loop reaches
//! for — `if done_or_out_of_attempts { return the answer }` — has none of those
//! properties, and it violates the one invariant that matters by construction:
//! it reports an exhausted budget as the answer.
//!
//! # Why not an `if`
//!
//! An `if` cannot be reset, cannot be checkpointed, cannot be composed with
//! another stopping rule without editing it, and cannot say why it fired. A run
//! that stops has to report why it stopped or its outcome cannot be scored.
//!
//! # The invariant
//!
//! **An error or an exhausted budget is never [`Outcome::Success`].** It is
//! held by construction rather than by review: a fired condition reports
//! [`Outcome::classify`], which reads the disqualifying conditions first, and
//! there is no other way to build the outcome this type reports.

use std::ops::{BitAnd, BitOr};

use serde::{Deserialize, Serialize};

use crate::policy::{Outcome, Thresholds, is_terminal, terminal_condition};
use crate::state::LoopState;

/// The named state a finished run ended in.
///
/// An alias for [`Outcome`] rather than a second enum with the same five
/// variants. The specification names this vocabulary in two places — the
/// termination condition and the run's classified result — and two types would
/// need a conversion at every boundary that nothing checks, which is exactly
/// the drift the kernel's other invariants exist to prevent.
pub type TerminalState = Outcome;

/// The rule a [`TerminationCondition`] tests.
///
/// Private: the constructors are the vocabulary, and keeping the enum out of
/// the public surface means adding a rule is not a breaking change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Rule {
    /// The generated terminal condition: any terminal route, an expired clock,
    /// or a spent restart allowance.
    Terminal,
    /// The run has spent its wall clock.
    Expired,
    /// The run reached an answer it stands behind.
    Solved,
    /// Every inner rule holds.
    All(Vec<TerminationCondition>),
    /// Some inner rule holds.
    Any(Vec<TerminationCondition>),
}

/// Whether a run should stop, and what to call the ending.
///
/// Stateful: the first time the rule holds, the condition latches the
/// [`Outcome`] it fired with, so the run reports one ending rather than
/// re-deciding on every read. [`Self::reset`] clears the latch so a restart
/// begins again, and the whole value round-trips through serde so it survives a
/// checkpoint with the rest of the run state.
///
/// # Examples
///
/// ```
/// # use tinyloops::{LoopState, Outcome, TerminationCondition, Thresholds};
/// let thresholds = Thresholds::default();
/// let mut condition = TerminationCondition::terminal() | TerminationCondition::expired();
///
/// let mut state = LoopState::new("goal");
/// assert_eq!(condition.evaluate(&state, &thresholds), None);
///
/// // Out of attempts is out of attempts, however hopeful the last pass was.
/// state.solved = true;
/// state.banked = 1;
/// state.attempts = thresholds.max_attempts;
/// assert_eq!(condition.evaluate(&state, &thresholds), Some(Outcome::Exhausted));
///
/// condition.reset();
/// assert_eq!(condition.fired(), None);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminationCondition {
    rule: Rule,
    /// The ending this condition latched, or `None` while it has not fired.
    fired: Option<Outcome>,
}

impl TerminationCondition {
    fn new(rule: Rule) -> Self {
        Self { rule, fired: None }
    }

    /// The kernel's default: stop when the routing ladder reaches a terminal
    /// arm, the clock expires, or the restart allowance is spent.
    ///
    /// The jq spelling of exactly this rule is
    /// [`terminal_condition`](crate::terminal_condition), which is what the
    /// emitted loop head runs — so the stop test the Rust holds is the one the
    /// engine evaluates.
    #[must_use]
    pub fn terminal() -> Self {
        Self::new(Rule::Terminal)
    }

    /// Stop when the run has spent its wall clock.
    #[must_use]
    pub fn expired() -> Self {
        Self::new(Rule::Expired)
    }

    /// Stop when the run reached an answer it stands behind.
    #[must_use]
    pub fn solved() -> Self {
        Self::new(Rule::Solved)
    }

    /// Stop only when every one of `conditions` holds.
    #[must_use]
    pub fn all(conditions: Vec<Self>) -> Self {
        Self::new(Rule::All(conditions))
    }

    /// Stop as soon as any of `conditions` holds.
    #[must_use]
    pub fn any(conditions: Vec<Self>) -> Self {
        Self::new(Rule::Any(conditions))
    }

    /// Whether the rule holds for `state`, ignoring the latch.
    fn holds(&self, state: &LoopState, thresholds: &Thresholds) -> bool {
        match &self.rule {
            Rule::Terminal => is_terminal(state, thresholds),
            Rule::Expired => state.expired,
            Rule::Solved => state.solved,
            Rule::All(inner) => inner.iter().all(|c| c.holds(state, thresholds)),
            Rule::Any(inner) => inner.iter().any(|c| c.holds(state, thresholds)),
        }
    }

    /// Tests `state` and, on the first pass that holds, latches the ending.
    ///
    /// Returns the latched [`Outcome`] on every later call, so a condition
    /// consulted twice in one pass — or once before and once after a
    /// checkpoint — reports the same ending rather than re-deciding.
    ///
    /// The reported outcome is always [`Outcome::classify`], never a value the
    /// caller chose: that is where "an error or an exhausted budget is never
    /// [`Outcome::Success`]" is enforced.
    pub fn evaluate(&mut self, state: &LoopState, thresholds: &Thresholds) -> Option<Outcome> {
        if let Some(outcome) = self.fired {
            return Some(outcome);
        }
        if !self.holds(state, thresholds) {
            return None;
        }
        let outcome = Outcome::classify(state, thresholds);
        self.fired = Some(outcome);
        Some(outcome)
    }

    /// The ending this condition latched, if it has fired.
    #[must_use]
    pub fn fired(&self) -> Option<Outcome> {
        self.fired
    }

    /// Clears the latch, so a restart begins again.
    ///
    /// Resets every nested condition too: a composed rule that kept a fired
    /// child would report the pre-restart ending for the rest of the run.
    pub fn reset(&mut self) {
        self.fired = None;
        match &mut self.rule {
            Rule::All(inner) | Rule::Any(inner) => {
                for condition in inner {
                    condition.reset();
                }
            }
            Rule::Terminal | Rule::Expired | Rule::Solved => {}
        }
    }

    /// The jq program the loop head's `config.until` carries.
    ///
    /// Every number in it is interpolated from `thresholds` by
    /// [`terminal_condition`](crate::terminal_condition); nothing in this
    /// module types a threshold. That is invariant 7 for the stop test, the
    /// same way [`ladder`](crate::ladder) covers it for the routing switch.
    ///
    /// An empty [`Self::all`] renders `true` and an empty [`Self::any`] renders
    /// `false`, which are the identities of the two operators — a loop given
    /// "stop when all of nothing holds" stops immediately, and one given "stop
    /// when any of nothing holds" runs to its cap.
    #[must_use]
    pub fn expression(&self, thresholds: &Thresholds) -> String {
        format!("={}", self.program(thresholds))
    }

    /// The `=`-less body, so a composed rule can nest it.
    fn program(&self, thresholds: &Thresholds) -> String {
        match &self.rule {
            Rule::Terminal => {
                let rendered = terminal_condition(thresholds);
                let body = rendered.strip_prefix('=').unwrap_or(&rendered);
                format!("({body})")
            }
            Rule::Expired => "((.state // .item) as $s | (($s | .expired) // false))".to_string(),
            Rule::Solved => "((.state // .item) as $s | (($s | .solved) // false))".to_string(),
            Rule::All(inner) => Self::join(inner, thresholds, "and", "true"),
            Rule::Any(inner) => Self::join(inner, thresholds, "or", "false"),
        }
    }

    fn join(inner: &[Self], thresholds: &Thresholds, operator: &str, empty: &str) -> String {
        if inner.is_empty() {
            return format!("({empty})");
        }
        let parts: Vec<String> = inner
            .iter()
            .map(|condition| condition.program(thresholds))
            .collect();
        format!("({})", parts.join(&format!(" {operator} ")))
    }
}

impl Default for TerminationCondition {
    /// [`Self::terminal`]: the stop test the kernel's ladder already defines.
    fn default() -> Self {
        Self::terminal()
    }
}

impl BitAnd for TerminationCondition {
    type Output = Self;

    /// Stop only when both hold.
    fn bitand(self, rhs: Self) -> Self {
        Self::all(vec![self, rhs])
    }
}

impl BitOr for TerminationCondition {
    type Output = Self;

    /// Stop as soon as either holds.
    fn bitor(self, rhs: Self) -> Self {
        Self::any(vec![self, rhs])
    }
}
