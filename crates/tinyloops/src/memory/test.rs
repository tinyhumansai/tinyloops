//! Unit tests for the memory seam.
//!
//! Three of these are regression tests for failures that reported success while
//! they were happening:
//!
//! - a backend that answered `200 {"status":"running"}` and dropped the work,
//!   across 193 `remember` calls that stored zero documents;
//! - a context cleanup meant to run once that ran on every turn, producing both
//!   forgetfulness and continuous prompt-cache misses;
//! - a compaction that dropped a governing constraint, taking the
//!   policy-violation rate from 0% to 30%.
//!
//! Every one of them is asserted here against a store or a history built to
//! behave the way the real one did.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use super::*;

/// How long a probe verdict stands in these tests.
const TTL: Duration = Duration::from_secs(60);

/// A memory that keeps what it is given, on a clock the test advances.
fn keeping() -> (Arc<ManualClock>, MapMemory) {
    let clock = Arc::new(ManualClock::new());
    let memory = MapMemory::new(clock.clone(), ProbeCache::new(TTL));
    (clock, memory)
}

/// A memory that acknowledges every write and retains nothing.
fn dropping() -> (Arc<ManualClock>, MapMemory) {
    let clock = Arc::new(ManualClock::new());
    let memory = MapMemory::dropping(clock.clone(), ProbeCache::new(TTL));
    (clock, memory)
}

fn scope() -> Scope {
    Scope::new("run-1")
}

#[test]
fn recall_and_remember_take_an_explicit_scope() {
    let (_clock, memory) = keeping();
    let here = Scope::new("run-1");
    let elsewhere = Scope::new("run-2");

    memory
        .remember(&here, &Record::new("r0", "the failing test is in state/"))
        .unwrap();

    assert_eq!(memory.recall(&here, "failing").unwrap().len(), 1);
    // The same query against another scope reads nothing: the scope is the
    // caller's, not the handle's.
    assert!(memory.recall(&elsewhere, "failing").unwrap().is_empty());
    assert_eq!(memory.held(&here), 1);
    assert_eq!(memory.held(&elsewhere), 0);
}

#[test]
fn a_deployment_without_memory_wires_none() {
    let absent: Option<&Arc<dyn Memory>> = None;

    let recalled = recall_where_available(absent, &scope(), "anything").unwrap();
    assert!(recalled.is_absent());
    assert_eq!(recalled.into_option(), None);

    let remembered =
        remember_where_available(absent, &scope(), &Record::new("r0", "note")).unwrap();
    assert!(remembered.is_absent());
}

#[test]
fn the_loop_under_none_is_covered_separately_from_the_store_error_path() {
    // Wiring fact: no provider. Not an error, and nothing was attempted.
    let absent: Option<&Arc<dyn Memory>> = None;
    assert!(
        remember_where_available(absent, &scope(), &Record::new("r0", "note"))
            .unwrap()
            .is_absent()
    );

    // Incident: a provider that is here and broken. An `Err`, down a different
    // channel, so the loop can tell "no memory here" from "memory is broken".
    let (_clock, memory) = dropping();
    let broken: Arc<dyn Memory> = Arc::new(memory);
    assert_eq!(
        remember_where_available(Some(&broken), &scope(), &Record::new("r0", "note")).unwrap_err(),
        Error::WriteNotDurable {
            scope: "run-1".to_owned()
        },
    );

    // And a wired, working provider is `Present`.
    let (_clock, working) = keeping();
    let wired: Arc<dyn Memory> = Arc::new(working);
    assert_eq!(
        remember_where_available(Some(&wired), &scope(), &Record::new("r0", "note")).unwrap(),
        Available::Present(()),
    );
    assert_eq!(
        recall_where_available(Some(&wired), &scope(), "note")
            .unwrap()
            .into_option()
            .unwrap()
            .len(),
        1,
    );
}

