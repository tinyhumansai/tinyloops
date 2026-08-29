//! The harness seam: who a loop can hand work to, and how it gets it back.
//!
//! A loop that only calls a model directly has one shape of attempt available
//! to it. Delegation is what lets a pass fan out — a researcher, a judge, a
//! specialist — and the deployment supplies the thing that actually runs those,
//! because one embedder has a durable agent harness and another has a single
//! model client. This module defines what a delegation *is*, and ships one
//! in-process implementation so an example runs with no credentials, no
//! network, and no dependence on the wall clock.
//!
//! # A role is four things
//!
//! [`Role`] is a prompt, a [`RoleGrant`], a [`RunBudget`], and a [`Tier`]. A
//! [`RoleRegistry`] resolves a name to that record, so a call site names a role
//! rather than assembling a model configuration inline. The budget is narrowed
//! per role rather than inherited from the run: see [`RunBudget::narrow`] for
//! the judge that spent four minutes reading source files because nothing
//! stopped it.
//!
//! # Delegation is asynchronous by construction
//!
//! [`Delegate`] has four operations and no fifth:
//!
//! ```text
//! let ticket = delegate.spawn(role, brief)?;  // returns immediately
//! let status = delegate.peek(&ticket)?;       // a separate call
//! delegate.steer(&ticket, note);              // a separate call
//! let outcome = delegate.settle(&ticket).await?;
//! ```
//!
//! **There is no blocking delegation, and adding one is a contract change
//! rather than a convenience.** The reason is measured. In a production run of
//! this design the loop sat 33 minutes unable to start its next attempt because
//! it was awaiting a single arm: the wait was invisible, nothing else could
//! proceed, and no observer could say which node was holding. A seam that
//! offers a call which both starts and finishes work will have that call used —
//! it is always the shorter line — so this seam offers none. `harness/test.rs`
//! asserts the absence over the trait's own method list rather than trusting
//! anyone to remember it.
//!
//! [`Delegate::settle`] returns a boxed future rather than being an `async fn`.
//! Rust 2024 has `async fn` in traits natively, but a trait containing one is
//! not dyn-compatible, and this seam exists to be held as `Arc<dyn Delegate>`
//! by a loop that does not know which harness it got. The boxed future keeps
//! both properties: asynchronous, and object-safe.
//!
//! # A full mailbox drops
//!
//! [`Mailbox`] is bounded post/collect. Posting at capacity returns
//! [`Posted::Dropped`] with the note, counts the drop, and tells a
//! [`DropObserver`]; it does not block and it does not grow. Back-pressure that
//! stalls the solve is the wrong trade — the note is an aside, the solve is the
//! work — and a drop nobody records is worse than the drop.
//!
//! # A failed delegation is a result, not an end
//!
//! Every [`Ending`] is a readable [`DelegationOutcome`] that names the brief,
//! how it ended, and what it left behind. [`salvage`] builds the outcome for
//! the ordinary case: a delegation its own cap killed, whose reply is gone and
//! whose files are all still there. Returning an `Err` instead would throw that
//! away and leave the pass judged on silence.
//!
//! # Every effect crosses `Capabilities`
//!
//! [`ScriptedDelegate`] holds a [`Capabilities`] bundle and has no field that
//! is a client. That is the whole discipline: there is no second way to reach a
//! model, so there is no call that can miss being recorded. An implementation
//! here that opened its own transport would be a defect, not an optimization.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Mutex, MutexGuard, PoisonError};

use tinyflows::caps::Capabilities;

mod types;

use types::refuse;
pub use types::{
    Artifact, Brief, DelegationOutcome, DropObserver, Ending, Mailbox, Note, Posted, Role,
    RoleGrant, SinkDrops, Status, Ticket, Tier, salvage,
};

use crate::{Caps, Error, Result, RunBudget};

/// A settling delegation.
///
/// Named so the signature of [`Delegate::settle`] reads as one type rather than
/// as four nested ones, and so the boxing is documented in exactly one place.
pub type Settling<'a> = Pin<Box<dyn Future<Output = Result<DelegationOutcome>> + Send + 'a>>;

/// Resolves a role name to the record that describes it.
///
/// Ordered, so a registry renders and iterates the same way every time. The
/// registry is the reason a call site can say "judge" instead of naming a
/// model, a temperature, and a tool list, and the reason changing what "judge"
/// means is one edit rather than a search.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoleRegistry {
    roles: BTreeMap<String, Role>,
}

