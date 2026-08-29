//! The three nodes the orchestrator is bound to, as steps.
//!
//! `plan` decomposes on a cadence, `attempt` commissions one pass of work and
//! writes the one report every arm reads, and `report` composes the final
//! answer once nothing is still running. All three are ordinary
//! [`Step`](crate::Step) implementations, so they live in the same closed
//! registry as every other node body and an unknown name is an error rather
//! than a no-op.
//!
//! Each of the three delegates its *judgement* to a seam — [`Decompose`],
//! [`Specialists`], [`Compose`] — and keeps only the bookkeeping. That split is
//! what lets the whole module be tested offline with no model, and it is why a
//! host swapping in a real decomposer changes one constructor argument rather
//! than a step.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::harness::{Artifact, Brief, DelegationOutcome, Ending, Mailbox, Note};
use crate::policy::Route;
use crate::state::LoopState;
use crate::step::{Advanced, CanWrite, Step, StepContext};
use crate::step::{STEP_ATTEMPT, STEP_PLAN, STEP_REPORT};

use super::board::{Task, TaskBoard, TaskId, TaskStatus};
use super::role::Orchestrator;

/// Turns a goal into named tasks.
///
/// The seam exists so the cadence, the id stability, and the board bookkeeping
/// are tested without a model in the loop. An implementation returns the tasks
/// it believes the goal decomposes into; [`Plan`] decides which of them are new
/// and which are restatements, because that decision is the one with an
/// invariant attached.
pub trait Decompose: std::fmt::Debug + Send + Sync {
    /// The tasks `goal` decomposes into, as of `pass`.
    ///
    /// `board` is the current decomposition, so an implementation can restate
    /// rather than start over.
    ///
    /// # Errors
    ///
    /// Whatever the implementation raises. A decomposition that cannot be
    /// produced is a failure of the pass, not an empty board: an empty board
    /// reads as "nothing to do", which is the wrong answer to "I could not
    /// think of anything".
    fn decompose(&self, goal: &str, board: &TaskBoard, pass: u32) -> Result<Vec<Task>>;
}

/// A decomposition declared up front, for tests and for the offline example.
///
/// It returns the same tasks every time it is asked, which is exactly what
/// makes the cadence assertion meaningful: if the board changes between passes,
/// the cadence is why.
#[derive(Debug, Clone, Default)]
pub struct FixedPlan {
    tasks: Vec<(String, String, String)>,
}

impl FixedPlan {
    /// A decomposition into `(id, statement, criterion)` triples.
    #[must_use]
    pub fn of<I, S>(tasks: I) -> Self
    where
        I: IntoIterator<Item = (S, S, S)>,
        S: Into<String>,
    {
        Self {
            tasks: tasks
                .into_iter()
                .map(|(id, statement, criterion)| {
                    (id.into(), statement.into(), criterion.into())
                })
                .collect(),
        }
    }
}

impl Decompose for FixedPlan {
    fn decompose(&self, _goal: &str, _board: &TaskBoard, pass: u32) -> Result<Vec<Task>> {
        self.tasks
            .iter()
            .map(|(id, statement, criterion)| {
                Ok(Task::new(
                    TaskId::new(id.clone())?,
                    statement.clone(),
                    criterion.clone(),
                    pass,
                ))
            })
            .collect()
    }
}

/// Starts a pass's specialists and hands back what they came home with.
///
/// **Dispatch is one call rather than a spawn and a separate collect**, and the
/// reason is where the concurrency lives. In the emitted graph this pair is a
/// [`NodeKind::Spawn`] and a [`NodeKind::Gate`], and the engine's `TaskRunner`
/// owns the overlap; with no runner injected the engine runs the same work
/// inline and the tickets come back already settled. Either way the step sees
/// briefs going out and outcomes coming back, so the answer a pass computes
/// does not depend on whether a scheduler was present. That property is the
/// spec's, not this trait's convenience.
///
/// An implementation must return one outcome per brief, in order. A specialist
/// that timed out, was capped, or failed still returns a
/// [`DelegationOutcome`]: a delegation that fails is a result, not an end.
///
/// [`NodeKind::Spawn`]: https://docs.rs/tinyflows
/// [`NodeKind::Gate`]: https://docs.rs/tinyflows
pub trait Specialists: std::fmt::Debug + Send + Sync {
    /// Runs every brief and returns their outcomes, one per brief, in order.
    ///
    /// # Errors
    ///
    /// Only for a failure of the dispatch itself — a specialist outside the
    /// declared set, a harness that will not start. A specialist that ran and
    /// ended badly is an [`Ending`], not an error.
    fn dispatch(&self, briefs: Vec<(String, Brief)>) -> Result<Vec<DelegationOutcome>>;
}

