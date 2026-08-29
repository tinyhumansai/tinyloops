//! Unit tests for the workspace seam.
//!
//! What is pinned here is the set of failures a workspace hides when it is
//! written the obvious way: a relocation performed quietly, a path checked once
//! rather than twice, a cap applied to a buffer that was already too big to
//! hold, and a checkpoint failure that took the write down with it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;

/// A checkpoint backend that refuses every commit.
#[derive(Debug, Default)]
struct BrokenCheckpoint;

impl Checkpoint for BrokenCheckpoint {
    fn commit(&self, _landed: &Landed) -> std::result::Result<(), String> {
        Err("the side repository is unreachable".to_owned())
    }
}

/// A resolver whose answer changes between the check and the write.
///
/// This is the symlink swap, made deterministic: the first resolution is inside
/// the workspace and the second has climbed out of it.
#[derive(Debug, Default)]
struct SwappingParents {
    calls: Mutex<u32>,
}

impl Parents for SwappingParents {
    fn canonical_parent(&self, _path: &str) -> String {
        let mut calls = lock(&self.calls);
        *calls += 1;
        if *calls == 1 {
            "notes".to_owned()
        } else {
            "../outside".to_owned()
        }
    }
}

/// A workspace over the standard layout and a fresh side repository.
fn workspace() -> (MemoryWorkspace, Arc<SideRepository>) {
    let side = Arc::new(SideRepository::new());
    let workspace =
        MemoryWorkspace::with_backends(Layout::standard(), side.clone(), Arc::new(PlainParents));
    (workspace, side)
}

#[test]
fn writing_an_unlisted_kind_is_refused_and_lands_no_bytes() {
    let layout = Layout::new().allow(WriteKind::Note, "notes");
    let workspace = MemoryWorkspace::new(layout);

    let refused = workspace.write(WriteKind::Source, "main.rs", b"fn main() {}");

    assert_eq!(
        refused.unwrap_err(),
        Error::UnlistedWriteKind {
            kind: WriteKind::Source
        }
    );
    assert_eq!(workspace.state().files, 0);
    assert_eq!(workspace.state().bytes, 0);
}

#[test]
fn a_relocated_write_returns_both_the_proposed_and_the_actual_location() {
    let (workspace, _side) = workspace();

    let landed = workspace.write(WriteKind::Note, "finding.md", b"it works").unwrap();

    assert!(landed.relocated());
    assert_eq!(landed.proposed, "finding.md");
    assert_eq!(landed.actual, "notes/finding.md");
    assert_eq!(landed.bytes, 8);
}

#[test]
fn the_relocation_record_appears_in_the_following_brief() {
    let (workspace, _side) = workspace();

    let landed = workspace.write(WriteKind::Note, "finding.md", b"it works").unwrap();
    let brief = format!("previously: {}", landed.brief_note());

    assert!(brief.contains("notes/finding.md"));
    assert!(
        brief.contains("relocated from finding.md"),
        "a model not told where its file went writes the next one to the same path: {brief}"
    );
}

#[test]
fn a_write_that_was_not_moved_says_only_where_it_landed() {
    let (workspace, _side) = workspace();

    let landed = workspace
        .write(WriteKind::Note, "notes/finding.md", b"it works")
        .unwrap();

    assert!(!landed.relocated());
    assert_eq!(landed.brief_note(), "wrote note as notes/finding.md");
}

#[test]
fn rejects_a_traversal_segment() {
    let (workspace, _side) = workspace();

    assert_eq!(
        workspace
            .write(WriteKind::Note, "../escape.md", b"x")
            .unwrap_err(),
        Error::PathTraversal {
            path: "../escape.md".to_owned()
        }
    );
    assert_eq!(workspace.state().files, 0);
}

#[test]
fn rejects_an_absolute_path() {
    let (workspace, _side) = workspace();

    assert_eq!(
        workspace
            .write(WriteKind::Note, "/etc/passwd", b"x")
            .unwrap_err(),
        Error::AbsolutePath {
            path: "/etc/passwd".to_owned()
        }
    );
    assert_eq!(
        validate("\\windows\\system32").unwrap_err(),
        Error::AbsolutePath {
            path: "\\windows\\system32".to_owned()
        }
    );
}