#[test]
fn a_store_that_accepts_every_write_and_retains_nothing_fails_the_probe() {
    let (_clock, memory) = dropping();
    let record = Record::new("r0", "the note that was never stored");

    // The backend acknowledges the write, exactly as the production one did.
    assert!(memory.store(&scope(), &record).is_ok());

    // And `remember` — the operation callers use — refuses to call it written.
    assert_eq!(
        memory.remember(&scope(), &record).unwrap_err(),
        Error::WriteNotDurable {
            scope: "run-1".to_owned()
        },
    );
    assert_eq!(memory.held(&scope()), 0);
    assert_eq!(memory.fetches(), vec!["run-1/r0".to_owned()]);
}

/// A backend that keeps a *different* body than the one it was handed.
///
/// The other half of the write-path failure: not a store that keeps nothing,
/// but one that acknowledges a write and retains something else. The read-back
/// is what tells the two apart from a success.
#[derive(Debug)]
struct Truncating {
    held: Mutex<BTreeMap<String, Record>>,
    probes: ProbeCache,
    clock: Arc<ManualClock>,
}

impl Memory for Truncating {
    fn store(&self, _scope: &Scope, record: &Record) -> Result<()> {
        let clipped = Record::new(record.id(), &record.body()[..4]);
        self.held
            .lock()
            .unwrap()
            .insert(record.id().to_owned(), clipped);
        Ok(())
    }

    fn fetch(&self, _scope: &Scope, id: &str) -> Result<Option<Record>> {
        Ok(self.held.lock().unwrap().get(id).cloned())
    }

    fn search(&self, _scope: &Scope, _query: &str) -> Result<Vec<Record>> {
        Ok(self.held.lock().unwrap().values().cloned().collect())
    }

    fn probes(&self) -> &ProbeCache {
        &self.probes
    }

    fn clock(&self) -> &dyn Clock {
        self.clock.as_ref()
    }
}

#[test]
fn a_store_that_retains_something_else_fails_the_probe() {
    let memory = Truncating {
        held: Mutex::new(BTreeMap::new()),
        probes: ProbeCache::new(TTL),
        clock: Arc::new(ManualClock::new()),
    };

    // The read-back finds the id and disagrees about the body, which is a
    // silently lost write wearing a successful one's clothes.
    assert_eq!(
        memory
            .remember(&scope(), &Record::new("r0", "what the caller wrote"))
            .unwrap_err(),
        Error::WriteNotDurable {
            scope: "run-1".to_owned()
        },
    );
    assert_eq!(memory.recall(&scope(), "").unwrap().len(), 1);
}

#[test]
fn the_probe_verdict_is_cached_per_scope() {
    let (_clock, memory) = keeping();

    memory
        .remember(&scope(), &Record::new("r0", "first"))
        .unwrap();
    memory
        .remember(&scope(), &Record::new("r1", "second"))
        .unwrap();
    memory
        .remember(&scope(), &Record::new("r2", "third"))
        .unwrap();

    // One read-back for three writes into the scope.
    assert_eq!(memory.fetches(), vec!["run-1/r0".to_owned()]);

    // A different scope has its own verdict, so it probes once too.
    memory
        .remember(&Scope::new("run-2"), &Record::new("r0", "elsewhere"))
        .unwrap();
    assert_eq!(
        memory.fetches(),
        vec!["run-1/r0".to_owned(), "run-2/r0".to_owned()],
    );
}

#[test]
fn the_cached_verdict_expires() {
    let (clock, memory) = keeping();

    memory
        .remember(&scope(), &Record::new("r0", "first"))
        .unwrap();
    memory
        .remember(&scope(), &Record::new("r1", "second"))
        .unwrap();
    assert_eq!(memory.fetches().len(), 1);

    // Advanced by hand, not slept through: the assertion is about the cache,
    // not about how fast this machine is.
    clock.advance(TTL);
    memory
        .remember(&scope(), &Record::new("r2", "third"))
        .unwrap();
    assert_eq!(
        memory.fetches(),
        vec!["run-1/r0".to_owned(), "run-1/r2".to_owned()],
    );
    assert_eq!(memory.probes().ttl(), TTL);
}

