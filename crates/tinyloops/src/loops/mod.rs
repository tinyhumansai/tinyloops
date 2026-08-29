//! The graph a goal run is: one builder, one `WorkflowGraph`, eleven
//! invariants.
//!
//! The kernel's thesis is that **the graph owns routing and Rust owns the
//! steps**. Every branch a run can take is declared in the emitted graph, where
//! it is inspectable, renderable, and diffable against the Rust that generated
//! it; everything a node *does* is a Rust step. This module is the first half —
//! `src/step/` and `src/arm/` are the second.
//!
//! # The shape
//!
//! ```text
//! trigger → plan → research → side_arms → loop ──done──→ stand_down → report
//!                                          │
//!                                          └─ body → attempt ─┬→ …arms… ─→ merge → route
//!                                                ▲                                  │
//!                                                └───────────── pass ←──────────────┘
//! ```
//!
//! `plan` and `research` run once, before the loop, so the first attempt starts
//! from a decomposition and a context rather than spending a pass acquiring
//! them. Every node inside the body is a `NodeKind::ToolCall` naming
//! [`RUN_LOOP_STEP`](crate::RUN_LOOP_STEP) with a step, so the whole pass is one
//! closed set the graph addresses by name.
//!
//! # `Spawn` and `Gate`, and why they are not `ToolCall`
//!
//! The work opened *beside* the loop is different in kind from the work inside
//! it: it does not gate the pass, it outlives it, and it has to be retired at
//! the end. `side_arms` is a `NodeKind::Spawn` and `stand_down` is a
//! `NodeKind::Gate`, which is what makes standing down **a node the graph can
//! reach** rather than a cleanup call somebody remembers to make after the
//! workflow returns.
//!
//! That boundary is drawn here rather than left to each builder because the
//! failure it prevents has a measured cost. A live run of this design recorded
//! its verdict at minute 29, kept spawning helpers for another 62 minutes
//! because nothing had retired them, and spent roughly 85% of its wall clock
//! and most of its budget after the problem was already solved.
//!
//! `Spawn` needs no `TaskRunner`: with none injected the work runs inline and
//! the ticket comes back already settled, so a host without a scheduler
//! computes the same answer and loses only the overlap.
//!
//! # What each invariant is, in this module
//!
//! - **2, one exit.** Every route port enters `pass`, and `pass` alone carries
//!   the edge back to the head. Routing the barrier straight back to `attempt`
//!   would create an inner cycle the head never sees, which `max_iterations`
//!   cannot bound.
//! - **3, arms read the previous step.** An arm's input is
//!   [`upstream_address`](crate::upstream_address)'s node — the attempt —
//!   never [`NodeIds::accumulator_address`]. The head folds at the *top* of a
//!   pass, so mid-body the accumulator is one pass behind.
//! - **4, the fold is at-least-once.** The head's `config.state.update` assigns
//!   the whole state `pass` returned. A replayed activation applies it twice
//!   and lands on the same value; `attempts + 1` twice is wrong by one and
//!   nothing reports it.
//! - **6, one list.** The fan-out edges and the barrier's fold inputs are both
//!   derived from one [`ArmSet`](crate::ArmSet).
//! - **7, generated thresholds.** The head's `until` comes from
//!   [`terminal_condition`](crate::terminal_condition) through
//!   [`TerminationCondition`], and the switch's expression from
//!   [`ladder`](crate::ladder). No threshold is typed here.
//! - **9, the signature.** [`GraphSignature`] hashes the emitted topology and
//!   [`verify_resume`] refuses a mismatched checkpoint by name.
//! - **10, composable termination.** [`TerminationCondition`].
//!
//! # Addressing, and the one place it is easy to get wrong
//!
//! Under this engine a compile error, a run error, non-JSON output, and empty
//! output all yield `null`, silently, and `null` is falsey. A binding that
//! reads a key nothing writes is therefore indistinguishable from a decision.
//! Every address this module emits is a *simple dotted path*
//! (`=nodes.<id>.item.json`), which the engine walks segment by segment rather
//! than compiling as jq, so a hyphenated node id stays addressable — and every
//! emitted graph is expected to pass `TestRun::assert_no_null_bindings`, which
//! is the only check that catches the difference.

mod builder;
mod signature;
mod termination;
mod types;

pub use builder::{LoopBuilder, STEP_MERGE};
pub use signature::{GraphSignature, verify_resume};
pub use termination::{TerminalState, TerminationCondition};
pub use types::NodeIds;

#[cfg(test)]
mod test;
