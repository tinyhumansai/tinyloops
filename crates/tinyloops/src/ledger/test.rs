//! Unit tests for the ledger.
//!
//! The bounds here are asserted rather than intended, and that distinction is
//! the point of the file. A ledger whose table was bounded and whose prose
//! sections were not grew to 86 KB and became a third of one prompt before
//! anybody counted: the table's bound was real, and nobody had asserted the
//! file's. So the absurd fixture below renders far more than any real run could
//! produce and the rendered size is checked against a documented constant.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;

use super::*;
use crate::tools::{ToolGrant, ToolInvocation, ToolSet};

/// A ledger holding one open and one closed entry, both with prose.
fn small_ledger() -> Ledger {
    let mut ledger = Ledger::new();
    ledger.merge(
        LedgerEvent::about("finding-1")
            .title("the judge never fires")
            .note("the terminal condition evaluated to null")
            .evidence(Evidence::supplied("a colleague said so")),
    );
    ledger.merge(
        LedgerEvent::about("finding-2")
            .title("the budget was never validated")
            .note("two caps could each trip first")
            .close("fixed in the constructor"),
    );
    ledger
}

/// A ledger far larger than any real run, with prose far longer than any note.
fn absurd_ledger(entries: usize) -> Ledger {
    let prose = "x".repeat(4_096);
    let mut ledger = Ledger::new();
    for index in 0..entries {
        let mut event = LedgerEvent::about(&format!("entry-{index:04}"))
            .title(prose.clone())
            .note(prose.clone())
            .evidence(Evidence::supplied(prose.clone()));
        if index % 2 == 0 {
            event = event.close(prose.clone());
        }
        ledger.merge(event);
    }
    ledger
}

/// The rendered sections, split on their headings.
fn sections(rendered: &str) -> Vec<&str> {
    rendered.split("\n## ").skip(1).collect()
}

#[test]
fn an_event_names_an_entry_and_merges_fields_into_it() {
    let mut ledger = Ledger::new();

    ledger.merge(LedgerEvent::about("finding-1").title("the judge never fires"));
    ledger.merge(LedgerEvent::about("finding-1").note("the condition was null"));

    let entry = ledger.entry("finding-1").unwrap();
    assert_eq!(entry.id(), "finding-1");
    assert_eq!(entry.title(), "the judge never fires");
    assert_eq!(entry.note(), "the condition was null");
    assert_eq!(entry.status(), EntryStatus::Open);
    assert_eq!(entry.closed_reason(), None);
    assert_eq!(ledger.len(), 1);
    assert!(!ledger.is_empty());
}

#[test]
fn closing_an_entry_leaves_it_present_with_its_reason() {
    let mut ledger = small_ledger();

    ledger.merge(LedgerEvent::about("finding-1").close("no longer reproducible"));

    let entry = ledger.entry("finding-1").unwrap();
    assert_eq!(entry.status(), EntryStatus::Closed);
    assert_eq!(entry.closed_reason(), Some("no longer reproducible"));
    assert_eq!(entry.title(), "the judge never fires");
}

#[test]
fn no_operation_removes_an_entry() {
    let mut ledger = small_ledger();
    let before = ledger.len();

    // Closing is the nearest thing the surface has to a delete, and it keeps
    // the entry: a deleted entry is indistinguishable from one that never
    // existed, and the next pass re-derives it.
    ledger.merge(LedgerEvent::about("finding-1").close("done"));
    ledger.merge(LedgerEvent::about("finding-2").close("done"));

    assert_eq!(ledger.len(), before);
    assert_eq!(ledger.with_status(EntryStatus::Open).len(), 0);
    assert_eq!(ledger.with_status(EntryStatus::Closed).len(), 2);
    assert_eq!(ledger.entries().len(), before);
}

#[test]
fn an_unknown_entry_is_an_error_rather_than_an_empty_row() {
    assert_eq!(
        Ledger::new().entry("finding-9").unwrap_err(),
        Error::UnknownEntry {
            id: "finding-9".to_owned()
        }
    );
    assert!(Ledger::new().is_empty());
}

#[test]
fn every_section_states_its_omission_count_and_its_fetch_call() {
    let rendered = render(&absurd_ledger(60));

    let sections = sections(&rendered);
    assert_eq!(sections.len(), 4, "open, closed, notes, evidence");
    for section in sections {
        let heading = section.lines().next().unwrap();
        assert!(
            section.contains("omitted — fetch"),
            "section {heading} does not say what it left out or where the rest is"
        );
        assert!(
            section.contains("_showing "),
            "section {heading} does not say how much it showed"
        );
    }
}

#[test]
fn an_absurd_fixture_renders_within_the_documented_bound() {
    let modest = render(&absurd_ledger(30));
    let absurd = render(&absurd_ledger(300));

    assert!(
        absurd.len() <= MAX_RENDERED_BYTES,
        "300 entries of four-kilobyte prose rendered {} bytes",
        absurd.len()
    );
    assert!(modest.len() <= MAX_RENDERED_BYTES);
    // Past the bound, more entries do not mean more file: the only difference
    // between these two is the number of omissions each section counts.
    assert!(
        absurd.len().abs_diff(modest.len()) < 64,
        "the file grew with the ledger: {} against {}",
        absurd.len(),
        modest.len()
    );
}