impl RoleRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Declares a role under `name`.
    ///
    /// `caps` is an `Option` on purpose. A role declared with no caps at all is
    /// a role that runs on whatever budget it is handed, and the failure that
    /// produces is not overspending in the abstract: a role that reads a report
    /// and answers in four lines, given an investigation's budget,
    /// investigates. So the absence is rejected at declaration rather than
    /// defaulted.
    ///
    /// # Errors
    ///
    /// - [`Error::RoleWithoutCaps`] when `caps` is `None`.
    /// - [`Error::DuplicateRole`] when `name` is already declared. A name has
    ///   one meaning; replacing a role silently would make which one runs
    ///   depend on declaration order.
    /// - Whatever [`Role::new`] returns for an illegal set of caps.
    pub fn declare(
        &mut self,
        name: impl Into<String>,
        prompt: impl Into<String>,
        grant: RoleGrant,
        caps: Option<Caps>,
        tier: Tier,
    ) -> Result<()> {
        let name = name.into();
        let Some(caps) = caps else {
            return Err(Error::RoleWithoutCaps { role: name });
        };
        if self.roles.contains_key(&name) {
            return Err(Error::DuplicateRole { role: name });
        }
        let role = Role::new(prompt, grant, caps, tier)?;
        self.roles.insert(name, role);
        Ok(())
    }

    /// The role declared under `name`.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownRole`] when nothing is declared under `name`. An unknown
    /// role must never fall back to a default one: the run would proceed on a
    /// prompt, a grant, and a budget nobody chose, and every one of those is
    /// invisible from the outside.
    pub fn resolve(&self, name: &str) -> Result<&Role> {
        self.roles.get(name).ok_or_else(|| Error::UnknownRole {
            role: name.to_owned(),
        })
    }

    /// The declared role names, in order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.roles.keys().map(String::as_str)
    }

    /// How many roles are declared.
    #[must_use]
    pub fn len(&self) -> usize {
        self.roles.len()
    }

    /// Whether nothing is declared.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.roles.is_empty()
    }
}

/// Hands work to a role and collects it later.
///
/// Four operations, deliberately separate. `spawn` starts work and returns;
/// `peek` reports; `steer` sends an aside; `settle` collects. Nothing here both
/// starts and finishes work, and nothing may be added that does — see the
/// module documentation for the 33 minutes that rule is made of.
pub trait Delegate: std::fmt::Debug + Send + Sync {
    /// Starts `role` on `brief` and returns immediately.
    ///
    /// # Errors
    ///
    /// - [`Error::UnknownRole`] when the role is not declared.
    /// - [`Error::SpawnRefused`] when the implementation will not start it —
    ///   at capacity, shutting down, or out of scripted work.
    fn spawn(&self, role: &str, brief: Brief) -> Result<Ticket>;

    /// Reports where `ticket` has got to, without settling it.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownTicket`] when the ticket was not issued here.
    fn peek(&self, ticket: &Ticket) -> Result<Status>;

    /// Sends an aside to a running delegation.
    ///
    /// Returns [`Posted`] rather than `()` so a steer lost to a full mailbox is
    /// visible at the call site. The specification left this open; it is
    /// resolved toward visibility, because the case where steering matters most
    /// is exactly the case where the mailbox is under pressure.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownTicket`] when the ticket was not issued here.
    fn steer(&self, ticket: &Ticket, note: Note) -> Result<Posted>;

    /// Collects the outcome of `ticket`, waiting for it if it is still running.
    ///
    /// A second settle of the same ticket returns the same outcome: collecting
    /// is a read, not a consume, so two arms reading one delegation cannot race
    /// each other into seeing different results.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownTicket`] when the ticket was not issued here. A
    /// delegation that timed out, was capped, or failed is *not* an error: it
    /// is a [`DelegationOutcome`] whose [`Ending`] says so.
    fn settle<'a>(&'a self, ticket: &'a Ticket) -> Settling<'a>;
}

