//! The records the memory seam moves around: where something is remembered,
//! what is remembered, what compaction did, and the cached verdict that decides
//! whether a write was a write.
//!
//! Data plus the arithmetic that keeps it honest. The [`Memory`](super::Memory)
//! trait and the one offline implementation of it live in the module root.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Where a record is remembered.
///
/// Explicit at every call rather than implied by the handle, so a recall can
/// never quietly read a different run's memory than the remember that preceded
/// it. A scope carried inside the provider is a scope nobody at the call site
/// can see.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Scope {
    name: String,
}

impl Scope {
    /// A scope named `name`.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }

    /// The scope's name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.name
    }
}

/// One remembered thing.
///
/// An identifier and a body, and nothing else. The identifier is what the
/// write-path probe reads back and what a condensation names when it stops
/// showing the record, so it has to be the caller's rather than the store's:
/// an id assigned by the backend is an id the caller cannot use to check that
/// the backend kept anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    id: String,
    body: String,
}

impl Record {
    /// A record with `id` holding `body`.
    #[must_use]
    pub fn new(id: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            body: body.into(),
        }
    }

    /// The record's identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// What the record says.
    #[must_use]
    pub fn body(&self) -> &str {
        &self.body
    }
}

/// The record identifiers compaction may not touch.
///
/// The cheapest correctness measure in the loop, and the numbers say so:
/// policy-violation rate goes from 0% to 30% after a compaction, measured 0%
/// when the governing constraint survived and 38% when it was dropped, and
/// pinning the constraint restored 0% for roughly 47 tokens. Forty-seven tokens
/// is less than the sentence explaining what went wrong without them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Pins {
    ids: BTreeSet<String>,
}

impl Pins {
    /// Nothing is pinned.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Pins the named records.
    #[must_use]
    pub fn of<I, S>(ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            ids: ids.into_iter().map(Into::into).collect(),
        }
    }

    /// Whether `id` is pinned.
    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.ids.contains(id)
    }

    /// How many records are pinned.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// Whether nothing is pinned.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

/// What one compaction did.
///
/// Appended to the history rather than replacing it. A person debugging a run
/// six weeks later can read which records a pass stopped showing, what stood in
/// for them, and where the cut fell — none of which survives a compaction that
/// simply deletes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Condensation {
    /// The identifiers this compaction stopped showing.
    ///
    /// Never includes a pinned record: pins are not forgotten, so they are not
    /// listed as forgotten either.
    pub forgotten: Vec<String>,
    /// What stands in for them in the view.
    pub summary: String,
    /// How far into the history the compaction has now cut.
    ///
    /// Everything before this index is condensed. The next compaction starts
    /// from here, which is what makes running one twice do nothing the second
    /// time.
    pub offset: usize,
}

/// Everything remembered in one scope, and everything compaction did to it.
///
/// Append-only in both halves. A history that loses entries when it is
/// compacted cannot answer "what did the pass stop showing", which is the one
/// question a compaction bug produces.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    entries: Vec<Record>,
    condensations: Vec<Condensation>,
}

impl History {
    /// An empty history.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends `record`.
    pub fn append(&mut self, record: Record) {
        self.entries.push(record);
    }

    /// Every record ever appended, condensed or not.
    #[must_use]
    pub fn entries(&self) -> &[Record] {
        &self.entries
    }

    /// The compactions applied so far, in order.
    #[must_use]
    pub fn condensations(&self) -> &[Condensation] {
        &self.condensations
    }

    /// The record with `id`, whether or not a compaction stopped showing it.
    #[must_use]
    pub fn record(&self, id: &str) -> Option<&Record> {
        self.entries.iter().find(|entry| entry.id() == id)
    }

    /// How far compaction has cut into the history.
    #[must_use]
    pub fn offset(&self) -> usize {
        self.condensations.last().map_or(0, |last| last.offset)
    }

    /// How many records the history holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing has been appended.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Records a compaction.
    pub(super) fn record_condensation(&mut self, condensation: Condensation) {
        self.condensations.push(condensation);
    }
}

/// The result of a compaction: the view to use, and what was recorded.
///
/// `condensation` is `None` when the history was already condensed far enough,
/// which is exactly what makes compaction idempotent — the second call returns
/// the same view and appends nothing. The hardest-to-diagnose harness
/// regression on public record was a context cleanup meant to run once that ran
/// on every turn; it produced both forgetfulness and continuous prompt-cache
/// misses, and it took over a week to find.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Condensed {
    view: Vec<Record>,
    condensation: Option<Condensation>,
}

