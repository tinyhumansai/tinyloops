//! What a workspace is configured with, what it answers a write with, and the
//! two bounded summaries it keeps.
//!
//! The rules that use these — the path check at both moments, the checkpoint
//! that never fails the work it records — are in the module root, because they
//! are rules about a workspace rather than properties of any one of its parts.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

/// How many file names a [`Snapshot`] names before it starts counting instead.
///
/// The snapshot is paid for on every turn, so it is a summary of the workspace
/// rather than its contents.
pub const SNAPSHOT_NAMES: usize = 8;

/// What a write is for.
///
/// A write names its kind and the workspace decides the path. Nothing lands
/// outside the allowlist, so "where could this run have written?" is answered
/// by reading the [`Layout`] rather than by scanning a disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteKind {
    /// Work under construction: the change the run is actually making.
    Source,
    /// Something the run wrote down for itself.
    Note,
    /// Something the run wrote down for a person.
    Report,
    /// Working space the run may lose without losing progress.
    Scratch,
}

impl WriteKind {
    /// Every kind, in a fixed order.
    pub const ALL: [Self; 4] = [Self::Source, Self::Note, Self::Report, Self::Scratch];

    /// The kind's wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Note => "note",
            Self::Report => "report",
            Self::Scratch => "scratch",
        }
    }
}

impl fmt::Display for WriteKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The allowlist a write is filed against.
///
/// A kind absent from the layout is refused, and no bytes land. A kind present
/// decides the directory, which is how a write can be *relocated*: the caller
/// proposes a path, the layout files it by kind, and the difference is
/// reported rather than performed quietly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Layout {
    locations: BTreeMap<WriteKind, String>,
}

impl Layout {
    /// An empty layout, which refuses every write.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The layout the reference workspace ships with.
    #[must_use]
    pub fn standard() -> Self {
        Self::new()
            .allow(WriteKind::Source, "src")
            .allow(WriteKind::Note, "notes")
            .allow(WriteKind::Report, "reports")
            .allow(WriteKind::Scratch, "scratch")
    }

    /// Lists `kind`, filing its writes under `directory`.
    #[must_use]
    pub fn allow(mut self, kind: WriteKind, directory: &str) -> Self {
        self.locations.insert(kind, directory.to_owned());
        self
    }

    /// The directory `kind` is filed under, if it is listed at all.
    #[must_use]
    pub fn location(&self, kind: WriteKind) -> Option<&str> {
        self.locations.get(&kind).map(String::as_str)
    }

    /// The kinds this layout lists, in order.
    #[must_use]
    pub fn kinds(&self) -> Vec<WriteKind> {
        self.locations.keys().copied().collect()
    }
}

/// Where a write actually landed.
///
/// Returned by every successful write, including the ones that landed where
/// they were proposed. A relocated write says so, and [`Self::brief_note`] is
/// the sentence that reaches the next brief — the failure being prevented is
/// specific: a model not told where its file went writes the next one to the
/// same original path and then cannot read either back. Two files exist, the
/// model believes in one, and it is looking in the wrong place for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Landed {
    /// The kind the write named.
    pub kind: WriteKind,
    /// The path the caller proposed.
    pub proposed: String,
    /// The path the layout filed it under.
    pub actual: String,
    /// How many bytes landed.
    pub bytes: usize,
}

impl Landed {
    /// Whether the layout filed this write somewhere else.
    #[must_use]
    pub fn relocated(&self) -> bool {
        self.proposed != self.actual
    }

    /// The sentence the next brief carries.
    #[must_use]
    pub fn brief_note(&self) -> String {
        if self.relocated() {
            format!(
                "wrote {} as {} (relocated from {})",
                self.kind, self.actual, self.proposed
            )
        } else {
            format!("wrote {} as {}", self.kind, self.actual)
        }
    }
}

