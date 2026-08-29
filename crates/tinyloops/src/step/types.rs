//! The capability-typed context a step runs in, and the two traits a step body
//! may implement.
//!
//! These types are the substance of the module, so they live here rather than
//! in the module root: everything in `mod.rs` is registration, dispatch, and
//! the one tool entry point built on top of them.
//!
//! # The capability parameter
//!
//! [`StepContext`] is generic over an [`AccumulatorAccess`] marker, and the
//! marker decides which methods exist. A [`CanWrite`] context can mint an
//! [`Advanced`]; a [`NoWrite`] context cannot, and there is no other
//! constructor. That is invariant 11 of `docs/specs/loop-kernel.md` written as
//! a type rather than as a review comment: "this arm wrote the accumulator" is
//! a missing method, not a failed assertion.
//!
//! The marker trait is deliberately not sealed. Implementing it buys an outside
//! crate a `StepContext<Mine>` with the read-only accessors and nothing else,
//! because [`Advanced`]'s field is private and [`StepContext::advance`] is
//! declared only in the [`CanWrite`] block.

use std::marker::PhantomData;

use crate::policy::Thresholds;
use crate::state::LoopState;
use crate::{Error, Result};

/// Marks whether a [`StepContext`] may advance the accumulator.
///
/// Implemented by [`CanWrite`] and [`NoWrite`]. It carries no methods: the
/// capability is expressed by which inherent `impl` block of [`StepContext`]
/// applies, not by anything this trait provides.
pub trait AccumulatorAccess: Send + Sync + 'static {}

/// The capability of a step that advances the run.
///
/// Handed to the node bodies the loop head folds from: `plan`, `research`,
/// `attempt`, `pass`, and `report`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanWrite;

/// The capability of a step that may only look at the run.
///
/// Handed to every observing body. A run's arms are evaluated through steps
/// that *do* return a state — the head folds their deltas — so `NoWrite` is for
/// the bodies that exist to report, meter, or trace, and whose contribution to
/// the accumulator must be nothing at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoWrite;

impl AccumulatorAccess for CanWrite {}
impl AccumulatorAccess for NoWrite {}

/// What a step is given that is not the accumulator.
///
/// Deliberately small. Everything a routing decision reads is a field of
/// [`LoopState`], which arrives as the step's argument, so this carries only
/// the two things a body cannot derive from it: which pass is running, and the
/// [`Thresholds`] in force for this run.
///
/// # Examples
///
/// ```
/// # use tinyloops::{StepContext, Thresholds};
/// let thresholds = Thresholds::default();
/// let ctx = StepContext::advancing(2, &thresholds);
///
/// assert_eq!(ctx.pass(), 2);
/// assert_eq!(ctx.thresholds().stuck, 2);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct StepContext<'a, A: AccumulatorAccess> {
    pass: u32,
    thresholds: &'a Thresholds,
    args: &'a serde_json::Value,
    access: PhantomData<A>,
}

/// The arguments a step built without any is handed.
///
/// A step that reads an argument it was not given sees `null`, which is the
/// same thing it would see from an expression that failed to resolve. That
/// symmetry is deliberate: there is one absent-argument case for a body to
/// handle, not two.
static NO_ARGS: serde_json::Value = serde_json::Value::Null;

impl<'a, A: AccumulatorAccess> StepContext<'a, A> {
    /// Which pass round the loop is running, counted from zero.
    #[must_use]
    pub fn pass(&self) -> u32 {
        self.pass
    }

    /// The thresholds this run is configured with.
    #[must_use]
    pub fn thresholds(&self) -> &'a Thresholds {
        self.thresholds
    }

    /// The whole argument object the node was invoked with.
    ///
    /// Most steps never touch it: a node body takes the accumulator and returns
    /// one, and that is the entire contract. Two do not fit that shape, and
    /// both are barriers rather than transformations — the merge is handed
    /// every arm's output, and a gate is handed what it is gating. Their inputs
    /// are addressed in the graph and arrive here rather than in the state,
    /// because the state is one value and a barrier reads several.
    ///
    /// It is `null` for a step invoked without arguments, which is what
    /// [`StepContext::advancing`] and [`StepContext::observing`] build.
    #[must_use]
    pub fn args(&self) -> &'a serde_json::Value {
        self.args
    }

    /// One named argument, or `None` when the node was not given it.
    ///
    /// Returns `None` for an argument that is present and `null` as well as for
    /// one that is absent. Under this engine an expression that failed to
    /// compile, failed to run, produced no output, or named a key nothing
    /// writes *all* resolve to `null`, so a body cannot tell those apart and
    /// should not pretend to: a null argument is a missing argument.
    #[must_use]
    pub fn arg(&self, name: &str) -> Option<&'a serde_json::Value> {
        self.args.get(name).filter(|value| !value.is_null())
    }
}