/// What the reference harness will do when a role is spawned.
///
/// A declared list, consumed in order, so "the same script produces the same
/// events on every run" is a property of the type rather than of the test that
/// checks it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scripted {
    /// It answers, and leaves `artifacts` behind.
    Answers {
        /// What it says.
        reply: String,
        /// What it leaves behind.
        artifacts: Vec<Artifact>,
    },
    /// It never completes, and is eventually collected as a timeout.
    ///
    /// [`Delegate::peek`] keeps reporting [`Status::Running`] for as long as
    /// anybody asks. [`Delegate::settle`] does not hang — a hanging test is a
    /// broken test — it returns the readable outcome the real timeout would
    /// have produced.
    NeverCompletes {
        /// What it managed to write before it stopped answering.
        artifacts: Vec<Artifact>,
    },
    /// Its own cap kills it, leaving `artifacts` and no reply.
    Capped {
        /// The files the cap did not take with it.
        artifacts: Vec<Artifact>,
    },
    /// It fails, saying why.
    Fails {
        /// The failure, as the next pass will read it.
        reason: String,
    },
}

/// The offline reference harness.
///
/// A role registry over deterministic stand-ins: each role is given a list of
/// [`Scripted`] outcomes, and the nth spawn of that role gets the nth entry.
/// Spawning past the end of a role's script is [`Error::SpawnRefused`] rather
/// than an improvised outcome, so a test that expected three delegations and
/// got four fails instead of quietly passing.
///
/// It holds a [`Capabilities`] bundle and no client of its own. Nothing in it
/// reads the wall clock, opens a socket, or requires a credential.
pub struct ScriptedDelegate {
    roles: RoleRegistry,
    caps: Capabilities,
    script: BTreeMap<String, Vec<Scripted>>,
    state: Mutex<BTreeMap<String, Delegation>>,
    mailbox_capacity: usize,
}

/// One in-flight delegation inside [`ScriptedDelegate`].
#[derive(Debug)]
struct Delegation {
    brief: Brief,
    scripted: Scripted,
    notes: Mailbox,
    settled: bool,
}

impl std::fmt::Debug for ScriptedDelegate {
    /// Renders everything but the capability bundle, which is not [`Debug`].
    ///
    /// The bundle is named rather than shown so the rendering still says that
    /// the delegate's effects go through it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptedDelegate")
            .field("roles", &self.roles)
            .field("script", &self.script)
            .field("state", &self.state)
            .field("mailbox_capacity", &self.mailbox_capacity)
            .field("caps", &"<Capabilities>")
            .finish()
    }
}

/// How many notes a delegation's mailbox holds by default.
///
/// Small on purpose. A steer queue deep enough to never drop is a queue deep
/// enough to deliver an instruction long after the moment it was about.
pub const DEFAULT_MAILBOX_CAPACITY: usize = 4;

impl ScriptedDelegate {
    /// Builds a reference harness over `roles`, reaching effects through
    /// `caps`.
    #[must_use]
    pub fn new(roles: RoleRegistry, caps: Capabilities) -> Self {
        Self {
            roles,
            caps,
            script: BTreeMap::new(),
            state: Mutex::new(BTreeMap::new()),
            mailbox_capacity: DEFAULT_MAILBOX_CAPACITY,
        }
    }

    /// Declares what the nth spawn of `role` will do.
    #[must_use]
    pub fn scripting(mut self, role: impl Into<String>, outcomes: Vec<Scripted>) -> Self {
        self.script.insert(role.into(), outcomes);
        self
    }

    /// Sets the capacity of each delegation's steer mailbox.
    #[must_use]
    pub fn with_mailbox_capacity(mut self, capacity: usize) -> Self {
        self.mailbox_capacity = capacity;
        self
    }

    /// The capability bundle every effect crosses.
    #[must_use]
    pub fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    /// The roles this harness can spawn.
    #[must_use]
    pub fn roles(&self) -> &RoleRegistry {
        &self.roles
    }

    /// The notes steered at `ticket` and not yet read by the delegation.
    ///
    /// Not part of [`Delegate`]: it is how a test proves a steer arrived, and a
    /// real harness delivers notes into the running agent instead.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownTicket`] when the ticket was not issued here.
    pub fn notes(&self, ticket: &Ticket) -> Result<Vec<Note>> {
        let state = self.lock();
        let delegation = Self::find(&state, ticket)?;
        Ok(delegation.notes.collect())
    }

