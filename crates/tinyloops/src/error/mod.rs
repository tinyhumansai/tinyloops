//! Crate-wide error and result types.
//!
//! Every fallible public function in this crate returns [`Result`], and every
//! failure mode is a distinct [`Error`] variant. Add a variant rather than
//! encoding new context into an existing message: callers match on variants,
//! and message text is not a stable API.
//!
//! Variants carry the data a caller needs to react, keep their `#[error]`
//! message lowercase and free of trailing punctuation, and are documented so
//! the rendered rustdoc explains when each one occurs.

/// Errors returned by this crate.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// A required name was empty or contained only whitespace.
    #[error("name must not be empty")]
    EmptyName,

    /// A run was asked to report a success it had not earned.
    ///
    /// Raised by `Outcome::success` for a run that was blocked, expired, or
    /// spent its attempt budget. The invariant is that an error or an exhausted
    /// budget is never a success, and this variant is how the type enforces it
    /// rather than describing it.
    #[error("run did not earn a success outcome")]
    UnearnedSuccess,

    /// The generated routing ladder did not evaluate to a route name.
    ///
    /// Under the workflow engine a compile error, a run error, non-JSON output,
    /// and empty output all yield `null`, and `null` is falsey — so a broken
    /// program is otherwise indistinguishable from a decision. This variant is
    /// what makes that difference visible.
    #[error("routing ladder did not evaluate to a route name")]
    LadderNotRouted,

    /// The generated terminal condition did not evaluate to a boolean.
    ///
    /// Raised for the same reason as [`Self::LadderNotRouted`]: a loop that
    /// silently never terminates is a worse failure than one that says why.
    #[error("terminal condition did not evaluate to a boolean")]
    TerminalConditionNotBoolean,

    /// Two evaluation arms wrote the same narrative field in one merge.
    ///
    /// Counters merge by addition and need no owner; text does not. Each
    /// narrative field on a `Contribution` belongs to exactly one arm, so a
    /// second writer is a wiring mistake with no correct resolution — picking a
    /// winner would reintroduce the arrival-order dependence the delta fold
    /// exists to remove. Both arms are named so the wiring can be found.
    #[error("field {field} was written by both {held_by} and {also}")]
    ContestedField {
        /// The field both arms wrote.
        field: &'static str,
        /// The arm that claimed it first.
        held_by: &'static str,
        /// The arm that also tried to write it.
        also: &'static str,
    },

    /// A budget cap was zero, which is a scope with no bound at all.
    ///
    /// Raised by [`RunBudget::new`](crate::RunBudget::new). Zero is not spelled
    /// "unlimited" here: a run whose scope is unbounded is the run that ends
    /// when something external kills it, and that is the one ending which
    /// produces no report.
    #[error("budget cap {} must not be zero", bound.as_str())]
    UnboundedCap {
        /// The cap that was zero.
        bound: crate::Bound,
    },

    /// An inner timeout was not strictly shorter than the one containing it.
    ///
    /// Raised by [`RunBudget::new`](crate::RunBudget::new) for
    /// `tool_timeout >= run_timeout` and for `request_timeout >= tool_timeout`.
    /// The ordering is a correctness property rather than a preference: an
    /// expired tool call returns its captured output with a timeout status and
    /// the run carries on, while an expired run loses its context and its
    /// report. If the outer clock can expire first, the inner scope's graceful
    /// path is unreachable.
    #[error("{} must expire before {}", inner.as_str(), outer.as_str())]
    NestedTimeout {
        /// The bound that must expire first.
        inner: crate::Bound,
        /// The bound that must expire second.
        outer: crate::Bound,
    },

    /// Two caps within one scope could each be the first to trip.
    ///
    /// Raised by [`RunBudget::new`](crate::RunBudget::new). Exactly one cap in
    /// a scope may be reachable, and it must be the one whose overrun path
    /// preserves partial results. A configuration in which the tool-call cap
    /// can be reached before the model-call cap puts the run on the overrun
    /// path that reports nothing, so it is rejected naming both caps rather
    /// than silently preferred.
    #[error("{} is reachable before {}", reachable.as_str(), shadowed.as_str())]
    ContendedCaps {
        /// The cap the run would reach first.
        reachable: crate::Bound,
        /// The cap that was meant to stop the run.
        shadowed: crate::Bound,
    },
}

/// The crate's standard result type.
///
/// Use this alias in public signatures instead of spelling out
/// `std::result::Result<T, Error>`.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod test;