impl<'a> StepContext<'a, CanWrite> {
    /// Builds the context handed to a step that advances the run.
    #[must_use]
    pub fn advancing(pass: u32, thresholds: &'a Thresholds) -> Self {
        Self::advancing_with(pass, thresholds, &NO_ARGS)
    }

    /// Builds the context handed to a step that advances the run, carrying the
    /// arguments its node was invoked with.
    ///
    /// This is what [`run_loop_step`](crate::run_loop_step) uses, so a body
    /// reached through the graph sees everything the node was addressed with.
    #[must_use]
    pub fn advancing_with(
        pass: u32,
        thresholds: &'a Thresholds,
        args: &'a serde_json::Value,
    ) -> Self {
        Self {
            pass,
            thresholds,
            args,
            access: PhantomData,
        }
    }

    /// Wraps the accumulator this step advanced to.
    ///
    /// The only constructor of [`Advanced`], and it exists only here. A
    /// [`NoWrite`] context has no such method, so a body holding one cannot
    /// produce the value its signature would need to return.
    #[must_use]
    pub fn advance(&self, state: LoopState) -> Advanced {
        Advanced(state)
    }
}

impl<'a> StepContext<'a, NoWrite> {
    /// Builds the context handed to a step that may only look.
    #[must_use]
    pub fn observing(pass: u32, thresholds: &'a Thresholds) -> Self {
        Self::observing_with(pass, thresholds, &NO_ARGS)
    }

    /// Builds the observing context, carrying the node's arguments.
    #[must_use]
    pub fn observing_with(
        pass: u32,
        thresholds: &'a Thresholds,
        args: &'a serde_json::Value,
    ) -> Self {
        Self {
            pass,
            thresholds,
            args,
            access: PhantomData,
        }
    }
}

/// An accumulator a step was entitled to advance to.
///
/// Returned by [`Step::run`] and unwrapped by the loop head, which is the
/// accumulator's sole writer (invariant 1). The inner value is private and
/// [`StepContext::advance`] is its only constructor, so possessing one is proof
/// that a [`CanWrite`] context was in scope where it was built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advanced(LoopState);

impl Advanced {
    /// Borrows the advanced accumulator.
    #[must_use]
    pub fn state(&self) -> &LoopState {
        &self.0
    }

    /// Takes the advanced accumulator.
    #[must_use]
    pub fn into_state(self) -> LoopState {
        self.0
    }
}

/// A node body that advances the run.
///
/// A step takes the whole accumulator and returns the whole accumulator, never
/// a patch. That is what keeps the loop head the accumulator's only writer: the
/// head replaces its slot with what came back, so a replayed activation applies
/// an assignment twice rather than an increment twice (invariant 4).
///
/// # Examples
///
/// ```
/// # use tinyloops::{Advanced, LoopState, Result, Step, StepContext, CanWrite, Thresholds};
/// struct CountAttempt;
///
/// impl Step for CountAttempt {
///     fn name(&self) -> &'static str {
///         "attempt"
///     }
///
///     fn run(&self, mut state: LoopState, ctx: StepContext<'_, CanWrite>) -> Result<Advanced> {
///         state.attempts = ctx.pass() + 1; // an assignment, never `+= 1`
///         Ok(ctx.advance(state))
///     }
/// }
/// ```
pub trait Step: Send + Sync {
    /// The name this step is registered and invoked under.
    ///
    /// It is also the node id and the `step` argument a graph node passes to
    /// [`RUN_LOOP_STEP`](super::RUN_LOOP_STEP), so the graph and the registry
    /// agree by construction rather than by convention.
    fn name(&self) -> &'static str;

