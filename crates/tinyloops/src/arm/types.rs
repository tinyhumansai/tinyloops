//! The evaluation arm, what it returns, and the one list every arm edge is
//! derived from.
//!
//! An arm is a *declaration* — a name, and whether it may end the run — paired
//! with the body that evaluates it. The declaration is what the graph builder
//! reads; the body is a step like any other, invoked through
//! [`RUN_LOOP_STEP`](crate::RUN_LOOP_STEP) under the arm's own name, which is
//! why the two cannot drift apart into different spellings.

use std::sync::Arc;

use serde_json::Value;

use crate::state::{Contribution, LoopState};
use crate::step::{NoWrite, StepContext};
use crate::{Error, Result};

/// What one arm returns from a pass.
///
/// A pair rather than an associated output type: an [`ArmSet`] holds its arms
/// as `Arc<dyn Arm>`, and an associated type is not object-safe. The pair also
/// says the thing plainly — the two halves merge under two different laws, so
/// they are two fields rather than one blob:
///
/// - `state` is the whole accumulator this arm would have produced from the
///   shared base. The merge takes its [`LoopState::delta_from`] and sums it, so
///   a reset from one arm and an increment from another compose instead of
///   racing.
/// - `contribution` is the narrative this arm owns — its lesson, its steer, its
///   score. Those merge by *exclusive ownership*: two arms writing the same
///   field is refused rather than resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArmOutcome {
    /// The whole accumulator this arm computed from the shared base.
    pub state: LoopState,
    /// The narrative fields this arm claims, filed under its own name.
    pub contribution: Contribution,
}

impl ArmOutcome {
    /// An outcome that moves nothing, from `arm`.
    ///
    /// The starting point an arm body mutates, so adding a field to
    /// [`Contribution`] does not break every arm in the crate.
    #[must_use]
    pub fn unchanged(arm: &'static str, base: &LoopState) -> Self {
        Self {
            state: base.clone(),
            contribution: Contribution::new(arm),
        }
    }

    /// The arm that produced this outcome.
    #[must_use]
    pub fn arm(&self) -> &'static str {
        self.contribution.arm
    }
}

/// One evaluation arm of the loop body.
///
/// Every arm is fanned out from `attempt`, converges on the merge barrier, and
/// is folded there. All three come from one declaration — see [`ArmSet`].
///
/// # Examples
///
/// ```
/// # use serde_json::Value;
/// # use tinyloops::{Arm, ArmOutcome, LoopState, NoWrite, Result, StepContext};
/// struct Judge;
///
/// impl Arm for Judge {
///     fn name(&self) -> &'static str {
///         "judge"
///     }
///
///     fn evaluate(
///         &self,
///         base: &LoopState,
///         report: &Value,
///         _ctx: StepContext<'_, NoWrite>,
///     ) -> Result<ArmOutcome> {
///         let mut outcome = ArmOutcome::unchanged(self.name(), base);
///         let score = report.get("score").and_then(Value::as_u64);
///         outcome.contribution.score = score.and_then(|score| u8::try_from(score).ok());
///         Ok(outcome)
///     }
/// }
///
/// assert!(!Judge.may_conclude());
/// ```
pub trait Arm: Send + Sync {
    /// The arm's name.
    ///
    /// It is the node id, the step name the node passes to
    /// [`RUN_LOOP_STEP`](crate::RUN_LOOP_STEP), and the key its contribution is
    /// filed under. One name for all three, declared rather than derived from
    /// position, so adding an arm renumbers nothing and a checkpoint taken
    /// before the addition still names the same nodes.
    fn name(&self) -> &'static str;

    /// Whether this arm may end the run.
    ///
    /// Exactly one arm — the reflection — may. See [`ArmSet::new`] for why a
    /// second one is refused at construction.
    fn may_conclude(&self) -> bool {
        false
    }

    /// Whether this arm may propose an amendment to the run's own profile.
    ///
    /// `false` for every arm but [`TunerArm`], and an implementor outside this
    /// crate answering `true` gains nothing by it: the slot a proposal travels
    /// in is crate-private, so the claim is unbacked. It is declared here so
    /// [`ArmSet::new`] can refuse a second proposer by the same route it
    /// already refuses a second concluding arm.
    fn may_tune(&self) -> bool {
        false
    }

    /// Evaluates the pass.
    ///
    /// `report` is the output of the node immediately upstream — the attempt's
    /// report — and never the loop head's accumulator. That is invariant 3, and
    /// the reason it is in the signature rather than in a comment: the head
    /// folds at the *top* of a pass, so mid-body the accumulator is one pass
    /// behind, and an arm reading it routes on a stale answer. The `ctx` is a
    /// [`NoWrite`] context, which has no accessor for that slot at all.
    ///
    /// `base` is the accumulator every arm in this superstep is handed, so each
    /// arm's delta is computed against the same starting point.
    ///
    /// # Errors
    ///
    /// Returns whatever [`Error`] the arm's body raises. An arm that cannot
    /// evaluate must say so: a merge that folds a silently unchanged arm is a
    /// route taken on evidence nobody gathered.
    fn evaluate(
        &self,
        base: &LoopState,
        report: &Value,
        ctx: StepContext<'_, NoWrite>,
    ) -> Result<ArmOutcome>;
}

