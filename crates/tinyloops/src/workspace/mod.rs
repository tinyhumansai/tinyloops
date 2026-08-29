//! The workspace seam: where a run puts bytes, and what it is told about it.
//!
//! # The layout reports the move
//!
//! A write names a [`WriteKind`]; the [`Layout`] decides the path. A kind the
//! layout does not list is refused and lands nothing. A write the layout files
//! somewhere other than where the caller proposed comes back as a [`Landed`]
//! that says both locations, and [`Landed::brief_note`] is how that reaches the
//! next brief. Performing the move quietly is the failure this exists to
//! prevent: a model not told where its file went writes the next one to the
//! same original path, and then cannot read either back.
//!
//! # The path is checked at both moments
//!
//! Traversal segments and absolute paths are rejected before any file system
//! call. Then the canonical parent is re-verified *immediately before* the
//! bytes land. Two checks, not one: a path that validated and a path that is
//! written are different moments, and a parent swapped between them is exactly
//! the gap the first check appears to close and does not.
//!
//! # Output is bounded as it arrives
//!
//! [`BoundedCapture`] keeps a head and a tail and counts what fell between
//! them. A cap applied to a completed buffer is not a cap: the process hits its
//! hard memory limit and is killed, taking with it the output that would have
//! explained the failure.
//!
//! # A checkpoint never fails the work it records
//!
//! Every successful write is committed to a **side** repository, so a
//! checkpoint is never confused with, and never interferes with, the change
//! under construction. A backend that errors leaves the write successful and
//! adds a [`WorkspaceEvent::CheckpointFailed`] to [`Workspace::events`].
//! Recording is subordinate to doing.
//!
//! # `derived/` is refused by construction
//!
//! A ledger is derived state and rendering is the only way bytes enter it. The
//! refusal is by folder — see [`crate::ledger::refuse_derived`] — so a filename
//! this code has never seen does not escape it by being new. The run's goals
//! and completion criteria live there too, which is what makes them immutable
//! for the run's duration.
//!
//! # In memory, on purpose
//!
//! [`MemoryWorkspace`] is the one reference implementation, and it holds its
//! files in a map. A real deployment supplies a directory, a sandbox mount, or
//! an object store behind the same trait; this one exists so every bundled
//! example and every test here runs with no credentials, no network, and no
//! dependence on the file system's own state.

mod types;

pub use types::{
    BoundedCapture, Landed, Layout, SNAPSHOT_NAMES, Snapshot, WorkspaceEvent, WriteKind,
};

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::{Error, Result};

/// Locks `mutex`, taking the value back from a poisoned lock.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Rejects a path before anything opens it.
///
/// Three refusals, each before any file system call: a traversal segment, an
/// absolute path, and anything inside a derived folder.
///
/// # Errors
///
/// - [`Error::AbsolutePath`] when the path is rooted.
/// - [`Error::PathTraversal`] when any segment is `..`.
/// - [`Error::DerivedWrite`] when any segment names the derived folder.
pub fn validate(path: &str) -> Result<()> {
    if path.starts_with('/') || path.starts_with('\\') {
        return Err(Error::AbsolutePath {
            path: path.to_owned(),
        });
    }
    if path.split(['/', '\\']).any(|segment| segment == "..") {
        return Err(Error::PathTraversal {
            path: path.to_owned(),
        });
    }
    crate::ledger::refuse_derived(path)
}

/// Resolves the canonical parent directory of a path.
///
/// A seam, because the swap this module defends against happens *in* this
/// resolution: on a real file system a symlinked parent can be replaced between
/// the check and the write, and the only way to test that deterministically is
/// to be able to supply a resolver that does it.
pub trait Parents: Send + Sync + std::fmt::Debug {
    /// The canonical parent directory of `path`.
    ///
    /// A result that is absolute or that climbs out with `..` is read as an
    /// escape and refuses the write.
    fn canonical_parent(&self, path: &str) -> String;
}

/// The resolver a workspace with no symlinks needs.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlainParents;

impl Parents for PlainParents {
    fn canonical_parent(&self, path: &str) -> String {
        match path.rsplit_once('/') {
            Some((parent, _)) => parent.to_owned(),
            None => String::new(),
        }
    }
}

/// Where a successful write is recorded.
///
/// A **side** repository: it keeps the run's history without touching whatever
/// version control the work itself lives in.
pub trait Checkpoint: Send + Sync + std::fmt::Debug {
    /// Records one successful write.
    ///
    /// # Errors
    ///
    /// Returns the backend's own message. The caller never propagates it into
    /// the write's result — see the module docs.
    fn commit(&self, landed: &Landed) -> std::result::Result<(), String>;
}

/// The reference side repository: an append-only list of commits.
#[derive(Debug, Default)]
pub struct SideRepository {
    commits: Mutex<Vec<String>>,
}

impl SideRepository {
    /// An empty side repository.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The commits recorded so far, oldest first.
    #[must_use]
    pub fn commits(&self) -> Vec<String> {
        lock(&self.commits).clone()
    }
}

impl Checkpoint for SideRepository {
    fn commit(&self, landed: &Landed) -> std::result::Result<(), String> {
        lock(&self.commits).push(landed.brief_note());
        Ok(())
    }
}

