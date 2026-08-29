//! The rows a ledger holds, the evidence recorded against them, and the run
//! spec that is immutable for the run's duration.
//!
//! Fields are private throughout and read through accessors. That is not
//! ceremony: the whole point of [`EvidenceOrigin`] and of a criterion that
//! moves only on recorded evidence is that no caller can assign either, and a
//! public field is an assignment.

use serde::{Deserialize, Serialize};

use crate::tools::ToolReceipt;

/// Whether a piece of evidence was collected or merely handed over.
///
/// A transcript you were given is a claim; a transcript you collected is
/// evidence. Both are worth recording, and conflating them is how a run
/// concludes a test passed on the strength of somebody's assertion that it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceOrigin {
    /// Produced by the executing tool itself.
    Collected,
    /// Handed to the run by something else.
    Supplied,
}

impl EvidenceOrigin {
    /// The origin's wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Collected => "collected",
            Self::Supplied => "supplied",
        }
    }
}

/// One recorded observation.
///
/// [`Evidence::collected`] is the only way to get [`EvidenceOrigin::Collected`]
/// and it demands a [`ToolReceipt`], which only
/// [`ToolSet::invoke`](crate::tools::ToolSet::invoke) mints. There is no setter
/// and no public field, so nothing in the run can promote a claim to evidence
/// after the fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    origin: EvidenceOrigin,
    tool: Option<String>,
    text: String,
}

impl Evidence {
    /// Evidence the executing tool produced.
    ///
    /// The receipt is proof that the tool ran: it is minted inside the tool
    /// registry and has no public constructor.
    #[must_use]
    pub fn collected(receipt: &ToolReceipt, text: impl Into<String>) -> Self {
        Self {
            origin: EvidenceOrigin::Collected,
            tool: Some(receipt.tool().to_owned()),
            text: text.into(),
        }
    }

    /// A claim the run was handed.
    #[must_use]
    pub fn supplied(text: impl Into<String>) -> Self {
        Self {
            origin: EvidenceOrigin::Supplied,
            tool: None,
            text: text.into(),
        }
    }

    /// Where this observation came from.
    #[must_use]
    pub const fn origin(&self) -> EvidenceOrigin {
        self.origin
    }

    /// The tool that produced it, when one did.
    #[must_use]
    pub fn tool(&self) -> Option<&str> {
        self.tool.as_deref()
    }

    /// What was observed.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// Whether an entry is still being worked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryStatus {
    /// Still open.
    Open,
    /// Closed, with a reason recorded on the entry.
    Closed,
}

impl EntryStatus {
    /// The status's wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }
}

/// One row of the ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerEntry {
    id: String,
    title: String,
    note: String,
    status: EntryStatus,
    closed_reason: Option<String>,
    evidence: Vec<Evidence>,
}

impl LedgerEntry {
    /// A fresh, open entry.
    pub(crate) fn new(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            title: String::new(),
            note: String::new(),
            status: EntryStatus::Open,
            closed_reason: None,
            evidence: Vec::new(),
        }
    }

    /// Merges one event's fields into this entry.
    pub(crate) fn absorb(&mut self, event: LedgerEvent) {
        if let Some(title) = event.title {
            self.title = title;
        }
        if let Some(note) = event.note {
            self.note = note;
        }
        self.evidence.extend(event.evidence);
        if let Some(reason) = event.close {
            self.status = EntryStatus::Closed;
            self.closed_reason = Some(reason);
        }
    }

    /// The entry's identity.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// What the entry is about, in a few words.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The prose recorded against it.
    #[must_use]
    pub fn note(&self) -> &str {
        &self.note
    }

    /// Whether it is open or closed.
    #[must_use]
    pub const fn status(&self) -> EntryStatus {
        self.status
    }

    /// Why it was closed, when it is.
    ///
    /// Closing keeps the entry. A deleted entry is indistinguishable from an
    /// entry that never existed, and the next pass re-derives it.
    #[must_use]
    pub fn closed_reason(&self) -> Option<&str> {
        self.closed_reason.as_deref()
    }

    /// The evidence recorded against it.
    #[must_use]
    pub fn evidence(&self) -> &[Evidence] {
        &self.evidence
    }
}

/// The one write operation: an event names an entry and merges fields into it.
///
/// There is no delete. Closing is a field like any other, and it keeps the
/// entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LedgerEvent {
    id: String,
    title: Option<String>,
    note: Option<String>,
    evidence: Vec<Evidence>,
    close: Option<String>,
}