#[test]
fn rejects_a_symlinked_parent_swapped_between_validation_and_write() {
    let workspace = MemoryWorkspace::with_backends(
        Layout::standard(),
        Arc::new(SideRepository::new()),
        Arc::new(SwappingParents::default()),
    );

    let refused = workspace.write(WriteKind::Note, "finding.md", b"it works");

    assert_eq!(
        refused.unwrap_err(),
        Error::ParentEscaped {
            path: "notes/finding.md".to_owned()
        }
    );
    assert_eq!(
        workspace.state().files,
        0,
        "the bytes must not land when the parent moved under the check"
    );
}

#[test]
fn a_write_into_a_derived_folder_is_refused_by_the_folder_name() {
    let layout = Layout::standard().allow(WriteKind::Report, "derived");
    let workspace = MemoryWorkspace::new(layout);

    // Refused on the way in, by a filename nothing has ever seen …
    assert_eq!(
        workspace
            .write(WriteKind::Note, "derived/whatever-this-is.md", b"x")
            .unwrap_err(),
        Error::DerivedWrite {
            path: "derived/whatever-this-is.md".to_owned()
        }
    );
    // … and again once the layout has filed it there itself.
    assert_eq!(
        workspace
            .write(WriteKind::Report, "summary.md", b"x")
            .unwrap_err(),
        Error::DerivedWrite {
            path: "derived/summary.md".to_owned()
        }
    );
    assert_eq!(workspace.state().files, 0);
}

#[test]
fn writing_the_goals_or_criteria_during_a_run_is_refused() {
    let (workspace, _side) = workspace();

    assert_eq!(
        workspace
            .write(WriteKind::Note, "derived/spec.md", b"criterion: done")
            .unwrap_err(),
        Error::DerivedWrite {
            path: "derived/spec.md".to_owned()
        }
    );
}

#[test]
fn a_command_far_over_the_bound_completes_and_says_how_much_it_dropped() {
    let mut capture = BoundedCapture::new(64);

    for _ in 0..1_000 {
        capture.push(b"the command keeps printing and printing and printing\n");
    }

    let rendered = capture.render();
    assert!(capture.dropped() > 50_000);
    assert!(
        rendered.contains(&format!("{} bytes dropped", capture.dropped())),
        "the capture must say what fell between the head and the tail: {rendered}"
    );
}

#[test]
fn peak_capture_memory_is_a_function_of_the_bound_not_the_command() {
    let mut small = BoundedCapture::new(32);
    let mut large = BoundedCapture::new(32);

    small.push(b"abc");
    for _ in 0..10_000 {
        large.push(b"0123456789");
    }

    assert!(small.retained() <= 32);
    assert!(
        large.retained() <= 32,
        "a hundred kilobytes of output retained {} bytes",
        large.retained()
    );
    assert_eq!(small.dropped(), 0);
    assert_eq!(small.render(), "abc");
}

#[test]
fn a_capture_with_no_room_at_all_keeps_nothing_and_counts_everything() {
    let mut capture = BoundedCapture::new(0);

    capture.push(b"lost");

    assert_eq!(capture.retained(), 0);
    assert_eq!(capture.dropped(), 4);
    assert!(capture.render().contains("4 bytes dropped"));
}

#[test]
fn a_snapshot_difference_names_exactly_the_files_the_action_touched() {
    let (workspace, _side) = workspace();

    workspace.write(WriteKind::Note, "first.md", b"one").unwrap();
    let after_first = workspace.state();
    workspace.mark();
    workspace.write(WriteKind::Note, "second.md", b"two").unwrap();
    let after_second = workspace.state();

    assert_eq!(after_first.changed, vec!["notes/first.md".to_owned()]);
    assert_eq!(after_second.changed, vec!["notes/second.md".to_owned()]);
    assert_eq!(after_second.files, 2);
    assert_eq!(after_second.bytes, 6);
    assert!(after_second.render().contains("notes/second.md"));
}