/// The declared arms of one loop, in a stable order.
///
/// The single source of the two facts invariant 6 requires never drift apart:
/// the fan-out from `attempt` to each arm, and the merge barrier's fold over
/// each arm's output. There is deliberately **no** constructor taking two
/// lists. "Every arm converges" and "every arm is folded" are one fact because
/// there is one place to say it; as two facts they drift, and the drift is
/// silent — an arm added to the fan-out but not to the fold runs, costs its
/// budget, and changes nothing.
#[derive(Clone)]
pub struct ArmSet {
    arms: Vec<Arc<dyn Arm>>,
}

impl ArmSet {
    /// Declares the arms of one loop.
    ///
    /// # Errors
    ///
    /// - [`Error::EmptyArmSet`] when `arms` is empty. A loop with nothing
    ///   evaluating it has no evidence to route on and no arm able to conclude,
    ///   so it can only run to its iteration cap.
    /// - [`Error::DuplicateArm`] when two arms share a name. The name is a node
    ///   id, a step name, and a fold key at once; two arms answering to it means
    ///   one of them silently replaces the other in all three.
    /// - [`Error::AmbiguousConclusion`] when more than one arm returns `true`
    ///   from [`Arm::may_conclude`]. Two arms able to end a run means the
    ///   outcome depends on which finished first — the arrival-order dependence
    ///   every other rule here exists to remove, arriving through the one door
    ///   left open.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use serde_json::Value;
    /// # use tinyloops::{Arm, ArmOutcome, ArmSet, Error, LoopState, NoWrite, Result, StepContext};
    /// struct Named(&'static str, bool);
    ///
    /// impl Arm for Named {
    ///     fn name(&self) -> &'static str {
    ///         self.0
    ///     }
    ///
    ///     fn may_conclude(&self) -> bool {
    ///         self.1
    ///     }
    ///
    ///     fn evaluate(
    ///         &self,
    ///         base: &LoopState,
    ///         _report: &Value,
    ///         _ctx: StepContext<'_, NoWrite>,
    ///     ) -> Result<ArmOutcome> {
    ///         Ok(ArmOutcome::unchanged(self.name(), base))
    ///     }
    /// }
    ///
    /// let set = ArmSet::new(vec![
    ///     Arc::new(Named("reflect", true)) as Arc<dyn Arm>,
    ///     Arc::new(Named("judge", false)),
    /// ])?;
    /// assert_eq!(set.names(), ["reflect", "judge"]);
    /// assert_eq!(set.concluding(), Some("reflect"));
    ///
    /// assert_eq!(ArmSet::new(vec![]).unwrap_err(), Error::EmptyArmSet);
    /// # Ok::<(), tinyloops::Error>(())
    /// ```
    pub fn new(arms: Vec<Arc<dyn Arm>>) -> Result<Self> {
        if arms.is_empty() {
            return Err(Error::EmptyArmSet);
        }

        let mut concluding: Option<&'static str> = None;
        let mut tuning: Option<&'static str> = None;
        for (index, arm) in arms.iter().enumerate() {
            if arms[..index].iter().any(|prior| prior.name() == arm.name()) {
                return Err(Error::DuplicateArm {
                    name: arm.name().to_string(),
                });
            }

            if arm.may_conclude() {
                if let Some(first) = concluding {
                    return Err(Error::AmbiguousConclusion {
                        first,
                        second: arm.name(),
                    });
                }
                concluding = Some(arm.name());
            }

            if arm.may_tune() {
                if let Some(first) = tuning {
                    return Err(Error::AmbiguousTuning {
                        first,
                        second: arm.name(),
                    });
                }
                tuning = Some(arm.name());
            }
        }

        Ok(Self { arms })
    }

