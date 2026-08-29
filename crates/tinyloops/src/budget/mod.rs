//! The limits every run carries, and the rules that decide whether a set of
//! them is a legal configuration.
//!
//! A loop that can retry can also fail to stop. Every attempt looks locally
//! reasonable — one more pass, one more tool call, one more sub-agent — and a
//! run with no bound ends when something external kills it, which is the one
//! ending that produces no report.
//!
//! # The bounds are concentric
//!
//! Outermost first: the loop (its iteration cap and its wall clock), the
//! per-role caps on model calls, tool calls and tokens, the per-tool timeout,
//! and the per-request timeout. Each nests inside the one before it. A single
//! global cap does not do this job, because each scope can overrun
//! independently: the loop can spin without any one step being slow, and a step
//! can hang without the loop having taken many passes.
//!
//! # Only one cap may be the one that trips
//!
//! The caps are not equally graceful. Some overruns end with a report and
//! partial results; others end with nothing. So the bounds are set such that
//! **the cap whose overrun is graceful is the reachable one**, and that
//! ordering is asserted at construction rather than left to whoever wrote the
//! configuration:
//!
//! - The tool-call cap sits far above what the model-call cap can reach, so the
//!   model-call cap is what actually stops a run. A configuration where both
//!   could trip is a bug, and [`RunBudget::new`] rejects it naming both caps.
//! - `tool_timeout < run_timeout`, for the same reason in the time dimension.
//!   An expired tool call returns the output it captured, tagged with a timeout
//!   status, and the run continues holding everything it had. An expired *run*
//!   loses its context and its report. If the run clock can expire while a tool
//!   call is outstanding, the tool's graceful path is unreachable.
//!
//! # Two meters, not one
//!
//! [`Meter`] advances raw compute and **effective feedback** side by side. Raw
//! compute bounds the worst case; effective feedback — the passes that produced
//! a usable signal — is what a stopping decision should read. See [`Meter`] for
//! why the distinction is load-bearing rather than decorative.
//!
//! # Arithmetic
//!
//! Saturating throughout, and no panics. This code runs inside nodes the engine
//! cannot unwind sensibly, and a wrapped counter reads as a fresh, unbudgeted
//! run.

use std::time::Duration;

mod types;

pub use types::{Bound, Caps, Meter, RunBudget, TOOL_CALLS_PER_MODEL_CALL};

use crate::{Error, Result};

/// Rejects a zero cap.
///
/// Zero is not "no limit" here, it is "a scope with no bound", and the whole
/// module exists because an unbounded scope is how a run ends with nothing to
/// show.
fn positive(value: u64, bound: Bound) -> Result<()> {
    if value == 0 {
        Err(Error::UnboundedCap { bound })
    } else {
        Ok(())
    }
}

/// Rejects a zero timeout, which is a scope that ends before it starts.
fn positive_duration(value: Duration, bound: Bound) -> Result<()> {
    if value.is_zero() {
        Err(Error::UnboundedCap { bound })
    } else {
        Ok(())
    }
}

/// Asserts that an inner timeout expires strictly before the outer one.
///
/// Strictly, not "at or before": equal timeouts race, and which one wins
/// decides whether the run keeps its report.
fn ordered(inner: Duration, outer: Duration, inner_bound: Bound, outer_bound: Bound) -> Result<()> {
    if inner < outer {
        Ok(())
    } else {
        Err(Error::NestedTimeout {
            inner: inner_bound,
            outer: outer_bound,
        })
    }
}

/// Asserts that the tool-call cap cannot be reached before the model-call cap.
///
/// The check is a multiplication rather than a comparison because the caps
/// measure different things: the question is not "is the tool cap larger" but
/// "can the model calls this run is allowed issue enough tool calls to reach
/// it", and one model call can fan out to
/// [`TOOL_CALLS_PER_MODEL_CALL`] of them.
fn uncontended(max_model_calls: u32, max_tool_calls: u32) -> Result<()> {
    let reach = max_model_calls.saturating_mul(TOOL_CALLS_PER_MODEL_CALL);
    if max_tool_calls >= reach {
        Ok(())
    } else {
        Err(Error::ContendedCaps {
            reachable: Bound::ToolCalls,
            shadowed: Bound::ModelCalls,
        })
    }
}

#[cfg(test)]
mod test;
