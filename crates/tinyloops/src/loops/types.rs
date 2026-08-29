//! The node identities the kernel emits, and the addresses they are read at.
//!
//! Everything here is a *declaration*. Invariant 9 of
//! `docs/specs/loop-kernel.md` requires node identity to be chosen by the
//! builder rather than derived from allocation or insertion order, because
//! identity derived from position renumbers a node's neighbours the moment one
//! is added and a resumed run then replays the wrong step. [`NodeIds`] is that
//! choice written down: one `&'static str` per node, changed only by editing
//! this file.

/// The id of every node the kernel emits.
///
/// A struct rather than a set of free constants so a caller can rename the
/// whole graph's nodes at once — two loops in one host workflow need distinct
/// ids — while the defaults stay the names the specification uses.
///
/// # Examples
///
/// ```
/// # use tinyloops::NodeIds;
/// let ids = NodeIds::default();
/// assert_eq!(ids.loop_head, "loop");
/// assert_eq!(ids.accumulator_address(), "=nodes.loop.state");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct NodeIds {
    /// The entry node.
    pub trigger: &'static str,
    /// Decomposes the goal, once, before the loop.
    pub plan: &'static str,
    /// Gathers context, once, before the first attempt.
    pub research: &'static str,
    /// Starts the work opened *beside* the loop, without waiting for it.
    pub side_arms: &'static str,
    /// The bounded loop head that owns the accumulator.
    pub loop_head: &'static str,
    /// The approval point emitted only at `Autonomy::Assisted`.
    pub approval: &'static str,
    /// The one attempt of a pass.
    pub attempt: &'static str,
    /// The barrier every evaluation arm converges on, and the fold.
    pub merge: &'static str,
    /// The routing switch.
    pub route: &'static str,
    /// The single node every route enters, and the only one closing the cycle.
    pub pass: &'static str,
    /// Retires whatever is still running beside the run.
    pub stand_down: &'static str,
    /// Writes the run's report.
    pub report: &'static str,
}

impl Default for NodeIds {
    fn default() -> Self {
        Self {
            trigger: "trigger",
            plan: "plan",
            research: "research",
            side_arms: "side_arms",
            loop_head: "loop",
            approval: "approve",
            attempt: "attempt",
            merge: "merge",
            route: "route",
            pass: "pass",
            stand_down: "stand_down",
            report: "report",
        }
    }
}

impl NodeIds {
    /// The address of the loop's accumulator.
    ///
    /// The one address an evaluation arm must never be wired to (invariant 3):
    /// the head folds at the *top* of a pass, so mid-body it holds the state as
    /// of the previous pass. It is a method rather than a constant because the
    /// loop head's id is a field, and it exists so the arm test has one string
    /// to look for rather than a spelling to guess at.
    #[must_use]
    pub fn accumulator_address(&self) -> String {
        format!("=nodes.{}.state", self.loop_head)
    }

    /// Every node id this struct names, in the order the loop reaches them.
    ///
    /// Includes [`Self::approval`], which only some autonomy levels emit; the
    /// builder decides what is in the graph, and this is the vocabulary.
    #[must_use]
    pub fn all(&self) -> [&'static str; 12] {
        [
            self.trigger,
            self.plan,
            self.research,
            self.side_arms,
            self.loop_head,
            self.approval,
            self.attempt,
            self.merge,
            self.route,
            self.pass,
            self.stand_down,
            self.report,
        ]
    }
}

/// The address a completed node's payload is read at.
///
/// A re-export of [`upstream_address`](crate::upstream_address) under the name
/// this module reads better with. It is deliberately *not* a second
/// implementation: the address is one fact, and the two copies that existed
/// while `upstream_address` was wrong are exactly how a wrong one survives —
/// the builder was correct and the helper every other caller used was not.
pub(super) use crate::upstream_address as payload_address;

/// Whether `config` mentions `address` anywhere, at any depth.
///
/// Used by the invariant tests: "no arm reads the accumulator" is a statement
/// about the emitted JSON, so it is asserted against the emitted JSON rather
/// than against the code that wrote it. Nothing in the builder needs it, so it
/// is compiled only for those tests.
#[cfg(test)]
pub(super) fn mentions(config: &serde_json::Value, address: &str) -> bool {
    use serde_json::Value;
    match config {
        Value::String(text) => text.contains(address),
        Value::Array(items) => items.iter().any(|item| mentions(item, address)),
        Value::Object(map) => map.values().any(|value| mentions(value, address)),
        _ => false,
    }
}
