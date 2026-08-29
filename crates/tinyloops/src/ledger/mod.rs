//! The ledger: derived state, walked by code and rendered to Markdown.
//!
//! # No agent writes one
//!
//! A ledger is not a file an agent writes. [`refuse_derived`] refuses every
//! path inside [`DERIVED_FOLDER`], and `workspace::write` calls it on both the
//! proposed and the filed path, so rendering is the only way bytes enter it.
//! The refusal is **by folder**: the folder name is the invariant, not a
//! per-file rule that a new file escapes by being new.
//!
//! # One write operation
//!
//! A [`LedgerEvent`] names an entry and merges fields into it. There is no
//! delete. Closing keeps the entry with its reason, because a deleted entry is
//! indistinguishable from an entry that never existed, and the next pass
//! re-derives it.
//!
//! # Every section says what it left out
//!
//! [`render`] caps rows, truncates prose, and states both the omission count
//! and the call that fetches the rest — in every section, not once for the
//! file. A cut list that reads as complete is worse than a long one: the
//! reader, model or person, concludes nothing more exists and stops looking.
//!
//! # The bounds are asserted, not intended
//!
//! [`MAX_RENDERED_BYTES`] is a promise this module's tests hold it to, with a
//! deliberately absurd fixture — dozens of entries, multi-kilobyte prose — and
//! an assertion that past the bound more entries do not mean more file. The
//! failure that requires is measured: a ledger's table was bounded and the
//! prose sections beneath it were not, so the file grew to 86 KB and became a
//! third of one prompt before anybody counted. The table's bound was real;
//! nobody had asserted the file's.
//!
//! # A prompt carries the index
//!
//! [`index`] is one line per entry — its identity and its status — ending in
//! the call that fetches the rest. The ledger is what that call returns. A
//! prompt that inlines the ledger has turned a growing record into a growing
//! prompt.

mod types;

pub use types::{
    Criterion, EntryStatus, Evidence, EvidenceOrigin, LedgerEntry, LedgerEvent, RunSpec,
};

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::{Error, Result};

/// The folder no write path may enter.
///
/// Named once, here, so the workspace's refusal and the renderer's destination
/// cannot drift apart.
pub const DERIVED_FOLDER: &str = "derived";

/// How many rows any one rendered section shows.
pub const MAX_ROWS: usize = 6;

/// How many characters of prose any one row shows.
pub const MAX_PROSE: usize = 120;

/// How many lines [`index`] carries before it starts counting instead.
pub const MAX_INDEX_ROWS: usize = 32;

/// The bound [`render`] is held to, whatever it is given.
///
/// Asserted against a fixture an order of magnitude larger than any real run.
pub const MAX_RENDERED_BYTES: usize = 8192;

/// Refuses a path inside the derived folder.
///
/// # Errors
///
/// [`Error::DerivedWrite`] when any segment of the path names
/// [`DERIVED_FOLDER`]. Segment-wise on purpose: a nested path and a filename
/// nothing has ever seen are refused by the same rule.
pub fn refuse_derived(path: &str) -> Result<()> {
    if path
        .split(['/', '\\'])
        .any(|segment| segment == DERIVED_FOLDER)
    {
        return Err(Error::DerivedWrite {
            path: path.to_owned(),
        });
    }
    Ok(())
}

/// The rows a run leaves behind.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ledger {
    entries: BTreeMap<String, LedgerEntry>,
}

impl Ledger {
    /// An empty ledger.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Merges one event, creating the entry it names if it is new.
    ///
    /// The only write operation. It never removes anything.
    pub fn merge(&mut self, event: LedgerEvent) {
        let id = event.id().to_owned();
        self.entries
            .entry(id.clone())
            .or_insert_with(|| LedgerEntry::new(&id))
            .absorb(event);
    }

    /// One entry by identity.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownEntry`] when nothing recorded that identity.
    pub fn entry(&self, id: &str) -> Result<&LedgerEntry> {
        self.entries
            .get(id)
            .ok_or_else(|| Error::UnknownEntry { id: id.to_owned() })
    }

    /// Every entry, in identity order.
    #[must_use]
    pub fn entries(&self) -> Vec<&LedgerEntry> {
        self.entries.values().collect()
    }

