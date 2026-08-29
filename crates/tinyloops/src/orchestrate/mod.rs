//! What drives a goal run: the role bound to `plan`, `attempt`, and `report`.
//!
//! The loop kernel says where a run goes. Something still has to decide *what
//! to attempt*, and left to one general-purpose agent with a full toolbox that
//! decision collapses predictably: a driver that can run the experiment runs it
//! instead of commissioning it, spends the pass doing one specialist's work
//! badly, and writes an attempt report about the tool call it made rather than
//! about the goal. The run then routes on that report, and the loop's whole
//! routing apparatus is deciding about the wrong thing.
//!
//! That is not a prompting failure and it does not have a prompting fix. **A
//! prompt instruction is not a control**: "delegate, do not execute" is
//! followed right up until executing is the locally cheaper path. So every
//! constraint here is a *registration-time* fact instead.
//!
//! # What is in the module
//!
//! - [`Orchestrator`] is the registration: a tool grant that refuses editing
//!   and executing groups, and a closed [`DelegateSet`] with no extend method.
//! - [`TaskBoard`] is the decomposition as values, carried in the accumulator,
//!   so "three of five discharged" is a count rather than a reading.
//! - [`Plan`], [`Attempt`], and [`Report`] are the three steps, each delegating
//!   its judgement to a seam ([`Decompose`], [`Specialists`], [`Compose`]) and
//!   keeping only the bookkeeping that has an invariant attached.
//!
//! # What it deliberately does not hold
//!
//! Anything that spans runs. `vendor/tinyflows/crates/adaptive` chooses *which
//! graph to run* across episodes; the orchestrator chooses *which specialists
//! to spawn* within one pass of one run of one graph. Neither reads the other's
//! state, and they compose by nesting rather than by sharing a state object.
//! There is therefore no ledger handle, no catalogue, and no score anywhere in
//! this module's types.
//!
//! # Examples
//!
//! ```
//! # use std::sync::Arc;
//! # use tinyloops::{
//! #     Attempt, DelegateSet, DEFAULT_MAILBOX_CAPACITY, FixedPlan, Inline, LoopState, Mailbox,
//! #     Orchestrator, Plan, Scripted, Step, StepContext, Thresholds, ToolGrant,
//! # };
//! let thresholds = Thresholds::default();
//! let delegates = DelegateSet::of(["prover"]);
//! let orchestrator = Orchestrator::new(ToolGrant::read_only(), delegates.clone())?;
//!
//! let plan = Plan::new(Arc::new(FixedPlan::of([(
//!     "bound-the-error",
//!     "bound the error term",
//!     "a proved bound",
//! )])));
//! let attempt = Attempt::new(
//!     orchestrator,
//!     Arc::new(Inline::new(
//!         delegates,
//!         [(
//!             "prover".to_owned(),
//!             vec![Scripted::Answers { reply: "bounded".to_owned(), artifacts: Vec::new() }],
//!         )],
//!     )),
//!     Arc::new(Mailbox::new(DEFAULT_MAILBOX_CAPACITY)),
//! );
//!
//! let state = plan
//!     .run(LoopState::new("bound it"), StepContext::advancing(0, &thresholds))?
//!     .into_state();
//! assert_eq!(state.board.len(), 1);
//!
//! let state = attempt
//!     .run(state, StepContext::advancing(0, &thresholds))?
//!     .into_state();
//! assert_eq!(state.attempts, 1);
//! assert_eq!(state.unproductive, 0);
//! # Ok::<(), tinyloops::Error>(())
//! ```

mod board;
mod role;
mod steps;

use std::collections::BTreeMap;
use std::sync::Mutex;

pub use board::{Task, TaskBoard, TaskId, TaskStatus};
pub use role::{DelegateSet, Orchestrator};
pub use steps::{
    Attempt, AttemptReport, Compose, Decompose, FixedPlan, Plan, Report, Specialists, Summarize,
};

use crate::error::{Error, Result};
use crate::harness::{Brief, DelegationOutcome, Scripted};

/// The reference [`Specialists`] dispatcher: a declared script, run inline.
///
/// It is the synchronous twin of
/// [`ScriptedDelegate`](crate::ScriptedDelegate), and it maps a script entry to
/// an outcome through [`Scripted::outcome`] rather than through a second copy
/// of that mapping, so the two cannot drift apart.
///
/// Running inline is not a simplification of the real thing; it is one of its
/// two legitimate modes. The engine runs a `Spawn`/`Gate` pair concurrently
/// when a `TaskRunner` is injected and inline when one is not, and the pass is
/// required to compute the same answer either way. This dispatcher is that
/// second mode, which is what lets a host run the loop before it has a
/// scheduler.
///
/// A role is answered from its own queue, in order, so the nth brief to a role
/// gets the nth script entry. When the queue runs dry the last entry repeats:
/// a loop runs an unknown number of passes, and a script that ran out would
/// turn a routing question into a fixture-length question.
#[derive(Debug)]
pub struct Inline {
    delegates: DelegateSet,
    script: BTreeMap<String, Vec<Scripted>>,
    served: Mutex<BTreeMap<String, usize>>,
}

impl Inline {
    /// A dispatcher over `delegates`, answering from `script`.
    pub fn of<I>(delegates: DelegateSet, script: I) -> Self
    where
        I: IntoIterator<Item = (String, Vec<Scripted>)>,
    {
        Self {
            delegates,
            script: script.into_iter().collect(),
            served: Mutex::new(BTreeMap::new()),
        }
    }

    /// A dispatcher over `delegates`, answering from `script`.
    ///
    /// An alias for [`Self::of`] reading better at a call site that passes an
    /// array literal.
    pub fn new<I>(delegates: DelegateSet, script: I) -> Self
    where
        I: IntoIterator<Item = (String, Vec<Scripted>)>,
    {
        Self::of(delegates, script)
    }

    /// How many briefs `role` has been served.
    ///
    /// A poisoned lock reads as zero rather than panicking. This is a
    /// diagnostic accessor, and a diagnostic that takes the process down when
    /// something else already went wrong is worse than one that reports
    /// nothing.
    #[must_use]
    pub fn served(&self, role: &str) -> usize {
        self.served
            .lock()
            .ok()
            .and_then(|served| served.get(role).copied())
            .unwrap_or_default()
    }
}

impl Specialists for Inline {
    fn dispatch(&self, briefs: Vec<(String, Brief)>) -> Result<Vec<DelegationOutcome>> {
        let mut outcomes = Vec::with_capacity(briefs.len());
        for (role, brief) in briefs {
            // Checked here as well as in `Orchestrator::spawn`, because this is
            // the door a host's own dispatcher would also have to guard: there
            // is no fallback to a wider registry anywhere on the path.
            if !self.delegates.holds(&role) {
                return Err(Error::UndeclaredDelegate { name: role });
            }
            let entries = self
                .script
                .get(&role)
                .ok_or_else(|| Error::UndeclaredDelegate { name: role.clone() })?;
            let Some(last) = entries.len().checked_sub(1) else {
                return Err(Error::SpawnRefused {
                    role: role.clone(),
                    reason: "the inline dispatcher holds no script for this role".to_owned(),
                });
            };

            let mut served = self.served.lock().map_err(|_| Error::StateEncoding)?;
            let cursor = served.entry(role).or_default();
            let entry = &entries[(*cursor).min(last)];
            *cursor += 1;
            drop(served);

            outcomes.push(entry.outcome(brief));
        }
        Ok(outcomes)
    }
}

#[cfg(test)]
mod test;
