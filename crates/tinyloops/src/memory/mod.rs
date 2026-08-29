//! The memory seam: recall, remember, and compaction over an explicit scope.
//!
//! A loop that keeps nothing re-derives everything, and a loop that keeps
//! everything cannot fit it in a prompt. This module owns the middle: a store
//! the deployment supplies, a write path that checks the store actually kept
//! what it accepted, and a compaction that records what it stopped showing
//! instead of deleting it.
//!
//! Three contracts govern it, and each one exists because of a specific
//! observed failure.
//!
//! # Absence is `None` at wiring time, never an erroring stub
//!
//! A deployment without memory supplies no provider, and the loop reads that as
//! a capability it does not have. This matches the engine, where
//! `Capabilities::memory` is `Option<Arc<dyn MemoryProvider>>`. A stub that
//! accepts calls in order to fail them turns a wiring fact into a run-time
//! error, and the loop can no longer tell "no memory here" from "memory is
//! broken" — the first is a configuration, the second is an incident.
//! [`recall_where_available`] and [`remember_where_available`] return
//! [`Available`] for the first and `Err` for the second, so the two never
//! arrive down the same channel.
//!
//! # A write returning success is not a write
//!
//! [`Memory`] cannot be implemented without a read-back: [`Memory::fetch`] is a
//! required method, and the provided [`Memory::remember`] calls it after every
//! write whose scope has no unexpired verdict. One production run logged 193
//! successful `remember` calls and stored zero documents, because the backend
//! answered `200 {"status":"running"}` and dropped the work; every one of those
//! calls was reported as a success by the only signal available. "The store
//! accepted it" and "the store has it" are different observations, and only the
//! second is a write.
//!
//! The verdict is cached per scope with a [`ProbeCache`], so a burst of writes
//! into one scope costs one round trip rather than one each, and expires, so a
//! store that starts dropping writes mid-run is caught before the run ends.
//!
//! # Compaction is recorded, not destructive
//!
//! [`condense`] returns the view a pass should use and appends a
//! [`Condensation`] — `forgotten`, `summary`, `offset` — to the [`History`].
//! Nothing is removed, so what a pass stopped showing is still readable
//! afterwards by anything that walks the history, including a person debugging
//! the run.
//!
//! It is **idempotent**: condensing an already-condensed history returns the
//! same view and appends no second event. That is asserted, not assumed. The
//! hardest-to-diagnose harness regression on public record was a context
//! cleanup meant to run once that ran on every turn, producing both
//! forgetfulness and continuous prompt-cache misses, and it took over a week to
//! locate.
//!
//! It also honours a [`Pins`] set it may not touch. Policy-violation rate goes
//! from 0% to 30% after a compaction, measured 0% when the governing constraint
//! survived and 38% when it was dropped, and pinning the constraint restored 0%
//! for roughly 47 tokens.
//!
//! # The traits here are synchronous
//!
//! [`Memory`] is a synchronous trait. `async fn` in traits is native in Rust
//! 2024, but a trait containing one is not dyn-compatible, and this seam exists
//! to be held as `Option<Arc<dyn Memory>>` — the shape the engine's own memory
//! capability has. The alternative, a boxed future per method, is what
//! [`Delegate::settle`](crate::Delegate::settle) does, and it is worth it there
//! because delegation is *long*: the whole point of that seam is that a caller
//! does not wait. A recall is a lookup. What this defers is a memory backend
//! whose reads are slow enough to want overlapping with other work; such a
//! deployment implements [`Memory`] over its own runtime handle today, and
//! moving the trait to boxed futures later is an additive change to the same
//! four operations.
//!
//! # The clock is the caller's
//!
//! Nothing here reads the wall clock. [`Clock`] is injected, and
//! [`ManualClock`] is what the tests advance by hand, so a cache expiry is
//! asserted rather than slept through.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

mod types;

pub use types::{
    Available, Clock, Condensation, Condensed, History, ManualClock, Pins, ProbeCache, Record,
    Scope,
};

use crate::{Error, Result};

/// What a loop remembers, and where.
///
/// Four required operations, and two provided ones that are the contract. An
/// implementor supplies a store, a read-back, a search, a probe cache, and a
/// clock; it does *not* supply `remember`, because `remember` is where the
/// verification lives and an implementation free to define it is an
/// implementation free to skip it.
pub trait Memory: std::fmt::Debug + Send + Sync {
    /// Hands `record` to the backend.
    ///
    /// A successful return means the backend accepted the write. It does not
    /// mean the backend kept it — see [`Memory::remember`], which is the
    /// operation callers use.
    ///
    /// # Errors
    ///
    /// Whatever the backend reports.
    fn store(&self, scope: &Scope, record: &Record) -> Result<()>;