    /// Every arm's name, in declaration order.
    ///
    /// Declaration order rather than sorted order: it is what a reader of the
    /// rendered graph sees, and the fold does not depend on it.
    #[must_use]
    pub fn names(&self) -> Vec<&'static str> {
        self.arms.iter().map(|arm| arm.name()).collect()
    }

    /// The declared arms.
    #[must_use]
    pub fn arms(&self) -> &[Arc<dyn Arm>] {
        &self.arms
    }

    /// How many arms are declared.
    #[must_use]
    pub fn len(&self) -> usize {
        self.arms.len()
    }

    /// Always `false`: an empty set cannot be constructed.
    ///
    /// Present because clippy asks for it beside [`Self::len`], and answered
    /// honestly rather than removed: [`Self::new`] rejects an empty list, so
    /// there is no `ArmSet` for which this is `true`.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.arms.is_empty()
    }

    /// The arm allowed to end the run, if one was declared.
    #[must_use]
    pub fn concluding(&self) -> Option<&'static str> {
        self.arms
            .iter()
            .find(|arm| arm.may_conclude())
            .map(|arm| arm.name())
    }

    /// The arm allowed to propose an amendment, if one was declared.
    #[must_use]
    pub fn tuning(&self) -> Option<&'static str> {
        self.arms
            .iter()
            .find(|arm| arm.may_tune())
            .map(|arm| arm.name())
    }
}

/// What proposes a change to the run's own configuration.
///
/// A trait of its own rather than a capability on [`Arm`], and the reason is
/// mechanical: `Arm::evaluate` takes a concrete `StepContext<'_, NoWrite>` and
/// an [`ArmSet`] holds `Arc<dyn Arm>`, so making the context generic over a
/// third capability marker would cost object safety. Wrapping a `Tuner` in
/// [`TunerArm`] buys the same guarantee for no change to the arm surface: the
/// adapter is the only code that can fill the crate-private slot a proposal
/// travels in, so an `impl Arm` has no way to propose one.
///
/// # What a tuner should and should not be
///
/// The shipped one is a pure function of the counters. A model asked mid-run
/// whether its own configuration is wrong has no ground truth to answer from
/// and every incentive to answer yes — the same pressure that makes a model
/// claim the goal is met on the eighth pass. A model tuner is permitted here,
/// and is bounded by exactly the same [`Bounds`](crate::Bounds), which is the
/// point of putting the bounds outside the proposer.
pub trait Tuner: Send + Sync {
    /// The arm's name, and the id of its node.
    fn name(&self) -> &'static str;

    /// Proposes at most one amendment for the *next* pass.
    ///
    /// `base` is the accumulator every arm in this superstep was handed, and
    /// `report` is the attempt's report — the same two inputs every other arm
    /// reads, for the same reason.
    ///
    /// Returning `None` is the ordinary answer. A tuner that proposes on every
    /// pass is a tuner that has mistaken its own budget for a target.
    ///
    /// # Errors
    ///
    /// Whatever the implementation raises. A tuner that cannot decide should
    /// return `Ok(None)` rather than an error: failing the pass over a
    /// configuration question is a worse outcome than not tuning.
    fn propose(
        &self,
        base: &LoopState,
        report: &Value,
        ctx: StepContext<'_, NoWrite>,
    ) -> Result<Option<Amendment>>;
}

/// The adapter that runs a [`Tuner`] as an evaluation arm.
///
/// The only writer of the proposal slot in the whole crate. Everything else
/// about it is an ordinary arm: it fans out from the attempt, converges on the
/// barrier, and folds as a zero delta with one narrative claim.
pub struct TunerArm {
    tuner: Arc<dyn Tuner>,
}

impl std::fmt::Debug for TunerArm {
    /// Renders the tuner's name; the body is a trait object.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TunerArm")
            .field("tuner", &self.tuner.name())
            .finish()
    }
}

impl TunerArm {
    /// Wraps `tuner` as an arm.
    #[must_use]
    pub fn new(tuner: Arc<dyn Tuner>) -> Self {
        Self { tuner }
    }
}

impl Arm for TunerArm {
    fn name(&self) -> &'static str {
        self.tuner.name()
    }

    fn may_tune(&self) -> bool {
        true
    }

    fn evaluate(
        &self,
        base: &LoopState,
        report: &Value,
        ctx: StepContext<'_, NoWrite>,
    ) -> Result<ArmOutcome> {
        let mut outcome = ArmOutcome::unchanged(Arm::name(self), base);
        if let Some(amendment) = self.tuner.propose(base, report, ctx)? {
            outcome.contribution.amendment = Some(amendment.clone());
            outcome.state.proposed = Some(amendment);
        }
        Ok(outcome)
    }
}

impl std::fmt::Debug for ArmSet {
    /// Renders the declared names.
    ///
    /// Hand-written because an arm is a trait object, and the list of names is
    /// the whole content of the declaration.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArmSet")
            .field("arms", &self.names())
            .field("concluding", &self.concluding())
            .finish()
    }
}

/// One directed edge of the emitted graph.
///
/// The kernel's own edge type rather than the engine's: an [`ArmSet`] answers
/// what connects to what, and it should answer that without every caller of
/// [`ArmSet::names`] taking a dependency on a graph model.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Edge {
    /// The node the edge leaves.
    pub from: String,
    /// The node the edge enters.
    pub to: String,
}

impl Edge {
    /// Builds an edge from `from` to `to`.
    #[must_use]
    pub fn new(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
        }
    }
}