/// Command output, bounded while it arrives.
///
/// A cap applied to a completed buffer is not a cap: the process reaches its
/// hard memory limit and is killed, and everything the run had accumulated dies
/// with it — including the output that would have explained the failure. This
/// keeps a head and a tail and counts what fell between them, so peak memory is
/// a function of the bound rather than of the command.
#[derive(Debug, Clone)]
pub struct BoundedCapture {
    limit: usize,
    head: Vec<u8>,
    tail: VecDeque<u8>,
    dropped: usize,
}

impl BoundedCapture {
    /// A capture that will retain at most `limit` bytes.
    ///
    /// # Panics
    ///
    /// Never. A zero limit yields a capture that retains nothing and counts
    /// everything as dropped, which is a degenerate bound rather than an error.
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            head: Vec::new(),
            tail: VecDeque::new(),
            dropped: 0,
        }
    }

    /// Takes one chunk as it arrives.
    ///
    /// Byte at a time, deliberately: a chunked implementation that buffers the
    /// chunk first is bounded by the chunk size rather than by the limit, and a
    /// command that prints one very long line is exactly the case that matters.
    pub fn push(&mut self, chunk: &[u8]) {
        for byte in chunk {
            self.push_byte(*byte);
        }
    }

    /// Takes one byte.
    fn push_byte(&mut self, byte: u8) {
        let head_limit = self.limit / 2;
        if self.head.len() < head_limit {
            self.head.push(byte);
            return;
        }
        let tail_limit = self.limit - head_limit;
        if tail_limit == 0 {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        self.tail.push_back(byte);
        if self.tail.len() > tail_limit {
            self.tail.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    /// How many bytes are being held right now.
    ///
    /// Never above the limit the capture was built with. That is the property
    /// worth asserting: it is what keeps the process alive to report.
    #[must_use]
    pub fn retained(&self) -> usize {
        self.head.len() + self.tail.len()
    }

    /// How many bytes fell between the head and the tail.
    #[must_use]
    pub const fn dropped(&self) -> usize {
        self.dropped
    }

    /// The captured output, saying what it left out.
    #[must_use]
    pub fn render(&self) -> String {
        let head = String::from_utf8_lossy(&self.head).into_owned();
        let tail = self.tail.iter().copied().collect::<Vec<u8>>();
        let tail = String::from_utf8_lossy(&tail).into_owned();
        if self.dropped == 0 {
            format!("{head}{tail}")
        } else {
            format!("{head}\n... {} bytes dropped ...\n{tail}", self.dropped)
        }
    }
}

/// Where the run is, and what changed since the last mark.
///
/// Recomputed after every action and carried into the next brief, so a brief
/// does not spend model calls rediscovering facts the workspace already knows
/// exactly. Bounded by construction: it names at most [`SNAPSHOT_NAMES`] files
/// per list and counts the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// How many files the workspace holds.
    pub files: usize,
    /// How many bytes they hold between them.
    pub bytes: usize,
    /// Some of the file names, up to [`SNAPSHOT_NAMES`].
    pub named: Vec<String>,
    /// How many names were left out of the list above.
    pub named_omitted: usize,
    /// The files touched since the last mark, up to [`SNAPSHOT_NAMES`].
    pub changed: Vec<String>,
    /// How many changed names were left out.
    pub changed_omitted: usize,
}

impl Snapshot {
    /// The snapshot as the line a brief carries.
    #[must_use]
    pub fn render(&self) -> String {
        let mut line = format!("{} files, {} bytes", self.files, self.bytes);
        if !self.changed.is_empty() {
            line.push_str("; changed: ");
            line.push_str(&self.changed.join(", "));
        }
        if self.changed_omitted > 0 {
            let _ = write!(
                line,
                " (+{} more — fetch the rest with `workspace.state()`)",
                self.changed_omitted
            );
        }
        line
    }
}

/// Something a workspace could not do while recording what it did.
///
/// Emitted rather than returned: recording is subordinate to doing, and a run
/// whose checkpoint backend is unavailable loses history rather than progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceEvent {
    /// A checkpoint of a successful write failed.
    CheckpointFailed {
        /// The path whose write was being recorded.
        path: String,
        /// What the backend said.
        reason: String,
    },
}