#[test]
fn a_failed_verdict_stands_until_it_expires() {
    let (clock, memory) = dropping();

    for id in ["r0", "r1"] {
        assert!(memory.remember(&scope(), &Record::new(id, "note")).is_err());
    }
    // The second write reads the cached failure rather than probing again.
    assert_eq!(memory.fetches(), vec!["run-1/r0".to_owned()]);

    clock.advance(TTL);
    assert!(
        memory
            .remember(&scope(), &Record::new("r2", "note"))
            .is_err()
    );
    assert_eq!(memory.fetches().len(), 2);
}

#[test]
fn clearing_the_cache_makes_the_next_write_probe_again() {
    let (_clock, memory) = keeping();
    memory
        .remember(&scope(), &Record::new("r0", "first"))
        .unwrap();
    memory.probes().clear();
    memory
        .remember(&scope(), &Record::new("r1", "second"))
        .unwrap();
    assert_eq!(memory.fetches().len(), 2);
    assert_eq!(ProbeCache::default().ttl(), Duration::from_secs(60));
    assert_eq!(
        memory
            .probes()
            .verdict(&Scope::new("nothing"), Duration::ZERO),
        None
    );
}

#[test]
fn the_reference_probe_genuinely_reads_back() {
    // Not a constant `true`: the same code path returns a different answer for
    // a store built to drop, which is the only way to prove the probe reads.
    let (_clock, keeps) = keeping();
    let (_clock2, drops) = dropping();
    let record = Record::new("r0", "note");

    assert!(keeps.remember(&scope(), &record).is_ok());
    assert!(drops.remember(&scope(), &record).is_err());
    assert_eq!(keeps.fetches().len(), 1);
    assert_eq!(drops.fetches().len(), 1);
    assert_eq!(keeps.fetch(&scope(), "missing").unwrap(), None);
}

#[test]
fn the_same_calls_produce_the_same_history_on_every_run() {
    fn run() -> (Vec<Record>, Vec<String>) {
        let (_clock, memory) = keeping();
        for n in 0..3 {
            memory
                .remember(&scope(), &Record::new(format!("r{n}"), format!("note {n}")))
                .unwrap();
        }
        (memory.recall(&scope(), "note").unwrap(), memory.fetches())
    }

    assert_eq!(run(), run());
}

#[test]
fn condensing_twice_returns_the_same_view_and_appends_one_event() {
    let mut history = History::new();
    for n in 0..6 {
        history.append(Record::new(format!("r{n}"), format!("note {n}")));
    }

    let first = condense(&mut history, &Pins::none(), 2);
    assert!(!first.is_noop());
    assert_eq!(history.condensations().len(), 1);
    assert_eq!(history.offset(), 4);

    let second = condense(&mut history, &Pins::none(), 2);
    assert_eq!(second.view(), first.view());
    assert!(second.is_noop());
    assert_eq!(second.condensation(), None);
    assert_eq!(history.condensations().len(), 1);
}

#[test]
fn what_a_pass_stopped_showing_is_still_readable_from_the_history() {
    let mut history = History::new();
    for n in 0..4 {
        history.append(Record::new(format!("r{n}"), format!("note {n}")));
    }

    let condensed = condense(&mut history, &Pins::none(), 1);
    let event = condensed.condensation().unwrap();
    assert_eq!(event.forgotten, vec!["r0", "r1", "r2"]);
    assert_eq!(event.offset, 3);
    assert!(event.summary.contains("[0, 3)"), "{}", event.summary);

    // The view stopped showing them. The history did not lose them.
    assert_eq!(condensed.view().len(), 1);
    for n in 0..4 {
        assert_eq!(
            history.record(&format!("r{n}")).map(Record::body),
            Some(format!("note {n}").as_str()),
        );
    }
    assert_eq!(history.len(), 4);
    assert!(!history.is_empty());
    assert!(history.record("nothing").is_none());
}

