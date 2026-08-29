//! The decomposition, as values rather than as prose.
//!
//! A plan written into a prompt cannot be counted, cannot be checked, and
//! cannot be routed on. "Three of five tasks discharged" is a fact only if the
//! tasks are values, so the decomposition lives here as a [`TaskBoard`]: a
//! serializable list of [`Task`]s carried in the loop's accumulator, read by
//! code, rendered by the report, and countable by anything that asks.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// A task's stable handle.
///
/// Stable is the whole property. Every count that spans passes — "task 3 is
/// still open" — resolves through this id, so a re-plan may add, restate, or
/// close a task, and may never reuse an id for a different one. [`TaskBoard`]
/// refuses that at the door rather than trusting a caller to remember.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskId(String);

impl TaskId {
    /// Names a task.
    ///
    /// # Errors
    ///
    /// [`Error::EmptyName`] when `id` is empty or only whitespace. An unnamed
    /// task is one no count can address.
    pub fn new(id: impl Into<String>) -> Result<Self> {
        let id = id.into();
        if id.trim().is_empty() {
            return Err(Error::EmptyName);
        }
        Ok(Self(id))
    }

    /// The id as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a task has got to.
///
/// Four states rather than a boolean, because "we stopped working on this"
/// and "we finished this" route differently and read differently in a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Stated, and nothing has been spent on it yet.
    #[default]
    Open,
    /// A specialist is working on it, or was when the pass ended.
    InFlight,
    /// Its completion criterion was met, on evidence the run can cite.
    Discharged,
    /// Deliberately dropped: the run decided it is not the way in.
    Abandoned,
}

impl TaskStatus {
    /// The status's wire name.
    ///
    /// Hand-written rather than taken from [`Debug`], for the reason every
    /// other wire string in this crate is: a `Debug` rendering is a diagnostic
    /// that moves when a variant is renamed, and this one is written into event
    /// streams that outlive the process.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::InFlight => "in_flight",
            Self::Discharged => "discharged",
            Self::Abandoned => "abandoned",
        }
    }

    /// Whether the run is done with this task, one way or the other.
    #[must_use]
    pub const fn is_settled(self) -> bool {
        matches!(self, Self::Discharged | Self::Abandoned)
    }
}

/// One named part of the goal, with the criterion that would discharge it.
///
/// The criterion is a required field rather than an optional one. A task
/// without one cannot be closed on evidence, only asserted closed, and a board
/// of those is prose again with extra syntax.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    /// The stable handle.
    pub id: TaskId,
    /// What the task is.
    pub statement: String,
    /// What would count as discharging it.
    pub criterion: String,
    /// Where it has got to.
    pub status: TaskStatus,
    /// The pass that last changed it.
    pub touched: u32,
}

impl Task {
    /// States a task, open, as of `pass`.
    #[must_use]
    pub fn new(
        id: TaskId,
        statement: impl Into<String>,
        criterion: impl Into<String>,
        pass: u32,
    ) -> Self {
        Self {
            id,
            statement: statement.into(),
            criterion: criterion.into(),
            status: TaskStatus::Open,
            touched: pass,
        }
    }
}

/// The run's decomposition: every task, in the order it was stated.
///
/// The board lives in [`LoopState`](crate::LoopState), which means the loop
/// head is its only writer — invariant 1 of the loop kernel. `plan` and
/// `attempt` return a whole state containing an updated board; neither writes
/// the accumulator slot. It is therefore checkpointed and resumed with every
/// other counter, which is what makes a task id stable across a crash rather
/// than only across a pass.
///
/// # Examples
///
/// ```
/// # use tinyloops::{Task, TaskBoard, TaskId, TaskStatus};
/// let mut board = TaskBoard::new();
/// let id = TaskId::new("bound-the-error")?;
/// board.add(Task::new(id.clone(), "bound the error term", "a proved bound", 0))?;
///
/// board.settle(&id, TaskStatus::Discharged, 2)?;
/// assert_eq!(board.count(TaskStatus::Discharged), 1);
/// assert_eq!(board.len(), 1);
/// # Ok::<(), tinyloops::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskBoard {
    tasks: Vec<Task>,
    planned_at: Option<u32>,
}

impl TaskBoard {
    /// An empty board.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a task.
    ///
    /// # Errors
    ///
    /// [`Error::DuplicateTask`] when the board already holds the id. Use
    /// [`Self::restate`] to change a task that already exists: reusing an id
    /// for a different task breaks every count that reads the board across
    /// passes, and does it silently.
    pub fn add(&mut self, task: Task) -> Result<()> {
        if self.find(&task.id).is_some() {
            return Err(Error::DuplicateTask {
                id: task.id.to_string(),
            });
        }
        self.tasks.push(task);
        Ok(())
    }

    /// Rewrites an existing task's statement and criterion, keeping its id.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownTask`] when no task answers to `id`.
    pub fn restate(
        &mut self,
        id: &TaskId,
        statement: impl Into<String>,
        criterion: impl Into<String>,
        pass: u32,
    ) -> Result<()> {
        let task = self.find_mut(id)?;
        task.statement = statement.into();
        task.criterion = criterion.into();
        task.touched = pass;
        Ok(())
    }

    /// Moves a task to `status` as of `pass`.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownTask`] when no task answers to `id`.
    pub fn settle(&mut self, id: &TaskId, status: TaskStatus, pass: u32) -> Result<()> {
        let task = self.find_mut(id)?;
        task.status = status;
        task.touched = pass;
        Ok(())
    }

    /// Records that the board was (re)planned at `pass`.
    pub fn planned(&mut self, pass: u32) {
        self.planned_at = Some(pass);
    }

    /// The pass the board was last planned at, or `None` before the first plan.
    #[must_use]
    pub fn planned_at(&self) -> Option<u32> {
        self.planned_at
    }

    /// The task with `id`, if the board holds it.
    #[must_use]
    pub fn find(&self, id: &TaskId) -> Option<&Task> {
        self.tasks.iter().find(|task| &task.id == id)
    }

    /// Every task, in the order stated.
    #[must_use]
    pub fn tasks(&self) -> &[Task] {
        &self.tasks
    }

    /// How many tasks are in `status`.
    ///
    /// This is the method that makes the board worth having: a count is a read
    /// over values, so "three of five discharged" needs no model and no parse.
    #[must_use]
    pub fn count(&self, status: TaskStatus) -> usize {
        self.tasks
            .iter()
            .filter(|task| task.status == status)
            .count()
    }

    /// How many tasks are on the board.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    /// Whether nothing has been stated yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// Whether every task is settled, one way or the other.
    ///
    /// An empty board is *not* complete. Nothing stated is not everything done,
    /// and reporting it as done would let a failed `plan` read as success.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        !self.tasks.is_empty() && self.tasks.iter().all(|task| task.status.is_settled())
    }

    /// The tasks still worth spending a pass on, in the order stated.
    #[must_use]
    pub fn outstanding(&self) -> Vec<&Task> {
        self.tasks
            .iter()
            .filter(|task| !task.status.is_settled())
            .collect()
    }

    fn find_mut(&mut self, id: &TaskId) -> Result<&mut Task> {
        self.tasks
            .iter_mut()
            .find(|task| &task.id == id)
            .ok_or_else(|| Error::UnknownTask { id: id.to_string() })
    }
}