impl Condensed {
    /// Builds a result.
    pub(super) fn new(view: Vec<Record>, condensation: Option<Condensation>) -> Self {
        Self { view, condensation }
    }

    /// The records a pass should show.
    #[must_use]
    pub fn view(&self) -> &[Record] {
        &self.view
    }

    /// What this call recorded, if it recorded anything.
    #[must_use]
    pub fn condensation(&self) -> Option<&Condensation> {
        self.condensation.as_ref()
    }

    /// Whether this call changed anything.
    #[must_use]
    pub fn is_noop(&self) -> bool {
        self.condensation.is_none()
    }
}

/// A capability the deployment may not have.
///
/// The distinction this type exists for is between "there is no memory here"
/// and "memory is broken". A stub that accepts a call in order to fail it
/// collapses the two into one error, and the loop can no longer tell a wiring
/// fact from a run-time failure — so absence is a value, and failure is an
/// [`Err`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Available<T> {
    /// The deployment does not have it.
    Absent,
    /// It is here, and this is what it said.
    Present(T),
}

impl<T> Available<T> {
    /// Whether the deployment lacks the capability.
    #[must_use]
    pub fn is_absent(&self) -> bool {
        matches!(self, Self::Absent)
    }

    /// What it said, or `None` when it is not there.
    #[must_use]
    pub fn into_option(self) -> Option<T> {
        match self {
            Self::Absent => None,
            Self::Present(value) => Some(value),
        }
    }
}

/// The passage of time, as the caller measures it.
///
/// Injected rather than read, for the same reason nothing in `observe` calls
/// [`Instant::now`](std::time::Instant::now): a cache expiry asserted against
/// the wall clock is a test that fails on a slow machine and passes on a fast
/// one. A deployment implements this over its own time source.
pub trait Clock: std::fmt::Debug + Send + Sync {
    /// How long the run has been going.
    fn now(&self) -> Duration;
}

/// A clock a caller advances by hand.
///
/// The reference implementation, and what every test here uses. Time moves
/// because something moved it.
#[derive(Debug, Default)]
pub struct ManualClock {
    elapsed: Mutex<Duration>,
}

impl ManualClock {
    /// A clock reading zero.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Moves the clock forward by `step`.
    pub fn advance(&self, step: Duration) {
        let mut elapsed = self.lock();
        *elapsed = elapsed.saturating_add(step);
    }

    fn lock(&self) -> MutexGuard<'_, Duration> {
        self.elapsed.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Duration {
        *self.lock()
    }
}

/// One scope's cached answer to "does this store keep what it accepts".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Verdict {
    durable: bool,
    at: Duration,
}

/// Remembers, per scope, whether a read-back confirmed a write.
///
/// The probe costs a round trip, so it is not paid on every write; the verdict
/// is not permanent, so a store that starts dropping writes mid-run is noticed
/// before the run ends. The lifetime is the caller's choice between those two,
/// and it is a choice rather than a constant because the right answer depends
/// on how long the run is.
#[derive(Debug)]
pub struct ProbeCache {
    ttl: Duration,
    verdicts: Mutex<BTreeMap<String, Verdict>>,
}

impl ProbeCache {
    /// A cache whose verdicts expire after `ttl`.
    #[must_use]
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            verdicts: Mutex::new(BTreeMap::new()),
        }
    }

    /// How long a verdict stands.
    #[must_use]
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// The unexpired verdict for `scope`, if there is one.
    #[must_use]
    pub fn verdict(&self, scope: &Scope, now: Duration) -> Option<bool> {
        let verdicts = self.lock();
        let verdict = verdicts.get(scope.as_str())?;
        if now.saturating_sub(verdict.at) >= self.ttl {
            return None;
        }
        Some(verdict.durable)
    }

    /// Records the verdict a probe reached for `scope`.
    pub fn record(&self, scope: &Scope, durable: bool, now: Duration) {
        self.lock()
            .insert(scope.as_str().to_owned(), Verdict { durable, at: now });
    }

    /// Forgets every verdict, so the next write probes again.
    pub fn clear(&self) {
        self.lock().clear();
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, Verdict>> {
        self.verdicts.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Default for ProbeCache {
    /// A cache whose verdicts stand for a minute.
    ///
    /// Long enough that a burst of writes into one scope pays for one probe,
    /// short enough that a store which starts dropping writes is caught within
    /// the same run rather than at the end of it.
    fn default() -> Self {
        Self::new(Duration::from_secs(60))
    }
}