    /// How many notes `ticket`'s mailbox has dropped.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownTicket`] when the ticket was not issued here.
    pub fn dropped_notes(&self, ticket: &Ticket) -> Result<usize> {
        let state = self.lock();
        Ok(Self::find(&state, ticket)?.notes.drops())
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, Delegation>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn find<'a>(
        state: &'a BTreeMap<String, Delegation>,
        ticket: &Ticket,
    ) -> Result<&'a Delegation> {
        state.get(ticket.id()).ok_or_else(|| Error::UnknownTicket {
            ticket: ticket.id().to_owned(),
        })
    }

    /// The budget the harness would run `role` under.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownRole`] when the role is not declared.
    pub fn budget_for(&self, role: &str) -> Result<RunBudget> {
        Ok(self.roles.resolve(role)?.budget())
    }

    /// The outcome `scripted` describes for `brief`.
    ///
    /// A re-export of [`Scripted::outcome`] under the name this type reads
    /// better with, and deliberately not a second implementation: the mapping
    /// from a script entry to an outcome is one fact, and the synchronous
    /// dispatcher in `orchestrate/` needs the same one.
    fn outcome(brief: Brief, scripted: &Scripted) -> DelegationOutcome {
        scripted.outcome(brief)
    }
}

impl Scripted {
    /// The outcome this script entry describes for `brief`.
    ///
    /// Public because the reference dispatcher in
    /// [`orchestrate`](crate::orchestrate_docs) collects specialists
    /// synchronously and must produce byte-identical outcomes to the ones
    /// [`ScriptedDelegate`] produces asynchronously. Two spellings of the same
    /// mapping is how a test starts agreeing with a bug.
    #[must_use]
    pub fn outcome(&self, brief: Brief) -> DelegationOutcome {
        match self {
            Self::Answers { reply, artifacts } => {
                DelegationOutcome::answered(brief, reply.clone()).with_artifacts(artifacts.clone())
            }
            Self::NeverCompletes { artifacts } => DelegationOutcome {
                brief,
                ending: Ending::TimedOut,
                artifacts: artifacts.clone(),
                reply: None,
            },
            Self::Capped { artifacts } => salvage(brief, artifacts.clone()),
            Self::Fails { reason } => DelegationOutcome {
                brief,
                ending: Ending::Failed,
                artifacts: Vec::new(),
                reply: Some(reason.clone()),
            },
        }
    }
}

impl Delegate for ScriptedDelegate {
    fn spawn(&self, role: &str, brief: Brief) -> Result<Ticket> {
        self.roles.resolve(role)?;
        let outcomes = self
            .script
            .get(role)
            .ok_or_else(|| refuse(role, "the script declares no outcomes for this role"))?;

        let mut state = self.lock();
        let prefix = format!("{role}#");
        let taken = state.keys().filter(|id| id.starts_with(&prefix)).count();
        let scripted = outcomes
            .get(taken)
            .ok_or_else(|| refuse(role, "the script declares no further outcomes"))?
            .clone();

        let ticket = Ticket::new(format!("{prefix}{taken}"));
        state.insert(
            ticket.id().to_owned(),
            Delegation {
                brief,
                scripted,
                notes: Mailbox::new(self.mailbox_capacity),
                settled: false,
            },
        );
        Ok(ticket)
    }

    fn peek(&self, ticket: &Ticket) -> Result<Status> {
        let state = self.lock();
        let delegation = Self::find(&state, ticket)?;
        if delegation.settled {
            return Ok(Status::Settled);
        }
        Ok(match delegation.scripted {
            Scripted::NeverCompletes { .. } => Status::Running,
            _ => Status::Ready,
        })
    }

    fn steer(&self, ticket: &Ticket, note: Note) -> Result<Posted> {
        let state = self.lock();
        Ok(Self::find(&state, ticket)?.notes.post(note))
    }

    fn settle<'a>(&'a self, ticket: &'a Ticket) -> Settling<'a> {
        Box::pin(async move {
            let mut state = self.lock();
            let delegation = state
                .get_mut(ticket.id())
                .ok_or_else(|| Error::UnknownTicket {
                    ticket: ticket.id().to_owned(),
                })?;
            delegation.settled = true;
            Ok(Self::outcome(
                delegation.brief.clone(),
                &delegation.scripted,
            ))
        })
    }
}

#[cfg(test)]
mod test;