/// Composes the run's final answer.
///
/// Separate from [`Decompose`] because the two are asked different questions at
/// opposite ends of a run, and because the report is the one artifact with a
/// sole-author rule attached to it.
pub trait Compose: std::fmt::Debug + Send + Sync {
    /// The final answer, given everything the run established.
    ///
    /// # Errors
    ///
    /// Whatever the implementation raises.
    fn compose(&self, state: &LoopState) -> Result<String>;
}

/// What one pass commissioned and what came back.
///
/// This is the single artifact every evaluation arm reads, which is what makes
/// the arms independent of one another and therefore concurrent: each reads the
/// same input and none reads another's output, so a pass costs the slowest arm
/// rather than the sum of them.
///
/// It is a typed, serializable record rather than prose, so an arm can count
/// what happened instead of parsing a paragraph about it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AttemptReport {
    /// The pass this report is for.
    pub pass: u32,
    /// The route the previous pass's counters chose, which shaped the briefs.
    pub route: String,
    /// One row per specialist, in the order they were briefed.
    pub outcomes: Vec<DelegationOutcome>,
    /// Operator directives drained from the mailbox before briefing.
    pub directives: Vec<String>,
    /// Every artifact the pass left behind, across all specialists.
    pub artifacts: Vec<Artifact>,
}

impl AttemptReport {
    /// Whether anything the pass commissioned came back as evidence about the
    /// goal.
    ///
    /// A reply or an artifact from *any* specialist is enough. That "any" is
    /// load-bearing: a pass where one specialist timed out and another answered
    /// produced work, and counting it unproductive would spend a diversify on a
    /// run that was not stuck.
    ///
    /// A [`Ending::Failed`] outcome's reply is *not* evidence, and the
    /// distinction is the whole reason [`LoopState::blocked`] exists apart from
    /// [`LoopState::unproductive`]. A specialist that could not start reports
    /// why it could not start, which is readable, useful, and says nothing
    /// about the goal. Artifacts still count, because a failure that left files
    /// behind left work behind.
    ///
    /// [`LoopState::blocked`]: crate::LoopState::blocked
    /// [`LoopState::unproductive`]: crate::LoopState::unproductive
    #[must_use]
    pub fn is_informative(&self) -> bool {
        self.outcomes.iter().any(Self::is_evidence)
    }

    /// How many specialists never got as far as saying anything about the goal.
    #[must_use]
    pub fn blocked(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|outcome| outcome.ending == Ending::Failed)
            .count()
    }

    /// Whether the pass learned nothing except that the machinery would not run.
    ///
    /// "Only outcome" is literal: one specialist that failed alongside one that
    /// merely came back empty is an unproductive pass, not a blocked one, and
    /// the two rungs of the ladder are different distances from the exit.
    #[must_use]
    pub fn is_blocked(&self) -> bool {
        !self.outcomes.is_empty() && self.blocked() == self.outcomes.len()
    }

    fn is_evidence(outcome: &DelegationOutcome) -> bool {
        !outcome.artifacts.is_empty()
            || (outcome.ending != Ending::Failed && outcome.reply.is_some())
    }
}

/// `plan`: the decomposition, written on a cadence.
///
/// It runs at pass 0 and then only on [`Thresholds::plan_interval`]. Re-planning
/// every pass spends a full role's work rewording a decomposition nothing has
/// yet tested, and makes the board unstable, so "task 3 is still open" stops
/// meaning anything across passes.
///
/// [`Thresholds::plan_interval`]: crate::Thresholds::plan_interval
#[derive(Debug)]
pub struct Plan {
    decompose: Arc<dyn Decompose>,
}

impl Plan {
    /// The plan step, decomposing through `decompose`.
    #[must_use]
    pub fn new(decompose: Arc<dyn Decompose>) -> Self {
        Self { decompose }
    }