    /// Reads one record back by identifier.
    ///
    /// Required rather than optional: this is the probe, and a [`Memory`] that
    /// could not be read back is a [`Memory`] whose writes cannot be verified.
    ///
    /// # Errors
    ///
    /// Whatever the backend reports.
    fn fetch(&self, scope: &Scope, id: &str) -> Result<Option<Record>>;

    /// Finds records in `scope` matching `query`.
    ///
    /// # Errors
    ///
    /// Whatever the backend reports.
    fn search(&self, scope: &Scope, query: &str) -> Result<Vec<Record>>;

    /// This memory's per-scope probe verdicts.
    fn probes(&self) -> &ProbeCache;

    /// The clock the probe cache ages against.
    fn clock(&self) -> &dyn Clock;

    /// Recalls what `scope` holds about `query`.
    ///
    /// # Errors
    ///
    /// Whatever [`Memory::search`] reports.
    fn recall(&self, scope: &Scope, query: &str) -> Result<Vec<Record>> {
        self.search(scope, query)
    }

    /// Writes `record`, and does not call it written until a read-back agreed.
    ///
    /// The first write into a scope — and the first after its verdict expires —
    /// costs one extra round trip. Every write after that reads the cached
    /// verdict.
    ///
    /// # Errors
    ///
    /// - Whatever [`Memory::store`] or [`Memory::fetch`] reports.
    /// - [`Error::WriteNotDurable`] when the backend acknowledged the write and
    ///   the read-back came back empty or holding something else. This is the
    ///   193-writes-zero-documents failure, turned into a value the caller
    ///   cannot miss.
    fn remember(&self, scope: &Scope, record: &Record) -> Result<()> {
        self.store(scope, record)?;
        let now = self.clock().now();

        let durable = if let Some(cached) = self.probes().verdict(scope, now) {
            cached
        } else {
            let read_back = self.fetch(scope, record.id())?;
            let verdict = read_back.is_some_and(|held| held.body() == record.body());
            self.probes().record(scope, verdict, now);
            verdict
        };

        if durable {
            Ok(())
        } else {
            Err(Error::WriteNotDurable {
                scope: scope.as_str().to_owned(),
            })
        }
    }
}

/// Recalls through a memory the deployment may not have.
///
/// Returns [`Available::Absent`] when there is no provider, which is a wiring
/// fact, and an `Err` only when a provider that exists failed, which is an
/// incident. Keeping them apart is the whole reason this function is not just
/// `memory.unwrap().recall(..)`.
///
/// # Errors
///
/// Whatever [`Memory::recall`] reports, when a memory is wired at all.
///
/// # Examples
///
/// ```
/// # use tinyloops::{recall_where_available, Available, Scope};
/// let absent = recall_where_available(None, &Scope::new("run-1"), "anything")?;
/// assert!(absent.is_absent());
/// # Ok::<(), tinyloops::Error>(())
/// ```
pub fn recall_where_available(
    memory: Option<&Arc<dyn Memory>>,
    scope: &Scope,
    query: &str,
) -> Result<Available<Vec<Record>>> {
    match memory {
        None => Ok(Available::Absent),
        Some(memory) => Ok(Available::Present(memory.recall(scope, query)?)),
    }
}

/// Remembers through a memory the deployment may not have.
///
/// # Errors
///
/// Whatever [`Memory::remember`] reports, including
/// [`Error::WriteNotDurable`], when a memory is wired at all.
pub fn remember_where_available(
    memory: Option<&Arc<dyn Memory>>,
    scope: &Scope,
    record: &Record,
) -> Result<Available<()>> {
    match memory {
        None => Ok(Available::Absent),
        Some(memory) => {
            memory.remember(scope, record)?;
            Ok(Available::Present(()))
        }
    }
}

