//! The closed set of loop steps, and the single tool a node body is.
//!
//! The kernel's thesis is that **the graph owns routing and Rust owns the
//! steps**. Every branch a run can take is declared in the emitted graph, where
//! it can be read, rendered, and diffed; everything a node *does* is a Rust
//! step, compiled and tested against this crate's own types. This module is the
//! second half of that sentence.
//!
//! # One tool, not one agent per node
//!
//! A node body is a `NodeKind::ToolCall` naming [`RUN_LOOP_STEP`] and passing
//! the step's name as an argument, never a bare `agent_ref`. Three things a
//! loop needs are lost by naming an agent and nothing else: the operator
//! directives an external control surface posts into a mailbox and `attempt`
//! drains, the salvage of an attempt its own cap killed, and the arms opened
//! beside the loop at a named node a checkpoint can land on. See
//! `docs/specs/loop-kernel.md`.
//!
//! # The set is closed
//!
//! [`StepRegistry::get`] on an unregistered name is an [`Error::UnknownStep`],
//! never a no-op. A graph naming a step that does not exist would otherwise run
//! green, change nothing, and route on a state nobody advanced — the same class
//! of failure `tinyflows`' own `assert_no_null_bindings` exists to catch,
//! arriving one layer too late for it. The registry is also the reason
//! [`STEP_NAMES`] is a constant: a graph builder and a registry that spell a
//! step differently disagree silently, and the only fix that holds is for both
//! to read the same list.
//!
//! # State crosses as JSON and comes back whole
//!
//! [`run_loop_step`] decodes `{ "step": <name>, "state": <accumulator> }`, runs
//! the named body, and returns the *entire* accumulator as JSON. The loop head
//! then replaces its slot with what came back, which is what keeps the head the
//! accumulator's sole writer (invariant 1) and what makes the fold an
//! assignment rather than an increment (invariant 4). A step that returned a
//! patch would put the head in the business of merging, and a replayed
//! activation would apply that patch twice.
//!
//! # Synchronous, for now
//!
//! [`Step::run`] and [`Observer::observe`] are synchronous. Wiring this
//! registry into `tinyflows::caps::ToolInvoker` — whose `invoke` is an
//! `async fn` behind `#[async_trait]` — needs the `async_trait` macro, which
//! this crate does not depend on. That adapter is a few lines and belongs with
//! the harness seam that introduces the dependency; nothing here is blocked by
//! its absence, because the decision a step makes is pure and the effects it
//! reaches for cross a capability trait the embedder implements.

mod types;

pub use types::{
    AccumulatorAccess, Advanced, CanWrite, NoWrite, Observer, RegisteredStep, Step, StepContext,
};

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::Value;

use crate::state::LoopState;
use crate::{Error, Result};

/// The slug of the one tool every node body is invoked through.
pub const RUN_LOOP_STEP: &str = "run_loop_step";

/// The step that decomposes the goal, before the loop and on each re-plan.
pub const STEP_PLAN: &str = "plan";
/// The step that gathers context once, before the first attempt.
pub const STEP_RESEARCH: &str = "research";
/// The step that attempts the goal, and drains the operator mailbox.
pub const STEP_ATTEMPT: &str = "attempt";
/// The reflection arm: the one arm allowed to conclude a run.
pub const STEP_REFLECT: &str = "reflect";
/// The judging arm: it scores a pass and never ends the run.
pub const STEP_JUDGE: &str = "judge";
/// The single node every route enters, and the only one closing the cycle.
pub const STEP_PASS: &str = "pass";
/// The step that writes the run's report, after `stand_down`.
pub const STEP_REPORT: &str = "report";

/// Every step name the kernel emits, in the order the loop reaches them.
///
/// Declared once so a graph builder and a [`StepRegistry`] cannot disagree
/// about a name. They fail differently and both failures are quiet: a builder
/// with a name the registry lacks produces a run that errors at the first
/// invocation, and a registry with a name no node calls produces a body that is
/// tested, maintained, and never run.
pub const STEP_NAMES: [&str; 7] = [
    STEP_PLAN,
    STEP_RESEARCH,
    STEP_ATTEMPT,
    STEP_REFLECT,
    STEP_JUDGE,
    STEP_PASS,
    STEP_REPORT,
];

/// The closed set of step bodies, keyed by name.
///
/// # Examples
///
/// ```
/// # use std::sync::Arc;
/// # use tinyloops::{Advanced, CanWrite, Error, LoopState, Result, Step, StepContext, StepRegistry};
/// struct Bank;
///
/// impl Step for Bank {
///     fn name(&self) -> &'static str {
///         "bank"
///     }
///
///     fn run(&self, mut state: LoopState, ctx: StepContext<'_, CanWrite>) -> Result<Advanced> {
///         state.banked = 1;
///         Ok(ctx.advance(state))
///     }
/// }
///
/// let mut registry = StepRegistry::new();
/// registry.register(Arc::new(Bank))?;
///
/// assert!(registry.get("bank").is_ok());
/// assert_eq!(
///     registry.get("bnak").unwrap_err(),
///     Error::UnknownStep { name: "bnak".to_string() },
/// );
/// # Ok::<(), tinyloops::Error>(())
/// ```
#[derive(Default, Clone)]
pub struct StepRegistry {
    /// A `BTreeMap` rather than a `HashMap`: [`Self::names`] feeds the graph
    /// builder, and the builder has to emit the same bytes for the same inputs
    /// so a checkpoint's graph signature is stable. Hash order is not.
    steps: BTreeMap<&'static str, RegisteredStep>,
}