    /// Runs the step over `state`.
    ///
    /// The returned [`Advanced`] can only have come from
    /// [`StepContext::advance`], so the signature itself records that this body
    /// was handed the capability to write.
    ///
    /// # Errors
    ///
    /// Returns whatever [`Error`] the body raises. A step that cannot do its
    /// work must return one rather than hand back an unchanged state: a route
    /// taken on a state nobody advanced is the silent failure the closed step
    /// set exists to prevent.
    fn run(&self, state: LoopState, ctx: StepContext<'_, CanWrite>) -> Result<Advanced>;
}

/// A node body that may only look at the run.
///
/// The output type is `()`. Together with the missing
/// [`StepContext::advance`] on a [`NoWrite`] context, that is invariant 11: an
/// observing body has neither a way to build an [`Advanced`] nor anywhere to
/// put one.
///
/// The two compile errors this buys, in the order a body hits them:
///
/// ```compile_fail,E0599
/// # use tinyloops::{LoopState, NoWrite, Observer, Result, StepContext};
/// struct Sneaky;
///
/// impl Observer for Sneaky {
///     fn name(&self) -> &'static str {
///         "sneaky"
///     }
///
///     fn observe(&self, state: &LoopState, ctx: StepContext<'_, NoWrite>) -> Result<()> {
///         // error[E0599]: no method named `advance` found for struct
///         // `StepContext<'_, NoWrite>`
///         let _ = ctx.advance(state.clone());
///         Ok(())
///     }
/// }
/// ```
///
/// ```compile_fail,E0308
/// # use tinyloops::{LoopState, NoWrite, Observer, Result, StepContext};
/// struct AlsoSneaky;
///
/// impl Observer for AlsoSneaky {
///     fn name(&self) -> &'static str {
///         "also_sneaky"
///     }
///
///     fn observe(&self, state: &LoopState, _ctx: StepContext<'_, NoWrite>) -> Result<()> {
///         // error[E0308]: expected `()`, found `LoopState`
///         Ok(state.clone())
///     }
/// }
/// ```
pub trait Observer: Send + Sync {
    /// The name this observer is registered and invoked under.
    fn name(&self) -> &'static str;

    /// Looks at `state`.
    ///
    /// # Errors
    ///
    /// Returns whatever [`Error`] the body raises. An observer that fails is
    /// still a node error: the loop must not route on a pass whose metering,
    /// reporting, or tracing did not happen.
    fn observe(&self, state: &LoopState, ctx: StepContext<'_, NoWrite>) -> Result<()>;
}

/// One entry of the closed step set.
///
/// Both kinds are registered by name in the same [`StepRegistry`](super::StepRegistry)
/// so the set is closed over both: a graph naming an observer where it meant a
/// step gets the observer, not a no-op.
#[derive(Clone)]
pub enum RegisteredStep {
    /// A body that advances the accumulator.
    Advancing(std::sync::Arc<dyn Step>),
    /// A body that may only look at it.
    Observing(std::sync::Arc<dyn Observer>),
}

impl RegisteredStep {
    /// The name this entry is registered under.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Advancing(step) => step.name(),
            Self::Observing(observer) => observer.name(),
        }
    }

    /// Whether this entry may advance the accumulator.
    #[must_use]
    pub fn advances(&self) -> bool {
        matches!(self, Self::Advancing(_))
    }

    /// Runs this entry over `state`, returning the state the pass ends on.
    ///
    /// An observing entry returns `state` unchanged. The wrapping happens here,
    /// once, rather than in every observer: a body that cannot express a change
    /// should not have to restate that it made none.
    ///
    /// # Errors
    ///
    /// Returns whatever [`Error`] the body raises.
    pub fn run(&self, state: LoopState, pass: u32) -> Result<LoopState> {
        self.run_with(state, pass, &NO_ARGS)
    }

    /// Runs the body, handing it the arguments its node was invoked with.
    ///
    /// # Errors
    ///
    /// Returns whatever [`Error`] the body raises.
    pub fn run_with(
        &self,
        state: LoopState,
        pass: u32,
        args: &serde_json::Value,
    ) -> Result<LoopState> {
        // Copied out before `state` moves, and taken from the state itself
        // rather than from a caller: a body handed thresholds the run is not
        // using would route on numbers nobody configured, and nothing would
        // report it.
        let thresholds = state.profile.thresholds;
        match self {
            Self::Advancing(step) => step
                .run(state, StepContext::advancing_with(pass, &thresholds, args))
                .map(Advanced::into_state),
            Self::Observing(observer) => {
                observer.observe(&state, StepContext::observing_with(pass, &thresholds, args))?;
                Ok(state)
            }
        }
    }
}

impl std::fmt::Debug for RegisteredStep {
    /// Renders the entry's name and kind.
    ///
    /// Hand-written because a step body is a trait object and cannot be
    /// derived, and because the name is the only part worth reading in a
    /// diagnostic.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegisteredStep")
            .field("name", &self.name())
            .field("advances", &self.advances())
            .finish()
    }
}

/// Reads a [`LoopState`] out of the `state` field of a tool payload.
///
/// # Errors
///
/// Returns [`Error::MalformedStepPayload`] when the field is absent or does not
/// decode as an accumulator.
pub(super) fn decode_state(args: &serde_json::Value) -> Result<LoopState> {
    let state = args
        .get("state")
        .ok_or(Error::MalformedStepPayload { field: "state" })?;

    serde_json::from_value(state.clone())
        .map_err(|_| Error::MalformedStepPayload { field: "state" })
}