/// Compacts `history` down to its most recent `keep` records, and records it.
///
/// Returns the view a pass should show: every pinned record, in history order,
/// followed by the records after the cut. Applying it a second time with the
/// same arguments returns the same view and appends nothing, because the cut
/// starts from [`History::offset`] rather than from the beginning.
///
/// Pinned records survive untouched. They are never listed in
/// [`Condensation::forgotten`], and they appear in the view even when the cut
/// fell past them.
///
/// # Examples
///
/// ```
/// # use tinyloops::{condense, History, Pins, Record};
/// let mut history = History::new();
/// for n in 0..4 {
///     history.append(Record::new(format!("r{n}"), format!("note {n}")));
/// }
///
/// let first = condense(&mut history, &Pins::of(["r0"]), 2);
/// assert_eq!(first.view().len(), 3); // the pin, plus the two kept records
/// assert_eq!(history.condensations().len(), 1);
///
/// let second = condense(&mut history, &Pins::of(["r0"]), 2);
/// assert_eq!(second.view(), first.view());
/// assert!(second.is_noop());
/// assert_eq!(history.condensations().len(), 1);
///
/// // What it stopped showing is still there.
/// assert_eq!(history.record("r1").map(Record::body), Some("note 1"));
/// ```
#[must_use]
pub fn condense(history: &mut History, pins: &Pins, keep: usize) -> Condensed {
    let offset = history.offset();
    let live = history.len().saturating_sub(offset);

    if live <= keep {
        return Condensed::new(view(history, pins, offset), None);
    }

    let cut = live - keep;
    let new_offset = offset + cut;
    let forgotten: Vec<String> = history.entries()[offset..new_offset]
        .iter()
        .filter(|entry| !pins.contains(entry.id()))
        .map(|entry| entry.id().to_owned())
        .collect();
    let summary = format!(
        "condensed {} of {} records into the range [{offset}, {new_offset})",
        forgotten.len(),
        cut,
    );

    let condensation = Condensation {
        forgotten,
        summary,
        offset: new_offset,
    };
    history.record_condensation(condensation.clone());

    Condensed::new(view(history, pins, new_offset), Some(condensation))
}

/// The records a pass shows given a cut at `offset`.
///
/// Pinned records from before the cut come first, in history order, so a
/// governing constraint keeps its position relative to the rest rather than
/// being appended where the model reads it last.
fn view(history: &History, pins: &Pins, offset: usize) -> Vec<Record> {
    let entries = history.entries();
    entries[..offset.min(entries.len())]
        .iter()
        .filter(|entry| pins.contains(entry.id()))
        .chain(entries[offset.min(entries.len())..].iter())
        .cloned()
        .collect()
}

/// The offline reference memory: an in-process map that genuinely reads back.
///
/// Deterministic — the same calls produce the same history on every run — and
/// dependent on nothing outside the process. [`MapMemory::dropping`] is the
/// same store with one behavior changed: it acknowledges every write and
/// retains nothing, which is the backend the write-path probe exists to catch.
#[derive(Debug)]
pub struct MapMemory {
    scopes: Mutex<BTreeMap<String, BTreeMap<String, Record>>>,
    probes: ProbeCache,
    clock: Arc<dyn Clock>,
    retains: bool,
    fetches: Mutex<Vec<String>>,
}

impl MapMemory {
    /// A memory that keeps what it is given.
    #[must_use]
    pub fn new(clock: Arc<dyn Clock>, probes: ProbeCache) -> Self {
        Self {
            scopes: Mutex::new(BTreeMap::new()),
            probes,
            clock,
            retains: true,
            fetches: Mutex::new(Vec::new()),
        }
    }

    /// A memory that acknowledges every write and retains nothing.
    ///
    /// The `200 {"status":"running"}` backend, in twelve lines. It exists so
    /// the probe's failure path is exercised by a store that behaves the way
    /// the real one did, rather than by an error injected at the call site.
    #[must_use]
    pub fn dropping(clock: Arc<dyn Clock>, probes: ProbeCache) -> Self {
        Self {
            retains: false,
            ..Self::new(clock, probes)
        }
    }

    /// Every identifier this memory has been asked to read back, in order.
    ///
    /// The probe's call log. "A second write to the same scope costs no second
    /// read-back" is asserted against this rather than inferred from timing.
    #[must_use]
    pub fn fetches(&self) -> Vec<String> {
        self.fetches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// How many records `scope` actually holds.
    #[must_use]
    pub fn held(&self, scope: &Scope) -> usize {
        self.lock().get(scope.as_str()).map_or(0, BTreeMap::len)
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, BTreeMap<String, Record>>> {
        self.scopes.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Memory for MapMemory {
    fn store(&self, scope: &Scope, record: &Record) -> Result<()> {
        if self.retains {
            self.lock()
                .entry(scope.as_str().to_owned())
                .or_default()
                .insert(record.id().to_owned(), record.clone());
        }
        // And whether or not anything was kept, the write is acknowledged.
        Ok(())
    }

    fn fetch(&self, scope: &Scope, id: &str) -> Result<Option<Record>> {
        self.fetches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(format!("{}/{id}", scope.as_str()));
        Ok(self
            .lock()
            .get(scope.as_str())
            .and_then(|held| held.get(id))
            .cloned())
    }

    fn search(&self, scope: &Scope, query: &str) -> Result<Vec<Record>> {
        Ok(self
            .lock()
            .get(scope.as_str())
            .map(|held| {
                held.values()
                    .filter(|record| record.body().contains(query))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }

    fn probes(&self) -> &ProbeCache {
        &self.probes
    }

    fn clock(&self) -> &dyn Clock {
        self.clock.as_ref()
    }
}

#[cfg(test)]
mod test;