impl StepRegistry {
    /// Builds an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a step that advances the accumulator.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DuplicateStep`] when the name is already registered. A
    /// name in a closed set has exactly one meaning; silently replacing a body
    /// makes which one runs depend on registration order.
    pub fn register(&mut self, step: Arc<dyn Step>) -> Result<()> {
        self.insert(RegisteredStep::Advancing(step))
    }

    /// Registers a step that may only look at the accumulator.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DuplicateStep`] for the same reason as
    /// [`Self::register`].
    pub fn register_observer(&mut self, observer: Arc<dyn Observer>) -> Result<()> {
        self.insert(RegisteredStep::Observing(observer))
    }

    fn insert(&mut self, entry: RegisteredStep) -> Result<()> {
        let name = entry.name();
        if self.steps.contains_key(name) {
            return Err(Error::DuplicateStep {
                name: name.to_string(),
            });
        }

        self.steps.insert(name, entry);
        Ok(())
    }

    /// Resolves `name` to its body.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnknownStep`] when nothing is registered under `name`.
    /// This is the closed set: an unrecognised name is an error and never a
    /// no-op, because a no-op leaves the run to route on a state nobody
    /// advanced.
    pub fn get(&self, name: &str) -> Result<&RegisteredStep> {
        self.steps.get(name).ok_or_else(|| Error::UnknownStep {
            name: name.to_string(),
        })
    }

    /// Every registered name, in a stable order.
    #[must_use]
    pub fn names(&self) -> Vec<&'static str> {
        self.steps.keys().copied().collect()
    }

    /// How many bodies are registered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// Whether nothing is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Runs the body registered under `name` over `state`.
    ///
    /// The pass number handed to the body is `state.passes`, so the context and
    /// the accumulator cannot disagree about which pass is running.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnknownStep`] when `name` is not registered, or
    /// whatever error the body raises.
    pub fn run(&self, name: &str, state: LoopState) -> Result<LoopState> {
        let pass = state.passes;
        self.get(name)?.run(state, pass)
    }

    /// Runs the body registered under `name`, handing it `args`.
    ///
    /// The graph path. A body reached through a node sees the arguments the
    /// node was addressed with, which is how a barrier reads inputs a single
    /// accumulator cannot carry.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnknownStep`] when `name` is not registered, or
    /// whatever error the body raises.
    pub fn run_with(&self, name: &str, state: LoopState, args: &Value) -> Result<LoopState> {
        let pass = state.passes;
        self.get(name)?.run_with(state, pass, args)
    }
}

impl std::fmt::Debug for StepRegistry {
    /// Renders the registered names.
    ///
    /// Hand-written because a body is a trait object; the names are what a
    /// diagnostic about a closed set is actually about.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StepRegistry")
            .field("steps", &self.names())
            .finish()
    }
}

/// Runs one step for a `run_loop_step` tool call and returns the whole
/// accumulator.
///
/// `args` is `{ "step": <name>, "state": <accumulator> }`. The returned value
/// is the entire [`LoopState`], serialized: the loop head replaces its slot
/// with it, so a step never merges and the head stays the sole writer.
///
/// # Examples
///
/// ```
/// # use std::sync::Arc;
/// # use serde_json::json;
/// # use tinyloops::{
/// #     Advanced, CanWrite, LoopState, Result, Step, StepContext, StepRegistry, run_loop_step,
/// # };
/// struct Solve;
///
/// impl Step for Solve {
///     fn name(&self) -> &'static str {
///         "solve"
///     }
///
///     fn run(&self, mut state: LoopState, ctx: StepContext<'_, CanWrite>) -> Result<Advanced> {
///         state.solved = true;
///         Ok(ctx.advance(state))
///     }
/// }
///
/// let mut registry = StepRegistry::new();
/// registry.register(Arc::new(Solve))?;
///
/// let args = json!({ "step": "solve", "state": LoopState::new("goal") });
/// let returned = run_loop_step(&registry, &args)?;
///
/// assert_eq!(returned["solved"], json!(true));
/// assert_eq!(returned["goal"], json!("goal")); // the *whole* state came back
/// # Ok::<(), tinyloops::Error>(())
/// ```
///
/// # Errors
///
/// - [`Error::MalformedStepPayload`] when `step` is absent or is not a string,
///   or when `state` is absent or does not decode as an accumulator. A payload
///   the tool cannot read must not fall through to a default step or a default
///   state: both would route the run on something nobody computed.
/// - [`Error::UnknownStep`] when `step` names a body the registry does not
///   hold.
/// - [`Error::StateEncoding`] when the returned accumulator cannot be
///   serialized.
/// - Whatever error the body itself raises.
pub fn run_loop_step(registry: &StepRegistry, args: &Value) -> Result<Value> {
    let name = args
        .get("step")
        .and_then(Value::as_str)
        .ok_or(Error::MalformedStepPayload { field: "step" })?;

    let state = types::decode_state(args)?;
    let returned = registry.run_with(name, state, args)?;

    serde_json::to_value(returned).map_err(|_| Error::StateEncoding)
}

#[cfg(test)]
mod test;