/// Where a run puts bytes.
pub trait Workspace: Send + Sync {
    /// Files `bytes` by kind, returning where they landed.
    ///
    /// # Errors
    ///
    /// Every refusal in [`validate`], plus [`Error::UnlistedWriteKind`] for a
    /// kind the layout does not hold and [`Error::ParentEscaped`] when the
    /// canonical parent moved between the two checks.
    fn write(&self, kind: WriteKind, proposed: &str, bytes: &[u8]) -> Result<Landed>;

    /// Reads back what a write landed.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownPath`] when nothing is there.
    fn read(&self, path: &str) -> Result<Vec<u8>>;

    /// Where the run is, and what changed since the last mark.
    fn state(&self) -> Snapshot;

    /// Forgets the current change set, so the next snapshot's difference names
    /// exactly what the next action touched.
    fn mark(&self);

    /// What the workspace could not do while recording what it did.
    fn events(&self) -> Vec<WorkspaceEvent>;
}

/// The offline reference workspace.
///
/// Holds its files in a map so two runs of the same actions produce the same
/// snapshots, with no dependence on a directory, a clock, or a test's ordering.
#[derive(Debug)]
pub struct MemoryWorkspace {
    layout: Layout,
    files: Mutex<BTreeMap<String, Vec<u8>>>,
    touched: Mutex<Vec<String>>,
    events: Mutex<Vec<WorkspaceEvent>>,
    checkpoint: Arc<dyn Checkpoint>,
    parents: Arc<dyn Parents>,
}

impl MemoryWorkspace {
    /// A workspace over `layout`, recording into a fresh side repository.
    #[must_use]
    pub fn new(layout: Layout) -> Self {
        Self::with_backends(
            layout,
            Arc::new(SideRepository::new()),
            Arc::new(PlainParents),
        )
    }

    /// A workspace over supplied backends.
    #[must_use]
    pub fn with_backends(
        layout: Layout,
        checkpoint: Arc<dyn Checkpoint>,
        parents: Arc<dyn Parents>,
    ) -> Self {
        Self {
            layout,
            files: Mutex::new(BTreeMap::new()),
            touched: Mutex::new(Vec::new()),
            events: Mutex::new(Vec::new()),
            checkpoint,
            parents,
        }
    }

    /// The layout this workspace files writes against.
    #[must_use]
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// Where the layout files a proposed path of this kind.
    ///
    /// # Errors
    ///
    /// [`Error::UnlistedWriteKind`] when the kind is not in the allowlist.
    fn locate(&self, kind: WriteKind, proposed: &str) -> Result<String> {
        let Some(directory) = self.layout.location(kind) else {
            return Err(Error::UnlistedWriteKind { kind });
        };
        let name = proposed.rsplit('/').next().unwrap_or(proposed);
        Ok(format!("{directory}/{name}"))
    }

    /// Refuses a canonical parent that has left the workspace.
    fn verify_parent(&self, path: &str) -> Result<()> {
        let parent = self.parents.canonical_parent(path);
        if parent.starts_with('/')
            || parent.starts_with('\\')
            || parent.split(['/', '\\']).any(|segment| segment == "..")
        {
            return Err(Error::ParentEscaped {
                path: path.to_owned(),
            });
        }
        Ok(())
    }
}

impl Workspace for MemoryWorkspace {
    fn write(&self, kind: WriteKind, proposed: &str, bytes: &[u8]) -> Result<Landed> {
        validate(proposed)?;
        let actual = self.locate(kind, proposed)?;
        validate(&actual)?;
        // The first moment: the parent as it stands when the path is checked.
        self.verify_parent(&actual)?;

        // The second moment: the parent as it stands when the bytes are about
        // to land. A resolver that answers differently here is the swap this
        // whole arrangement exists to catch.
        self.verify_parent(&actual)?;

        let landed = Landed {
            kind,
            proposed: proposed.to_owned(),
            actual: actual.clone(),
            bytes: bytes.len(),
        };
        lock(&self.files).insert(actual.clone(), bytes.to_vec());
        lock(&self.touched).push(actual);

        if let Err(reason) = self.checkpoint.commit(&landed) {
            lock(&self.events).push(WorkspaceEvent::CheckpointFailed {
                path: landed.actual.clone(),
                reason,
            });
        }
        Ok(landed)
    }

    fn read(&self, path: &str) -> Result<Vec<u8>> {
        lock(&self.files)
            .get(path)
            .cloned()
            .ok_or_else(|| Error::UnknownPath {
                path: path.to_owned(),
            })
    }

    fn state(&self) -> Snapshot {
        let files = lock(&self.files);
        let touched = lock(&self.touched);
        let named = files
            .keys()
            .take(SNAPSHOT_NAMES)
            .cloned()
            .collect::<Vec<_>>();
        let changed = touched
            .iter()
            .take(SNAPSHOT_NAMES)
            .cloned()
            .collect::<Vec<_>>();
        Snapshot {
            files: files.len(),
            bytes: files.values().map(Vec::len).sum(),
            named_omitted: files.len().saturating_sub(named.len()),
            named,
            changed_omitted: touched.len().saturating_sub(changed.len()),
            changed,
        }
    }

    fn mark(&self) {
        lock(&self.touched).clear();
    }

    fn events(&self) -> Vec<WorkspaceEvent> {
        lock(&self.events).clone()
    }
}

#[cfg(test)]
mod test;