#[test]
fn a_section_shows_everything_when_there_is_little_to_show() {
    let rendered = render(&small_ledger());

    assert!(rendered.contains("the judge never fires"));
    assert!(rendered.contains("_showing 1 of 1; 0 omitted"));
    assert!(rendered.contains("· supplied · a colleague said so"));
}

#[test]
fn a_prompt_carries_the_index_never_the_ledger() {
    let ledger = absurd_ledger(50);

    let index = index(&ledger);

    assert_eq!(index.lines().count(), MAX_INDEX_ROWS + 1);
    assert!(index.contains("entry-0000 · closed"));
    assert!(index.ends_with("fetch an entry with `ledger.entry(id)`_\n"));
    assert!(
        !index.contains("xxxx"),
        "the index carries identity and status, never the prose"
    );
    assert!(index.len() < render(&ledger).len());
}

#[test]
fn a_direct_write_anywhere_inside_a_ledger_folder_is_refused() {
    for path in [
        "derived/ledger.md",
        "derived/a-name-nobody-has-ever-used.json",
        "reports/derived/nested.md",
    ] {
        assert_eq!(
            refuse_derived(path).unwrap_err(),
            Error::DerivedWrite {
                path: path.to_owned()
            },
            "the refusal is by folder, not by filename"
        );
    }
    assert!(refuse_derived("notes/finding.md").is_ok());
    assert_eq!(DERIVED_FOLDER, "derived");
}

#[test]
fn an_evidence_record_from_supplied_text_reports_supplied() {
    let claim = Evidence::supplied("the test passed, honest");

    assert_eq!(claim.origin(), EvidenceOrigin::Supplied);
    assert_eq!(claim.tool(), None);
    assert_eq!(claim.text(), "the test passed, honest");
}

#[test]
fn no_public_api_promotes_supplied_to_collected() {
    // `Evidence::collected` demands a `ToolReceipt`, and a receipt exists only
    // where a tool actually ran: it is minted inside `ToolSet::invoke` and has
    // no public constructor. A transcript the run was handed cannot acquire
    // one, and `Evidence` has no setter that could change its mind afterwards.
    let tools = ToolSet::new(ToolGrant::all());
    let outcome = tools
        .invoke(&ToolInvocation::new(
            "call-1",
            "read",
            json!({ "path": "plan.md" }),
        ))
        .unwrap();

    let collected = Evidence::collected(outcome.receipt(), outcome.content.clone());
    let supplied = Evidence::supplied(outcome.content.clone());

    assert_eq!(collected.origin(), EvidenceOrigin::Collected);
    assert_eq!(collected.tool(), Some("read"));
    assert_eq!(supplied.origin(), EvidenceOrigin::Supplied);
    assert_ne!(collected, supplied);
}

#[test]
fn a_criterion_moves_to_true_only_through_recorded_evidence() {
    let mut spec = RunSpec::new("make the loop stop", &[("c1", "the judge fires")]);

    assert_eq!(spec.goal(), "make the loop stop");
    assert!(
        spec.criteria()
            .iter()
            .all(|criterion| !criterion.satisfied())
    );
    assert_eq!(
        spec.satisfy("c1").unwrap_err(),
        Error::EvidenceNotRecorded {
            id: "c1".to_owned()
        }
    );

    spec.record("c1", Evidence::supplied("it fired")).unwrap();
    spec.satisfy("c1").unwrap();

    let criterion = spec.criterion("c1").unwrap();
    assert!(criterion.satisfied());
    assert_eq!(criterion.text(), "the judge fires");
    assert_eq!(criterion.id(), "c1");
    assert_eq!(criterion.evidence().len(), 1);
}

#[test]
fn a_criterion_the_spec_does_not_hold_cannot_be_added_by_recording_against_it() {
    let mut spec = RunSpec::new("make the loop stop", &[("c1", "the judge fires")]);

    assert_eq!(
        spec.record("c2", Evidence::supplied("x")).unwrap_err(),
        Error::UnknownCriterion {
            id: "c2".to_owned()
        }
    );
    assert_eq!(
        spec.satisfy("c2").unwrap_err(),
        Error::UnknownCriterion {
            id: "c2".to_owned()
        }
    );
    assert_eq!(spec.criteria().len(), 1);
    assert_eq!(spec.criterion("c2"), None);
}

#[test]
fn the_ledger_vocabulary_is_its_wire_names() {
    assert_eq!(EntryStatus::Open.as_str(), "open");
    assert_eq!(EntryStatus::Closed.as_str(), "closed");
    assert_eq!(EvidenceOrigin::Collected.as_str(), "collected");
    assert_eq!(EvidenceOrigin::Supplied.as_str(), "supplied");
    assert_eq!(
        serde_json::to_string(&EntryStatus::Closed).unwrap(),
        "\"closed\""
    );
    assert_eq!(
        serde_json::from_str::<EvidenceOrigin>("\"collected\"").unwrap(),
        EvidenceOrigin::Collected
    );
    assert_eq!(
        serde_json::from_str::<EntryStatus>("\"open\"").unwrap(),
        EntryStatus::Open
    );
    assert_eq!(
        serde_json::to_string(&EvidenceOrigin::Supplied).unwrap(),
        "\"supplied\""
    );
}

#[test]
fn an_event_names_the_entry_it_merges_into() {
    let event = LedgerEvent::about("finding-1");

    assert_eq!(event.id(), "finding-1");
    assert_eq!(event, LedgerEvent::about("finding-1"));
    assert_eq!(LedgerEvent::default().id(), "");
}