impl LedgerEvent {
    /// An event naming the entry it merges into.
    #[must_use]
    pub fn about(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            ..Self::default()
        }
    }

    /// The entry this event names.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Sets the entry's title.
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Sets the entry's note.
    #[must_use]
    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// Records one observation against the entry.
    #[must_use]
    pub fn evidence(mut self, evidence: Evidence) -> Self {
        self.evidence.push(evidence);
        self
    }

    /// Closes the entry, keeping it, with the reason recorded.
    #[must_use]
    pub fn close(mut self, reason: impl Into<String>) -> Self {
        self.close = Some(reason.into());
        self
    }
}

/// One thing the run must show before it is done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Criterion {
    id: String,
    text: String,
    satisfied: bool,
    evidence: Vec<Evidence>,
}

impl Criterion {
    /// The criterion's identity.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// What it demands.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether it has been shown.
    #[must_use]
    pub const fn satisfied(&self) -> bool {
        self.satisfied
    }

    /// The evidence recorded against it.
    #[must_use]
    pub fn evidence(&self) -> &[Evidence] {
        &self.evidence
    }

    /// Records one observation against this criterion.
    pub(crate) fn record(&mut self, evidence: Evidence) {
        self.evidence.push(evidence);
    }

    /// Marks it shown. Callable only from the run spec, which checks first.
    pub(crate) fn satisfy(&mut self) {
        self.satisfied = true;
    }
}

/// The run's goal and completion criteria, written once at configuration.
///
/// Immutable to the agent for the run's duration: there is no way to add a
/// criterion, edit one's text, or assign its verdict. An agent that can edit
/// its own completion criteria does not have completion criteria; it has a
/// preference. A criterion moves to `true` only through evidence recorded
/// against it, and every criterion starts `false`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSpec {
    goal: String,
    criteria: Vec<Criterion>,
}

impl RunSpec {
    /// Writes the spec once, with every criterion `false`.
    #[must_use]
    pub fn new(goal: impl Into<String>, criteria: &[(&str, &str)]) -> Self {
        Self {
            goal: goal.into(),
            criteria: criteria
                .iter()
                .map(|(id, text)| Criterion {
                    id: (*id).to_owned(),
                    text: (*text).to_owned(),
                    satisfied: false,
                    evidence: Vec::new(),
                })
                .collect(),
        }
    }

    /// What the run is for.
    #[must_use]
    pub fn goal(&self) -> &str {
        &self.goal
    }

    /// Every criterion, in the order it was written.
    #[must_use]
    pub fn criteria(&self) -> &[Criterion] {
        &self.criteria
    }

    /// One criterion by identity.
    #[must_use]
    pub fn criterion(&self, id: &str) -> Option<&Criterion> {
        self.criteria.iter().find(|criterion| criterion.id() == id)
    }

    /// Records evidence against a criterion.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownCriterion`](crate::Error::UnknownCriterion) when the
    /// spec holds no such criterion. A criterion cannot be added this way: the
    /// spec is written once.
    pub fn record(&mut self, id: &str, evidence: Evidence) -> crate::Result<()> {
        let criterion = self
            .criteria
            .iter_mut()
            .find(|criterion| criterion.id() == id)
            .ok_or_else(|| crate::Error::UnknownCriterion { id: id.to_owned() })?;
        criterion.record(evidence);
        Ok(())
    }

    /// Marks a criterion satisfied, on the strength of what was recorded.
    ///
    /// # Errors
    ///
    /// - [`Error::UnknownCriterion`](crate::Error::UnknownCriterion) when the
    ///   spec holds no such criterion.
    /// - [`Error::EvidenceNotRecorded`](crate::Error::EvidenceNotRecorded) when
    ///   nothing has been recorded against it. This is the whole difference
    ///   between a criterion and a preference.
    pub fn satisfy(&mut self, id: &str) -> crate::Result<()> {
        let criterion = self
            .criteria
            .iter_mut()
            .find(|criterion| criterion.id() == id)
            .ok_or_else(|| crate::Error::UnknownCriterion { id: id.to_owned() })?;
        if criterion.evidence().is_empty() {
            return Err(crate::Error::EvidenceNotRecorded { id: id.to_owned() });
        }
        criterion.satisfy();
        Ok(())
    }
}
