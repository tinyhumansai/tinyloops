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

    /// A graph node named a step the registry does not hold.
    ///
    /// The step set is closed, and this variant is what closes it. An
    /// unrecognised name must never be a no-op: the node would run green,
    /// change nothing, and leave the loop to route on a state nobody advanced.
    #[error("no step named {name} is registered")]
    UnknownStep {
        /// The name the node asked for.
        name: String,
    },

    /// A step was registered under a name that was already taken.
    ///
    /// A name in a closed set has exactly one meaning. Replacing a body
    /// silently would make which one runs depend on registration order.
    #[error("a step named {name} is already registered")]
    DuplicateStep {
        /// The contested name.
        name: String,
    },

    /// A `run_loop_step` payload was missing a field or held the wrong shape.
    ///
    /// Reading a default step name or a default accumulator instead would route
    /// the run on something nobody computed, which is the failure the closed
    /// step set exists to prevent arriving by another door.
    #[error("step payload field {field} is missing or malformed")]
    MalformedStepPayload {
        /// The field that could not be read.
        field: &'static str,
    },

    /// The accumulator a step returned could not be serialized.
    ///
    /// Unreachable for the accumulator as it stands — every field serializes
    /// infallibly — but the failure is reported rather than unwrapped, because
    /// a panic inside a graph node is far worse than an error the run can route
    /// on.
    #[error("could not encode the returned accumulator")]
    StateEncoding,

    /// A loop was declared with no evaluation arms.
    ///
    /// A loop nothing evaluates has no evidence to route on and no arm able to
    /// conclude, so it can only run to its iteration cap and report nothing
    /// about why.
    #[error("a loop must declare at least one evaluation arm")]
    EmptyArmSet,

    /// Two evaluation arms were declared under the same name.
    ///
    /// The name is a node id, a step name, and a fold key at once, so a
    /// duplicate silently replaces the other arm in all three.
    #[error("an arm named {name} is already declared")]
    DuplicateArm {
        /// The contested name.
        name: String,
    },

    /// More than one arm declared itself able to end the run.
    ///
    /// Exactly one arm may conclude. Two means the run's outcome depends on
    /// which of them finished first — the arrival-order dependence every other
    /// rule here exists to remove, arriving through the one door left open.
    #[error("both {first} and {second} may conclude the run")]
    AmbiguousConclusion {
        /// The arm that declared it first.
        first: &'static str,
        /// The arm that also declared it.
        second: &'static str,
    },

    /// A merge was handed an outcome from an arm the set does not declare.
    ///
    /// Folding it would credit the run with evidence no declared arm produced.
    #[error("no arm named {name} is declared")]
    UnknownArm {
        /// The undeclared arm.
        name: &'static str,
    },

    /// A call named a tool the set does not hold.
    ///
    /// This is also what a withheld tool group looks like from the call site: a
    /// grant is resolved in `ToolSet::new`, so a tool the attempt may not use
    /// was never registered and there is nothing here to reach.
    #[error("no tool named {name} is registered")]
    UnknownTool {
        /// The name the call asked for.
        name: String,
    },

    /// A tool failed with the one recovery that ends a step.
    ///
    /// `Recovery::Requery` and `Recovery::Salvage` never arrive here: they
    /// become model-readable results and a line in the tool history. Only
    /// `Recovery::Fatal` terminates anything.
    #[error("tool {tool} failed fatally: {message}")]
    ToolFatal {
        /// The tool that failed.
        tool: String,
        /// What it reported.
        message: String,
    },

    /// One tool's failures were fed back more times than the bound allows.
    ///
    /// The bound is the point of the requery path. Without it a single broken
    /// tool spends the run's entire budget on the same question and the run
    /// reports nothing, because it never got anywhere else.
    #[error("tool {tool} exhausted its {limit} requeries")]
    RequeriesExhausted {
        /// The tool that kept failing.
        tool: String,
        /// The bound it passed.
        limit: u32,
    },

    /// A write named a kind the layout does not list.
    ///
    /// The allowlist is what makes "where could this run have written?" a
    /// question answered by reading the layout rather than by scanning a disk,
    /// so an unlisted kind lands no bytes at all.
    #[error("no location is listed for write kind {kind}")]
    UnlistedWriteKind {
        /// The kind that was refused.
        kind: crate::WriteKind,
    },

    /// A path held a traversal segment.
    ///
    /// Rejected before any file system call, because a path that has already
    /// been opened has already left the workspace.
    #[error("path {path} holds a traversal segment")]
    PathTraversal {
        /// The path that was refused.
        path: String,
    },

    /// A path was absolute.
    ///
    /// A workspace decides where a write lands; an absolute path is a caller
    /// deciding instead, and the layout can no longer answer for it.
    #[error("path {path} is absolute")]
    AbsolutePath {
        /// The path that was refused.
        path: String,
    },

    /// A write was aimed inside a derived folder.
    ///
    /// Ledgers are derived state: rendering is the only way bytes enter them.
    /// The refusal is by folder rather than by filename, so a file the
    /// implementation has never seen does not escape it by being new.
    #[error("path {path} is inside a derived folder")]
    DerivedWrite {
        /// The path that was refused.
        path: String,
    },

    /// A path's canonical parent left the workspace between the two checks.
    ///
    /// Validation and the write are different moments. A symlink swapped
    /// between them is exactly the gap the first check appears to close and
    /// does not, so the canonical parent is re-verified immediately before the
    /// bytes land.
    #[error("the canonical parent of {path} left the workspace")]
    ParentEscaped {
        /// The path whose parent moved.
        path: String,
    },

    /// A read named a path the workspace does not hold.
    #[error("the workspace holds nothing at {path}")]
    UnknownPath {
        /// The path that was read.
        path: String,
    },

    /// An operation named a ledger entry that does not exist.
    ///
    /// Entries are created by merging an event, never implicitly, so this is a
    /// caller reading an identity nothing recorded.
    #[error("no ledger entry named {id} exists")]
    UnknownEntry {
        /// The identity that was asked for.
        id: String,
    },

    /// An operation named a completion criterion the run spec does not hold.
    #[error("no criterion named {id} exists")]
    UnknownCriterion {
        /// The identity that was asked for.
        id: String,
    },

    /// A criterion was asked to pass on evidence nothing had recorded.
    ///
    /// A criterion moves to `true` only through evidence recorded against it.
    /// Assignment is what an agent with a preference does; this variant is the
    /// difference between the two.
    #[error("criterion {id} has no recorded evidence")]
    EvidenceNotRecorded {
        /// The criterion that was not satisfied.
        id: String,
    },

    /// A declared arm produced no outcome for a merge.
    ///
    /// Invariant 6 checked at the barrier as well as at the edges: the point of
    /// deriving both edge sets from one list is that an arm cannot run, cost
    /// its budget, and then go unfolded.
    #[error("arm {name} was declared but not folded")]
    ArmNotFolded {
        /// The arm missing from the fold.
        name: &'static str,
    },

    /// A role was declared with no caps of its own.
    ///
    /// Raised by [`RoleRegistry::declare`](crate::RoleRegistry::declare). A
    /// role with no caps runs on whatever budget it is handed, and the failure
    /// that produces is specific rather than abstract: a role that reads a
    /// report and answers in four lines, given an investigation's budget,
    /// investigates, because it has the calls.
    #[error("role {role} was declared without caps")]
    RoleWithoutCaps {
        /// The role that was missing them.
        role: String,
    },

    /// Two roles were declared under the same name.
    ///
    /// A role name is what a call site says instead of a model configuration,
    /// so it has exactly one meaning. Replacing one silently would make which
    /// prompt, grant, and budget ran depend on declaration order.
    #[error("a role named {role} is already declared")]
    DuplicateRole {
        /// The contested name.
        role: String,
    },

    /// A call named a role the registry does not hold.
    ///
    /// Never a fallback to a default role: the run would proceed on a prompt, a
    /// grant, and a budget nobody chose, and none of the three is visible from
    /// outside the process.
    #[error("no role named {role} is declared")]
    UnknownRole {
        /// The name the caller asked for.
        role: String,
    },

    /// A delegation handle was not issued by the harness it was presented to.
    ///
    /// A ticket is the only thing tying a caller to work in flight. Treating an
    /// unrecognised one as "not finished yet" would leave the caller polling
    /// something that does not exist.
    #[error("no delegation is held for ticket {ticket}")]
    UnknownTicket {
        /// The handle that resolved to nothing.
        ticket: String,
    },

    /// The harness declined to start a delegation.
    ///
    /// Distinct from a delegation that started and failed: nothing ran, so
    /// there is nothing to salvage, and the pass must decide what to do
    /// instead rather than read an outcome.
    #[error("spawn of {role} refused: {reason}")]
    SpawnRefused {
        /// The role that was not started.
        role: String,
        /// Why the harness declined.
        reason: String,
    },

    /// A write was acknowledged by the store but could not be read back.
    ///
    /// Raised by [`Memory::remember`](crate::Memory::remember) when its
    /// verification probe fails. "The store accepted it" and "the store has it"
    /// are different observations, and only the second is a write: one
    /// production run logged 193 successful `remember` calls and stored zero
    /// documents, because the backend answered `200 {"status":"running"}` and
    /// dropped the work. Every one of those calls was reported as a success by
    /// the only signal available.
    #[error("write to scope {scope} was acknowledged but not retained")]
    WriteNotDurable {
        /// The scope whose read-back came back empty or wrong.
        scope: String,
    },

    /// A checkpoint was resumed against a graph it was not taken from.
    ///
    /// The loop graph is generated *from* the thresholds, so changing a
    /// constant changes the topology. Restoring an old checkpoint onto the new
    /// shape puts state into slots that no longer mean what they meant, which
    /// is silent corruption rather than a crash. Both signatures are named so
    /// the difference can be found rather than guessed at.
    #[error("checkpoint signature {recorded} does not match the current graph {current}")]
    GraphSignatureMismatch {
        /// The signature the checkpoint recorded.
        recorded: String,
        /// The signature of the graph being resumed onto.
        current: String,
    },

    /// The emitted loop graph did not pass the engine's structural validation.
    ///
    /// Carries the engine's own message rather than a re-classification of it:
    /// the validator names the node and the reason, and restating that as a
    /// closed set of variants here would go stale the first time the engine
    /// learns a new check.
    #[error("the emitted loop graph is not valid: {reason}")]
    InvalidLoopGraph {
        /// The engine's validation message.
        reason: String,
    },
}

/// The crate's standard result type.
///
/// Use this alias in public signatures instead of spelling out
/// `std::result::Result<T, Error>`.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod test;