#[test]
fn a_pinned_policy_constraint_survives_a_compaction_below_its_own_size() {
    let mut history = History::new();
    history.append(Record::new("policy", "never write outside the workspace"));
    for n in 0..5 {
        history.append(Record::new(format!("r{n}"), format!("note {n}")));
    }

    // Keep one record: the cut falls well past the constraint.
    let condensed = condense(&mut history, &Pins::of(["policy"]), 1);
    let ids: Vec<&str> = condensed.view().iter().map(Record::id).collect();
    assert_eq!(ids, vec!["policy", "r4"]);

    // And the pin is never listed as forgotten, because it was not forgotten.
    let event = condensed.condensation().unwrap();
    assert!(!event.forgotten.contains(&"policy".to_owned()));
    assert_eq!(event.forgotten.len(), 4);

    // Idempotent with a pin in play, too.
    let again = condense(&mut history, &Pins::of(["policy"]), 1);
    assert_eq!(again.view(), condensed.view());
    assert!(again.is_noop());
}

#[test]
fn a_history_shorter_than_the_bound_is_left_alone() {
    let mut history = History::new();
    history.append(Record::new("r0", "note"));

    let condensed = condense(&mut history, &Pins::none(), 4);
    assert!(condensed.is_noop());
    assert_eq!(condensed.view().len(), 1);
    assert_eq!(history.offset(), 0);
    assert!(history.condensations().is_empty());

    let empty = condense(&mut History::new(), &Pins::none(), 0);
    assert!(empty.view().is_empty());
    assert!(empty.is_noop());
}

#[test]
fn a_pin_set_says_what_it_holds() {
    let pins = Pins::of(["policy", "goal"]);
    assert!(pins.contains("policy"));
    assert!(!pins.contains("r0"));
    assert_eq!(pins.len(), 2);
    assert!(!pins.is_empty());
    assert!(Pins::none().is_empty());
    assert_eq!(Pins::none().len(), 0);
}

#[test]
fn the_manual_clock_moves_only_when_something_moves_it() {
    let clock = ManualClock::new();
    assert_eq!(clock.now(), Duration::ZERO);
    clock.advance(Duration::from_secs(5));
    clock.advance(Duration::from_secs(5));
    assert_eq!(clock.now(), Duration::from_secs(10));
}

#[test]
fn the_wire_form_is_pinned() {
    let record = Record::new("r0", "note");
    assert_eq!(
        serde_json::to_string(&record).unwrap(),
        r#"{"id":"r0","body":"note"}"#,
    );
    assert_eq!(
        serde_json::from_str::<Record>(r#"{"id":"r0","body":"note"}"#).unwrap(),
        record,
    );
    assert_eq!(
        serde_json::to_string(&Scope::new("run-1")).unwrap(),
        r#""run-1""#,
    );
    assert_eq!(
        serde_json::to_string(&Pins::of(["policy"])).unwrap(),
        r#"["policy"]"#,
    );

    let condensation = Condensation {
        forgotten: vec!["r0".to_owned()],
        summary: "s".to_owned(),
        offset: 1,
    };
    let json = serde_json::to_string(&condensation).unwrap();
    assert_eq!(json, r#"{"forgotten":["r0"],"summary":"s","offset":1}"#);
    assert_eq!(
        serde_json::from_str::<Condensation>(&json).unwrap(),
        condensation,
    );

    let mut history = History::new();
    history.append(record);
    let json = serde_json::to_string(&history).unwrap();
    assert_eq!(serde_json::from_str::<History>(&json).unwrap(), history);
}

#[test]
fn the_write_failure_renders_readably() {
    assert_eq!(
        Error::WriteNotDurable {
            scope: "run-1".to_owned()
        }
        .to_string(),
        "write to scope run-1 was acknowledged but not retained",
    );
}
