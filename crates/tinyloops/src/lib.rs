//! `TinyLoops`: a loop engineering framework.
//!
//! The scaffolding for building agentic loops that run a workflow, judge what
//! came back, and decide whether to go round again. The pieces it is assembled
//! from are vendored rather than reimplemented: [TinyFlows] executes one graph
//! and decides nothing, `TinyAgents` is the optional durable harness, and `TinyBus`
//! is how a loop is reached from outside the process.
//!
//! The layering this crate occupies is worth stating, because it is what keeps
//! the crate small: the engine may know about one graph run; anything that
//! spans runs belongs to `tinyflows-adaptive`; and the shape of *one goal run*
//! — attempt, evaluate, route, budget, roles, tools, memory, workspace — is
//! this crate.
//!
//! # Layout
//!
//! This is the implementation half of a two-crate workspace:
//!
//! - [`tinyloops_bus`] — the wire contract. Member names, payload types, and the
//!   contract version, with no transport and no behavior. A host that only
//!   makes calls depends on that crate alone.
//! - `tinyloops` — this crate. The behavior, the crate-wide error type, and the
//!   `TinyBus` adapter that serves them, built as both an `rlib` and the
//!   `cdylib` the loader consumes.
//!
//! Within this crate:
//!
//! - `src/error/` holds the crate-wide [`Error`] enum and the [`Result`] alias
//!   returned by every fallible public function.
//! - Each feature area lives in its own module directory with a `mod.rs`
//!   module root, an optional `types.rs`, and a `test.rs` holding its unit
//!   tests.
//! - Every public item is re-exported from here — including all of
//!   [`tinyloops_bus`] — so downstream users have a single predictable surface
//!   and `tinyloops::GreetRequest` is the *same type* as
//!   `tinyloops_bus::GreetRequest`, not a structural twin.
//! - `tinybus_module` adapts the public behavior to `TinyBus` and exports the
//!   module descriptor, embedded manifest, and initialization entrypoint.
//!
//! # Example
//!
//! ```
//! use tinyloops::{greet, Error, GreetRequest};
//!
//! assert_eq!(greet("Ferris")?, "Hello, Ferris!");
//! assert_eq!(greet("   ").unwrap_err(), Error::EmptyName);
//! assert_eq!(GreetRequest::new("Ferris").name, "Ferris");
//! # Ok::<(), tinyloops::Error>(())
//! ```
//!
//! [TinyFlows]: https://github.com/tinyhumansai/tinyflows

mod arm;
mod budget;
mod error;
mod greeting;
mod observe;
mod policy;
mod state;
mod step;
mod tinybus_module;

// The limits every run carries, and the events it emits while spending them.
// `budget` is what stops a run; `observe` is what the run leaves behind so a
// person can see why it stopped.
pub use budget::{Bound, Caps, Meter, RunBudget, TOOL_CALLS_PER_MODEL_CALL};
pub use error::{Error, Result};
pub use observe::{
    Accounting, Capture, Entry, Event, FanOutSink, JsonlSink, LineSink, ModelCall, PassProfile,
    Recorder, RedactingSink, Report, Sink, Spend, StepTiming, ToolCall, Unit, Unpaired, render,
};
pub use greeting::greet;
// The loop's accumulator and the decision made from it. `state` is what one
// goal run carries between turns; `policy` is the routing that reads it, in
// both the Rust and the jq spelling.
pub use policy::{
    Autonomy, Judgement, Outcome, Route, Thresholds, evaluate_ladder, evaluate_terminal_condition,
    expr_scope, is_terminal, ladder, route, terminal_condition,
};
pub use state::{Contribution, Delta, LoopState};

// The loop body: the arms a pass fans out to, and the closed set of steps their
// nodes are. `arm` owns the one list both arm edge sets are derived from and the
// merge that folds them; `step` owns the single tool a node body is.
pub use arm::{Arm, ArmOutcome, ArmSet, Edge, upstream_address};
pub use step::{
    AccumulatorAccess, Advanced, CanWrite, NoWrite, Observer, RUN_LOOP_STEP, RegisteredStep,
    STEP_ATTEMPT, STEP_JUDGE, STEP_NAMES, STEP_PASS, STEP_PLAN, STEP_REFLECT, STEP_REPORT,
    STEP_RESEARCH, Step, StepContext, StepRegistry, run_loop_step,
};

// The wire contract, re-exported by module rather than by item so every path
// through this crate resolves to the same definitions the contract crate
// publishes. A host may depend on `tinyloops-bus` directly and get exactly these
// types; nothing here redefines them.
pub use tinyloops_bus;
pub use tinyloops_bus::{
    CONTRACT_VERSION, GreetRequest, GreetResponse, INTERFACE, METHODS, OBJECT_PATH, is_compatible,
    names, version,
};