    /// How many entries the ledger holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the ledger holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entries with a given status, in identity order.
    #[must_use]
    pub fn with_status(&self, status: EntryStatus) -> Vec<&LedgerEntry> {
        self.entries
            .values()
            .filter(|entry| entry.status() == status)
            .collect()
    }
}

/// Truncates prose to [`MAX_PROSE`], saying how much it left out.
fn clip(text: &str, fetch: &str) -> String {
    let mut characters = text.chars();
    let head = characters.by_ref().take(MAX_PROSE).collect::<String>();
    let omitted = characters.count();
    if omitted == 0 {
        head
    } else {
        format!("{head}… {omitted} characters omitted — {fetch}")
    }
}

/// The line a section ends on when it showed everything it holds.
fn omission(shown: usize, total: usize, fetch: &str) -> String {
    let omitted = total.saturating_sub(shown);
    format!("_showing {shown} of {total}; {omitted} omitted — {fetch}_\n")
}

/// Renders one table of entries.
fn table(out: &mut String, heading: &str, rows: &[&LedgerEntry], fetch: &str) {
    let _ = writeln!(out, "\n## {heading}\n");
    let _ = writeln!(out, "| id | status | title | evidence |");
    let _ = writeln!(out, "| --- | --- | --- | --- |");
    for entry in rows.iter().take(MAX_ROWS) {
        let title = entry.title().chars().take(40).collect::<String>();
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} |",
            entry.id(),
            entry.status().as_str(),
            title,
            entry.evidence().len()
        );
    }
    out.push_str(&omission(rows.len().min(MAX_ROWS), rows.len(), fetch));
}

/// Renders the ledger to Markdown, within [`MAX_RENDERED_BYTES`].
///
/// Every section caps its rows, truncates its prose, and states what it left
/// out plus the call that returns the rest.
#[must_use]
pub fn render(ledger: &Ledger) -> String {
    let mut out = String::from("# Run ledger\n");
    let open = ledger.with_status(EntryStatus::Open);
    let closed = ledger.with_status(EntryStatus::Closed);
    table(&mut out, "Open", &open, "fetch the rest with `ledger.entries()`");
    table(
        &mut out,
        "Closed",
        &closed,
        "fetch the rest with `ledger.entries()`",
    );

    let noted = ledger
        .entries()
        .into_iter()
        .filter(|entry| !entry.note().is_empty())
        .collect::<Vec<_>>();
    let _ = writeln!(out, "\n## Notes\n");
    for entry in noted.iter().take(MAX_ROWS) {
        let _ = writeln!(
            out,
            "- {}: {}",
            entry.id(),
            clip(entry.note(), "fetch the rest with `ledger.entry(id)`")
        );
    }
    out.push_str(&omission(
        noted.len().min(MAX_ROWS),
        noted.len(),
        "fetch the rest with `ledger.entry(id)`",
    ));

    let evidence = ledger
        .entries()
        .into_iter()
        .flat_map(|entry| {
            entry
                .evidence()
                .iter()
                .map(move |record| (entry.id().to_owned(), record))
        })
        .collect::<Vec<_>>();
    let _ = writeln!(out, "\n## Evidence\n");
    for (id, record) in evidence.iter().take(MAX_ROWS) {
        let _ = writeln!(
            out,
            "- {id} · {} · {}",
            record.origin().as_str(),
            clip(record.text(), "fetch the rest with `ledger.entry(id)`")
        );
    }
    out.push_str(&omission(
        evidence.len().min(MAX_ROWS),
        evidence.len(),
        "fetch the rest with `ledger.entry(id)`",
    ));
    out
}

/// Renders the index a prompt carries: one line per entry, then the fetch call.
///
/// The ledger itself never goes in a prompt. This is what does.
#[must_use]
pub fn index(ledger: &Ledger) -> String {
    let entries = ledger.entries();
    let mut out = String::new();
    for entry in entries.iter().take(MAX_INDEX_ROWS) {
        let _ = writeln!(out, "{} · {}", entry.id(), entry.status().as_str());
    }
    out.push_str(&omission(
        entries.len().min(MAX_INDEX_ROWS),
        entries.len(),
        "fetch an entry with `ledger.entry(id)`",
    ));
    out
}

#[cfg(test)]
mod test;
