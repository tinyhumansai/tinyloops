//! The records the harness seam moves around: what a role is, what a
//! delegation is asked for, what it returns, and the bounded mailbox notes
//! travel through.
//!
//! Everything here is data plus the arithmetic that keeps it honest. The traits
//! that operate on it, and the one offline implementation of them, live in the
//! module root.

use std::collections::BTreeSet;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::{Caps, Error, Result, RunBudget};

/// Which class of model a role runs on.
///
/// A tier rather than a model name on purpose: a role declares the *kind* of
/// thinking it needs, and the deployment maps that onto whatever it has. A role
/// pinned to a vendor's model string is a role that has to be edited when the
/// deployment changes providers, and the edit is invisible until it is wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// The cheapest tier: extraction, formatting, tidying.
    Small,
    /// The working tier: most attempts and most judgements.
    Standard,
    /// The expensive tier, for work that has already failed cheaply.
    Deep,
}

impl Tier {
    /// The wire name of this tier.
    ///
    /// Hand-written rather than derived from [`Debug`], because the string is
    /// written into role declarations that outlive the process and a rename
    /// must be a compile error here rather than a quiet change of vocabulary
    /// out there.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Standard => "standard",
            Self::Deep => "deep",
        }
    }

    /// Reads a tier name, strictly.
    ///
    /// An unknown name is `None`. Falling back to a default tier would silently
    /// run an expensive role cheaply, and the only symptom would be worse
    /// answers.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tinyloops::Tier;
    /// assert_eq!(Tier::parse(" DEEP "), Some(Tier::Deep));
    /// assert_eq!(Tier::parse("deepest"), None);
    /// ```
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "small" => Some(Self::Small),
            "standard" => Some(Self::Standard),
            "deep" => Some(Self::Deep),
            _ => None,
        }
    }
}

/// The set of tool names a role may use.
///
/// Names, not instances. Resolving a name to a callable tool is the tools
/// seam's job, and keeping the two apart is what lets a role be declared
/// without a tool registry in scope. The set is ordered so a grant renders the
/// same way every time it is printed or serialized.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RoleGrant {
    names: BTreeSet<String>,
}

impl RoleGrant {
    /// An empty grant: a role that may call no tools at all.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// A grant over the named tools.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tinyloops::RoleGrant;
    /// let grant = RoleGrant::of(["read", "search"]);
    /// assert!(grant.allows("read"));
    /// assert!(!grant.allows("execute"));
    /// ```
    #[must_use]
    pub fn of<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            names: names.into_iter().map(Into::into).collect(),
        }
    }

    /// Whether this grant names `tool`.
    ///
    /// This answers a question about a declaration. It is *not* the control
    /// that withholds a tool: a withheld tool is withheld by never registering
    /// it, and a check a caller may forget to make is not an enforcement.
    #[must_use]
    pub fn allows(&self, tool: &str) -> bool {
        self.names.contains(tool)
    }

    /// The names in this grant, in order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.names.iter().map(String::as_str)
    }

    /// How many tools this grant names.
    #[must_use]
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether this grant names nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

/// A role: a prompt, a tool grant, a budget, and a model tier.
///
/// Four things and nothing else. The constraint is the point — a role that also
/// carried a retry policy, a provider, or a scratch directory would be a place
/// for deployment detail to accumulate where a call site cannot see it.
///
/// The budget is per-role and narrowed from the run's, never inherited whole: a
/// role that reads a report and answers in four lines, handed an
/// investigation's budget, investigates, because it has the calls. See
/// [`RunBudget::narrow`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Role {
    prompt: String,
    grant: RoleGrant,
    budget: RunBudget,
    tier: Tier,
}

impl Role {
    /// Builds a role from its four parts.
    ///
    /// # Errors
    ///
    /// Returns whatever [`RunBudget::new`] returns for `caps`: a role is not
    /// constructible with a budget that could not stop it.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tinyloops::{Caps, Role, RoleGrant, Tier};
    /// let role = Role::new("judge one attempt", RoleGrant::of(["read"]), Caps::default(), Tier::Standard)?;
    /// assert_eq!(role.tier(), Tier::Standard);
    /// # Ok::<(), tinyloops::Error>(())
    /// ```
    pub fn new(
        prompt: impl Into<String>,
        grant: RoleGrant,
        caps: Caps,
        tier: Tier,
    ) -> Result<Self> {
        Ok(Self {
            prompt: prompt.into(),
            grant,
            budget: RunBudget::new(caps)?,
            tier,
        })
    }

    /// The role's prompt.
    #[must_use]
    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    /// The tools the role may use.
    #[must_use]
    pub fn grant(&self) -> &RoleGrant {
        &self.grant
    }

    /// The role's validated budget.
    #[must_use]
    pub fn budget(&self) -> RunBudget {
        self.budget
    }

    /// The role's model tier.
    #[must_use]
    pub fn tier(&self) -> Tier {
        self.tier
    }
}

