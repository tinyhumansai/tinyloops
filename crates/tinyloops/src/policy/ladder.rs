//! The routing decision, emitted as the jq the graph actually runs.
//!
//! The loop's branch is taken inside a workflow graph, and a graph branches on
//! a `=`-prefixed jq expression. So the decision [`route`](super::route) makes
//! in Rust has to exist a second time, in jq, and the two have to agree.
//!
//! # How they are kept in agreement
//!
//! Neither program holds a threshold. Both *address* one, reading it out of the
//! accumulator at `.profile.thresholds.<field>` — the same place
//! [`route`](super::route) reads it. A literal `2` in the jq would be a second
//! copy of a constant, free to drift from the Rust the moment either is tuned,
//! and drift between a router and its ladder is invisible: both sides still
//! produce a route, they just produce different ones.
//!
//! Rendering the number in, which is what these functions used to do, removed
//! that drift and bought a different problem: the emitted graph became a
//! function of the thresholds, so changing one changed the topology and a
//! resume across the change was refused. See
//! `docs/adr/0006-thresholds-addressed-from-run-state.md`. Addressing keeps the
//! single source and drops the coupling, and it means these are *constant*
//! programs — one ladder serves every preset and every revision of one.
//!
//! The program emits exactly [`Route::as_str`], which is what
//! [`Route::parse`] reads back.
//!
//! # The sentinel, and why it is not zero
//!
//! Every threshold read carries `// 4294967295`. That is `u32::MAX` standing
//! for "this state names no threshold", and it makes every rung of the ladder
//! false, so a state with no profile falls through to [`Route::Retry`] — the
//! cheap outcome every default in this crate points at.
//!
//! It cannot be left off. A missing key resolves to `null`, `null` sorts below
//! every number in jq, and `0 >= null` is therefore **true** — so an unguarded
//! read would fire the *first* rung and route [`Route::Blocked`] on a state
//! that simply had no profile, which is the most expensive answer available.
//!
//! # Addressing the accumulator
//!
//! Both programs begin `(.state // .item) as $s`, which covers the two places
//! the engine presents the accumulator:
//!
//! - inside the loop head's `until`, the engine adds the post-fold accumulator
//!   to the scope as `state`;
//! - at any node downstream of the loop, the accumulator arrives as that node's
//!   input, so it is `item`.
//!
//! It is also always at `nodes.<loop id>.state`, which is what
//! [`expr_scope`] reproduces, but that address needs the loop's id and these
//! functions generate one program for any graph.
//!
//! # A note on failure
//!
//! Under this engine a broken jq program is indistinguishable from a false
//! condition: a compile error, a run error, non-JSON output, and empty output
//! all yield `Value::Null`, and `null` is falsey. [`evaluate_ladder`] and
//! [`evaluate_terminal_condition`] therefore return an error rather than a
//! default when the program does not produce the shape it should.

use serde_json::{Value, json};
use tinyflows::expr;

use super::{Route, Thresholds};
use crate::state::LoopState;
use crate::{Error, Result};

/// The value a threshold read falls back to when the state names none.
///
/// `u32::MAX`, so every rung of the ladder is false and the run falls through
/// to [`Route::Retry`]. See the module docs for why the fallback cannot be
/// omitted and cannot be zero.
const NO_THRESHOLD: u32 = u32::MAX;

/// Returns the jq program that evaluates to a [`Route::as_str`] name.
///
/// The arms are tested in the same order as [`route`](super::route), and for
/// the same reasons; that documentation is the specification and this is its
/// translation.
///
/// The program is a constant: it addresses the thresholds in the accumulator
/// rather than carrying them, so one ladder routes every preset and every
/// revision of one.
///
/// # Examples
///
/// ```
/// # use tinyloops::ladder;
/// let program = ladder();
/// assert!(program.starts_with('='));
/// assert!(program.contains(".profile.thresholds"));
/// assert!(!program.contains(">= 2"));
/// ```
#[must_use]
pub fn ladder() -> String {
    format!(
        "=(.state // .item) as $s | (($s | .profile.thresholds) // {{}}) as $t \
| if ((($s | .blocked) // 0) >= (($t | .blocked) // {none})) then \"{route_blocked}\" \
elif ((($s | .solved) // false) or ((($s | .attempts) // 0) >= (($t | .max_attempts) // {none}))) then \"{route_solved}\" \
elif ((($s | .unverified) // 0) >= (($t | .unverified) // {none})) then \"{route_reported}\" \
elif (((($s | .unproductive) // 0) >= (($t | .stuck) // {none})) or ((($s | .computational) // 0) >= (($t | .computational) // {none}))) then \"{route_diversify}\" \
else \"{route_retry}\" \
end",
        none = NO_THRESHOLD,
        route_blocked = Route::Blocked.as_str(),
        route_solved = Route::Solved.as_str(),
        route_reported = Route::Reported.as_str(),
        route_diversify = Route::Diversify.as_str(),
        route_retry = Route::Retry.as_str(),
    )
}