    /// Whether the step would do anything at `pass`.
    ///
    /// Pass 0 always plans: the first attempt should already have a
    /// decomposition rather than spending itself acquiring one.
    #[must_use]
    pub fn plans_on(pass: u32, thresholds: &crate::policy::Thresholds) -> bool {
        pass == 0 || thresholds.plans_on(pass)
    }
}

impl Step for Plan {
    fn name(&self) -> &'static str {
        STEP_PLAN
    }

    fn run(&self, state: LoopState, ctx: StepContext<'_, CanWrite>) -> Result<Advanced> {
        let pass = ctx.pass();
        if !Self::plans_on(pass, ctx.thresholds()) {
            return Ok(ctx.advance(state));
        }

        let mut state = state;
        let proposed = self.decompose.decompose(&state.goal, &state.board, pass)?;
        for task in proposed {
            if state.board.find(&task.id).is_some() {
                state
                    .board
                    .restate(&task.id, task.statement, task.criterion, pass)?;
            } else {
                state.board.add(task)?;
            }
        }
        state.board.planned(pass);
        Ok(ctx.advance(state))
    }
}

/// `attempt`: one pass of commissioned work, and the one report it writes.
///
/// It never discharges a task on a specialist's say-so. A task moves to
/// [`TaskStatus::InFlight`] when it is briefed and no further, because the
/// criterion that would discharge it is checked against the workspace, not
/// against a reply. Everything this step credits the run with —
/// [`LoopState::established`] — is counted off artifacts for the same reason.
#[derive(Debug)]
pub struct Attempt {
    orchestrator: Orchestrator,
    specialists: Arc<dyn Specialists>,
    mailbox: Arc<Mailbox>,
}

impl Attempt {
    /// The attempt step.
    #[must_use]
    pub fn new(
        orchestrator: Orchestrator,
        specialists: Arc<dyn Specialists>,
        mailbox: Arc<Mailbox>,
    ) -> Self {
        Self {
            orchestrator,
            specialists,
            mailbox,
        }
    }

    /// The mailbox operator directives are posted into.
    ///
    /// Bounded, and a post at capacity drops the note rather than stalling the
    /// loop. See [`Mailbox`] for why dropping is the only one of the three
    /// failure modes that leaves the loop running.
    #[must_use]
    pub fn mailbox(&self) -> &Arc<Mailbox> {
        &self.mailbox
    }

    /// The briefs this pass would send, given `state` and the route it implies.
    ///
    /// Exposed so a test can assert the shape of a pass without dispatching it,
    /// and so the diversify rule is checkable rather than inferred from an
    /// outcome.
    #[must_use]
    pub fn briefs(&self, state: &LoopState, route: Route, directives: &[Note]) -> Vec<(String, Brief)> {
        let context = Self::context(state, directives);
        let outstanding: Vec<&Task> = state.board.outstanding();
        let delegates: Vec<&str> = self.orchestrator.delegates().names().collect();

        if route == Route::Diversify {
            // Sequential revision conditions every next attempt on the same
            // failed framing. Drawing several independent attempts on one task
            // does not, which is the whole reason this rung exists.
            let task = outstanding.first();
            return delegates
                .iter()
                .map(|role| {
                    let brief = match task {
                        Some(task) => Brief::new(task.statement.clone()),
                        None => Brief::new(state.goal.clone()),
                    };
                    ((*role).to_owned(), brief.with_context(context.clone()))
                })
                .collect();
        }

        let Some(first) = delegates.first() else {
            return Vec::new();
        };
        if outstanding.is_empty() {
            return vec![(
                (*first).to_owned(),
                Brief::new(state.goal.clone()).with_context(context),
            )];
        }
        outstanding
            .iter()
            .zip(delegates.iter().cycle())
            .map(|(task, role)| {
                (
                    (*role).to_owned(),
                    Brief::new(task.statement.clone()).with_context(context.clone()),
                )
            })
            .collect()
    }

    /// The context every brief in a pass carries: the steer, the last lesson,
    /// and whatever the operator said.
    fn context(state: &LoopState, directives: &[Note]) -> String {
        let mut lines = Vec::new();
        if !state.steer.is_empty() {
            lines.push(format!("steer: {}", state.steer));
        }
        if let Some(lesson) = state.lessons.last() {
            lines.push(format!("lesson: {lesson}"));
        }
        for note in directives {
            lines.push(format!("directive from {}: {}", note.from, note.body));
        }
        lines.join("\n")
    }
}