/// What a delegation is asked to do.
///
/// Carried through to the [`DelegationOutcome`] verbatim, so an outcome read
/// months later still says what was asked as well as what came back. An outcome
/// that names only its result is unreadable exactly when it matters: when it
/// went wrong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Brief {
    /// What the delegation is for.
    pub task: String,
    /// What it needs to know to start.
    pub context: String,
}

impl Brief {
    /// A brief with no context beyond the task.
    #[must_use]
    pub fn new(task: impl Into<String>) -> Self {
        Self {
            task: task.into(),
            context: String::new(),
        }
    }

    /// This brief with `context` attached.
    #[must_use]
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = context.into();
        self
    }
}

/// A handle to work that was started and has not been collected.
///
/// Opaque and cheap to clone. It identifies a delegation without holding it, so
/// the loop can start several, take its next step, and come back to them.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Ticket {
    id: String,
}

impl Ticket {
    /// Builds a ticket from its identifier.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }

    /// The ticket's identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// Where a delegation has got to, as of the moment it was asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Accepted but not started.
    Pending,
    /// Running now.
    Running,
    /// Finished, and its outcome is waiting to be collected.
    Ready,
    /// Collected: its outcome has already been handed over.
    Settled,
}

impl Status {
    /// Whether a caller collecting this ticket would have to wait.
    #[must_use]
    pub fn is_outstanding(self) -> bool {
        matches!(self, Self::Pending | Self::Running)
    }
}

/// How a delegation ended.
///
/// Every variant is a *result*. A delegation that timed out, was killed at its
/// cap, or failed outright still produces an outcome the next pass can read and
/// act on, because the alternative — an error return — throws away everything
/// the delegation produced before it ended, and the pass is then judged on
/// silence rather than on evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ending {
    /// It finished and answered.
    Answered,
    /// It ran out of time.
    TimedOut,
    /// Its own cap stopped it.
    ///
    /// The ordinary way a long delegation ends. It destroys the reply and
    /// leaves every file the delegation wrote, which is exactly the case
    /// [`salvage`] exists for.
    Capped,
    /// It failed.
    Failed,
}

impl Ending {
    /// The wire name of this ending.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Answered => "answered",
            Self::TimedOut => "timed_out",
            Self::Capped => "capped",
            Self::Failed => "failed",
        }
    }

    /// Whether the delegation produced the answer it was asked for.
    #[must_use]
    pub fn is_answered(self) -> bool {
        matches!(self, Self::Answered)
    }
}

/// Something a delegation left behind.
///
/// Named rather than embedded: the outcome cites what exists, and whoever reads
/// the outcome fetches it. An outcome that inlined every artifact would be an
/// outcome nobody can put in a prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    /// Where it is.
    pub path: String,
    /// What it is.
    pub description: String,
}

impl Artifact {
    /// An artifact at `path`, described by `description`.
    #[must_use]
    pub fn new(path: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            description: description.into(),
        }
    }
}

/// What a delegation came back with.
///
/// Named `DelegationOutcome` rather than `Outcome` because the crate root
/// already exports [`Outcome`](crate::Outcome), the way a whole *run* came out.
/// One delegation ending is not one run ending, and giving them the same name
/// at the crate surface would invite a call site to conflate them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegationOutcome {
    /// What was asked.
    pub brief: Brief,
    /// How it ended.
    pub ending: Ending,
    /// What it left behind, whether or not it answered.
    pub artifacts: Vec<Artifact>,
    /// What it said, when it got as far as saying anything.
    pub reply: Option<String>,
}

impl DelegationOutcome {
    /// An outcome for a delegation that answered.
    #[must_use]
    pub fn answered(brief: Brief, reply: impl Into<String>) -> Self {
        Self {
            brief,
            ending: Ending::Answered,
            artifacts: Vec::new(),
            reply: Some(reply.into()),
        }
    }

    /// This outcome with `artifacts` attached.
    #[must_use]
    pub fn with_artifacts(mut self, artifacts: Vec<Artifact>) -> Self {
        self.artifacts = artifacts;
        self
    }

    /// Whether this outcome carries anything the next pass can act on.
    ///
    /// A reply or an artifact is evidence; neither is silence. The distinction
    /// is what the routing ladder needs in order to tell "the arm found
    /// nothing" from "the arm never reported".
    #[must_use]
    pub fn is_informative(&self) -> bool {
        self.reply.is_some() || !self.artifacts.is_empty()
    }
}

/// Builds the outcome of a delegation its own cap killed.
///
/// The cap destroys the reply and leaves every file the delegation wrote.
/// Without this the pass reports nothing, and the routing ladder spends a
/// diversify on a run that was not stuck — it was cut off mid-sentence with its
/// work on disk.
///
/// # Examples
///
/// ```
/// # use tinyloops::{Artifact, Brief, Ending, salvage};
/// let outcome = salvage(
///     Brief::new("survey the failing tests"),
///     vec![Artifact::new("notes.md", "partial survey")],
/// );
/// assert_eq!(outcome.ending, Ending::Capped);
/// assert!(outcome.reply.is_none());
/// assert!(outcome.is_informative());
/// ```
#[must_use]
pub fn salvage(brief: Brief, artifacts: Vec<Artifact>) -> DelegationOutcome {
    DelegationOutcome {
        brief,
        ending: Ending::Capped,
        artifacts,
        reply: None,
    }
}