/// Returns the jq program for the loop head's `until`.
///
/// True exactly when [`is_terminal`](super::is_terminal) is true: any terminal
/// route, an expired clock, or a spent restart allowance. Written as a
/// disjunction rather than as a copy of the ladder because `until` only needs
/// to know *whether* the run stops, not which arm stopped it, and the
/// disjunction is the same set of conditions with the ordering removed.
///
/// # Examples
///
/// ```
/// # use tinyloops::{Thresholds, terminal_condition};
/// let program = terminal_condition(&Thresholds::default());
/// assert!(program.starts_with('='));
/// assert!(program.contains(">= 8"));
/// ```
#[must_use]
pub fn terminal_condition(thresholds: &Thresholds) -> String {
    format!(
        "=(.state // .item) as $s \
| ((($s | .expired) // false) \
or ((($s | .restarts) // 0) >= {max_restarts}) \
or (($s | .solved) // false) \
or ((($s | .attempts) // 0) >= {max_attempts}) \
or ((($s | .blocked) // 0) >= {blocked}) \
or ((($s | .unverified) // 0) >= {unverified}))",
        max_restarts = thresholds.max_restarts,
        max_attempts = thresholds.max_attempts,
        blocked = thresholds.blocked,
        unverified = thresholds.unverified,
    )
}

/// Builds the evaluation scope the engine would build around `state`.
///
/// The accumulator appears at every address the engine offers it: as the
/// previous step's `item`, in `items`, as the loop head's `state`, and at
/// `nodes.<loop_id>.state`. Reproducing all of them is what lets a program be
/// evaluated — and compared against [`route`](super::route) — outside a running
/// graph.
///
/// It also matters for testing: if the scope carried the accumulator at only
/// one address, a program that regressed to reading a different one would
/// resolve to `null` and, since `null` is falsey, look like a decision rather
/// than a break.
///
/// # Examples
///
/// ```
/// # use tinyloops::{LoopState, expr_scope};
/// let scope = expr_scope(&LoopState::new("goal"), "loop");
/// assert_eq!(scope["state"]["goal"], "goal");
/// assert_eq!(scope["nodes"]["loop"]["state"]["goal"], "goal");
/// ```
#[must_use]
pub fn expr_scope(state: &LoopState, loop_id: &str) -> Value {
    // `to_value` on a plain struct of strings, numbers, and vectors cannot
    // fail; `Null` is the honest fallback rather than a panic inside a node.
    let accumulator = serde_json::to_value(state).unwrap_or(Value::Null);
    // Built by hand rather than as one `json!` literal: the loop's id is a
    // runtime string, not a literal key.
    let mut nodes = serde_json::Map::new();
    nodes.insert(
        loop_id.to_string(),
        json!({
            "item": accumulator.clone(),
            "items": [accumulator.clone()],
            "state": accumulator.clone(),
            "iteration": state.passes,
        }),
    );
    json!({
        "item": accumulator.clone(),
        "items": [accumulator.clone()],
        "run": { "inputs": Value::Null },
        "inputs": Value::Null,
        "nodes": Value::Object(nodes),
        "state": accumulator,
    })
}

/// Evaluates [`ladder`] against `state` and returns the route the graph would
/// take.
///
/// # Errors
///
/// Returns [`Error::LadderNotRouted`] when the program does not produce a
/// string — which, under this engine, is how a compile error, a run error, and
/// an empty result all present.
///
/// # Examples
///
/// ```
/// # use tinyloops::{LoopState, Route, Thresholds, evaluate_ladder};
/// let mut state = LoopState::new("goal");
/// state.blocked = 2;
/// assert_eq!(evaluate_ladder(&state, "loop", &Thresholds::default())?, Route::Blocked);
/// # Ok::<(), tinyloops::Error>(())
/// ```
pub fn evaluate_ladder(state: &LoopState, loop_id: &str, thresholds: &Thresholds) -> Result<Route> {
    let scope = expr_scope(state, loop_id);
    let evaluated = expr::evaluate(&Value::String(ladder(thresholds)), &scope);
    evaluated
        .as_str()
        .map(Route::parse)
        .ok_or(Error::LadderNotRouted)
}

/// Evaluates [`terminal_condition`] against `state`.
///
/// # Errors
///
/// Returns [`Error::TerminalConditionNotBoolean`] when the program does not
/// produce a boolean.
///
/// # Examples
///
/// ```
/// # use tinyloops::{LoopState, Thresholds, evaluate_terminal_condition};
/// let mut state = LoopState::new("goal");
/// state.expired = true;
/// assert!(evaluate_terminal_condition(&state, "loop", &Thresholds::default())?);
/// # Ok::<(), tinyloops::Error>(())
/// ```
pub fn evaluate_terminal_condition(
    state: &LoopState,
    loop_id: &str,
    thresholds: &Thresholds,
) -> Result<bool> {
    let scope = expr_scope(state, loop_id);
    let evaluated = expr::evaluate(&Value::String(terminal_condition(thresholds)), &scope);
    evaluated
        .as_bool()
        .ok_or(Error::TerminalConditionNotBoolean)
}