impl Step for Attempt {
    fn name(&self) -> &'static str {
        STEP_ATTEMPT
    }

    fn run(&self, state: LoopState, ctx: StepContext<'_, CanWrite>) -> Result<Advanced> {
        let mut state = state;
        let pass = ctx.pass();

        // Drained first, so a directive posted since the last pass shapes this
        // pass's briefs rather than the next one's.
        let directives = self.mailbox.collect();
        let route = crate::policy::route(&state, ctx.thresholds());
        let briefs = self.briefs(&state, route, &directives);

        for (_, brief) in &briefs {
            if let Some(task) = state
                .board
                .tasks()
                .iter()
                .find(|task| task.statement == brief.task)
                .map(|task| task.id.clone())
            {
                state.board.settle(&task, TaskStatus::InFlight, pass)?;
            }
        }

        let outcomes = self.specialists.dispatch(briefs)?;
        let artifacts: Vec<Artifact> = outcomes
            .iter()
            .flat_map(|outcome| outcome.artifacts.clone())
            .collect();

        let report = AttemptReport {
            pass,
            route: route.as_str().to_owned(),
            directives: directives.iter().map(|note| note.body.clone()).collect(),
            artifacts,
            outcomes,
        };

        state.attempts = state.attempts.saturating_add(1);
        state.established = state
            .established
            .saturating_add(u32::try_from(report.artifacts.len()).unwrap_or(u32::MAX));
        // The three counters this step may move, and the order they are decided
        // in. Evidence breaks both streaks; a pass that learned only that the
        // machinery would not run is blocked rather than unproductive, because
        // infrastructure failure is not evidence about the goal and the ladder
        // exits on it far sooner.
        if report.is_informative() {
            state.unproductive = 0;
            state.blocked = 0;
        } else if report.is_blocked() {
            state.blocked = state.blocked.saturating_add(1);
        } else {
            state.unproductive = state.unproductive.saturating_add(1);
        }
        state.last_attempt = serde_json::to_string(&report).map_err(|_| crate::Error::StateEncoding)?;

        Ok(ctx.advance(state))
    }
}

/// `report`: the run's sole account of what it concluded.
///
/// It writes [`LoopState::answer`] and nothing else writes that field. That is
/// not a convention: [`LoopState::apply`] carries the answer through the fold
/// untouched, so no arm and no delta can reach it however it is wired.
#[derive(Debug)]
pub struct Report {
    compose: Arc<dyn Compose>,
}

impl Report {
    /// The report step, composing through `compose`.
    #[must_use]
    pub fn new(compose: Arc<dyn Compose>) -> Self {
        Self { compose }
    }
}

impl Step for Report {
    fn name(&self) -> &'static str {
        STEP_REPORT
    }

    fn run(&self, state: LoopState, ctx: StepContext<'_, CanWrite>) -> Result<Advanced> {
        let mut state = state;
        state.answer = self.compose.compose(&state)?;
        Ok(ctx.advance(state))
    }
}

/// The reference composer: the board, the lessons, and how the run ended.
///
/// It renders values rather than asking anything, which is what makes the
/// offline example produce the same report on every run. It says which tasks
/// were discharged and which were not, and it renders a run that stopped on a
/// budget as a run that stopped on a budget.
#[derive(Debug, Clone, Copy, Default)]
pub struct Summarize;

impl Compose for Summarize {
    fn compose(&self, state: &LoopState) -> Result<String> {
        let mut out = format!("# {}\n\n", state.goal);
        out.push_str(&format!(
            "{} of {} tasks discharged after {} attempts across {} passes.\n\n",
            state.board.count(TaskStatus::Discharged),
            state.board.len(),
            state.attempts,
            state.passes,
        ));
        for task in state.board.tasks() {
            out.push_str(&format!(
                "- [{}] {} ({})\n",
                task.status.as_str(),
                task.statement,
                task.criterion,
            ));
        }
        if !state.lessons.is_empty() {
            out.push_str("\nLearned:\n");
            for lesson in &state.lessons {
                out.push_str(&format!("- {lesson}\n"));
            }
        }
        Ok(out)
    }
}