#[test]
fn the_snapshot_is_bounded_in_size() {
    let (workspace, _side) = workspace();

    for index in 0..40 {
        workspace
            .write(WriteKind::Note, &format!("note-{index:02}.md"), b"x")
            .unwrap();
    }

    let snapshot = workspace.state();
    assert_eq!(snapshot.named.len(), SNAPSHOT_NAMES);
    assert_eq!(snapshot.named_omitted, 40 - SNAPSHOT_NAMES);
    assert_eq!(snapshot.changed.len(), SNAPSHOT_NAMES);
    assert_eq!(snapshot.changed_omitted, 40 - SNAPSHOT_NAMES);
    assert!(
        snapshot.render().len() < 512,
        "the snapshot is paid for on every turn: {}",
        snapshot.render()
    );
    assert!(snapshot.render().contains("32 more"));
}

#[test]
fn a_checkpoint_follows_every_successful_write() {
    let (workspace, side) = workspace();

    workspace.write(WriteKind::Note, "first.md", b"one").unwrap();
    workspace.write(WriteKind::Note, "second.md", b"two").unwrap();
    let _ = workspace.write(WriteKind::Source, "../escape.rs", b"no");

    assert_eq!(side.commits().len(), 2);
    assert!(side.commits()[0].contains("notes/first.md"));
    assert!(workspace.events().is_empty());
}

#[test]
fn a_backend_that_errors_on_every_call_leaves_every_write_successful() {
    let workspace = MemoryWorkspace::with_backends(
        Layout::standard(),
        Arc::new(BrokenCheckpoint),
        Arc::new(PlainParents),
    );

    for index in 0..3 {
        workspace
            .write(WriteKind::Note, &format!("note-{index}.md"), b"kept")
            .unwrap();
    }

    assert_eq!(workspace.state().files, 3);
    assert_eq!(workspace.events().len(), 3);
    assert_eq!(
        workspace.events()[0],
        WorkspaceEvent::CheckpointFailed {
            path: "notes/note-0.md".to_owned(),
            reason: "the side repository is unreachable".to_owned(),
        }
    );
}

#[test]
fn the_layout_round_trips_a_write_and_a_read_back() {
    let (workspace, _side) = workspace();

    let landed = workspace
        .write(WriteKind::Report, "summary.md", b"it worked")
        .unwrap();

    assert_eq!(workspace.read(&landed.actual).unwrap(), b"it worked");
    assert_eq!(
        workspace.read("notes/absent.md").unwrap_err(),
        Error::UnknownPath {
            path: "notes/absent.md".to_owned()
        }
    );
}

#[test]
fn two_runs_of_the_same_actions_produce_the_same_snapshots() {
    let actions = |workspace: &MemoryWorkspace| {
        workspace.write(WriteKind::Note, "one.md", b"a").unwrap();
        workspace.write(WriteKind::Source, "lib.rs", b"bb").unwrap();
        workspace.state()
    };

    let first = actions(&MemoryWorkspace::new(Layout::standard()));
    let second = actions(&MemoryWorkspace::new(Layout::standard()));

    assert_eq!(first, second);
}

#[test]
fn a_layout_answers_for_the_kinds_it_lists() {
    let layout = Layout::standard();

    assert_eq!(layout.location(WriteKind::Note), Some("notes"));
    assert_eq!(layout.kinds(), WriteKind::ALL.to_vec());
    assert_eq!(Layout::new().location(WriteKind::Note), None);
    assert_eq!(
        MemoryWorkspace::new(Layout::standard()).layout(),
        &Layout::standard()
    );
}

#[test]
fn the_write_kinds_are_their_wire_names() {
    assert_eq!(
        WriteKind::ALL.map(WriteKind::as_str),
        ["source", "note", "report", "scratch"]
    );
    assert_eq!(WriteKind::Scratch.to_string(), "scratch");
    assert_eq!(
        serde_json::to_string(&WriteKind::Report).unwrap(),
        "\"report\""
    );
    assert_eq!(
        serde_json::from_str::<WriteKind>("\"source\"").unwrap(),
        WriteKind::Source
    );
}

#[test]
fn the_plain_resolver_names_the_directory_a_path_sits_in() {
    assert_eq!(PlainParents.canonical_parent("notes/one.md"), "notes");
    assert_eq!(PlainParents.canonical_parent("one.md"), "");
}