/// An aside sent to a running delegation, or collected from one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    /// Who sent it.
    pub from: String,
    /// What it says.
    pub body: String,
}

impl Note {
    /// A note from `from` saying `body`.
    #[must_use]
    pub fn new(from: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            from: from.into(),
            body: body.into(),
        }
    }
}

/// What happened to a posted note.
///
/// A value, never an `Err`. An error would push the caller toward handling it
/// by waiting, which is the one response a bounded mailbox exists to prevent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Posted {
    /// The mailbox took it.
    Accepted,
    /// The mailbox was full, and the note was dropped rather than queued.
    ///
    /// Carries the note back so the caller can log it, fold it into the next
    /// brief, or decide it did not matter.
    Dropped(Note),
}

impl Posted {
    /// Whether the note was taken.
    #[must_use]
    pub fn is_accepted(&self) -> bool {
        matches!(self, Self::Accepted)
    }
}

/// Told about every dropped note.
///
/// A local trait rather than the loop's [`Sink`](crate::Sink), because the
/// event vocabulary in `observe` has no drop variant and inventing one by
/// reusing an unrelated variant would put a lie on the journal. An embedder
/// wires this onto whatever it records with.
pub trait DropObserver: std::fmt::Debug + Send + Sync {
    /// Reports one dropped note and the capacity it did not fit in.
    fn dropped(&self, note: &Note, capacity: usize);
}

/// A bounded post/collect queue that drops rather than blocks.
///
/// Three behaviors are possible when a note arrives at a full queue, and only
/// one of them leaves the loop running:
///
/// - grow, which turns a slow consumer into unbounded memory;
/// - block, which turns a slow consumer into a stalled solve;
/// - drop, which loses an aside.
///
/// The note is an aside; the solve is the work. So this drops, and reports the
/// drop through [`DropObserver`] and [`Mailbox::drops`], because the only thing
/// worse than a dropped note is a dropped note nobody can see.
#[derive(Debug)]
pub struct Mailbox {
    capacity: usize,
    state: Mutex<MailboxState>,
    observer: Option<std::sync::Arc<dyn DropObserver>>,
}

#[derive(Debug, Default)]
struct MailboxState {
    queued: Vec<Note>,
    drops: usize,
}

impl Mailbox {
    /// A mailbox holding at most `capacity` notes.
    ///
    /// Capacity is declared here and nowhere else: a queue whose bound is set
    /// by whoever posts to it has no bound.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            state: Mutex::new(MailboxState::default()),
            observer: None,
        }
    }

    /// A mailbox that tells `observer` about every drop.
    #[must_use]
    pub fn observed(capacity: usize, observer: std::sync::Arc<dyn DropObserver>) -> Self {
        Self {
            capacity,
            state: Mutex::new(MailboxState::default()),
            observer: Some(observer),
        }
    }

    /// How many notes this mailbox holds at once.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Posts `note`, dropping it if the mailbox is full.
    ///
    /// Never blocks and never returns an error. A full mailbox yields
    /// [`Posted::Dropped`] carrying the note straight back.
    pub fn post(&self, note: Note) -> Posted {
        let mut state = self.lock();
        if state.queued.len() >= self.capacity {
            state.drops += 1;
            drop(state);
            if let Some(observer) = &self.observer {
                observer.dropped(&note, self.capacity);
            }
            return Posted::Dropped(note);
        }
        state.queued.push(note);
        Posted::Accepted
    }

    /// Takes every queued note, leaving the mailbox empty.
    pub fn collect(&self) -> Vec<Note> {
        std::mem::take(&mut self.lock().queued)
    }

    /// How many notes are waiting.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().queued.len()
    }

    /// Whether nothing is waiting.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How many notes have been dropped over this mailbox's life.
    ///
    /// The count is cumulative and survives [`Mailbox::collect`], so "did this
    /// run lose asides" is answerable at the end of the run rather than only at
    /// the moment of the loss.
    #[must_use]
    pub fn drops(&self) -> usize {
        self.lock().drops
    }

    /// The queue, recovering from a poisoned lock rather than panicking.
    ///
    /// A mailbox is an aside. Taking the loop down because a thread panicked
    /// while holding this lock would trade the whole solve for the queue, which
    /// is the trade every rule in this module rejects.
    fn lock(&self) -> std::sync::MutexGuard<'_, MailboxState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Builds the refusal a spawn returns, naming the role and the reason.
///
/// One spelling of the refusal, so every path that declines to start work
/// declines it the same way.
pub(super) fn refuse(role: &str, reason: &'static str) -> Error {
    Error::SpawnRefused {
        role: role.to_owned(),
        reason: reason.to_owned(),
    }
}
