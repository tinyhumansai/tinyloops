# Workspace and ledgers

- **Status:** Accepted — 2026-08-29
- **Owner:** Maintainers
- **Related:** [`seams.md`](seams.md), [`loop-kernel.md`](loop-kernel.md),
  [`observability.md`](observability.md)

## Problem

A run that lasts more than a few turns leaves things behind: files it wrote,
commands it ran, findings it recorded, criteria it must eventually satisfy. Two
distinct problems live in that sentence and get conflated.

The first is the file system. A loop writing wherever it likes loses track of
what it wrote, re-writes the same path, and cannot read its own output back. It
runs a command whose output is larger than memory and the process dies with
everything unsaved. It cannot say what changed since the last turn, so each
brief re-derives the situation from scratch.

The second is the record. A run's findings, attempts, and completion criteria
are read by the next brief, so they are inputs to the model — which means they
are subject to every failure a prompt is subject to. An unbounded record grows
until it is a third of the prompt. A truncated record that does not say it was
truncated is read as complete. A record the agent can rewrite is not a record of
what happened; it is a record of what the agent last wanted to be true.

This specification defines the workspace seam and the ledger format, and
separates them: the workspace is where a run puts bytes, the ledger is derived
state that nothing in the run is permitted to write directly.

## Goals

- Define a `Layout` allowlist that files a write by kind and reports where it
  landed.
- Define the path check, the output bound, the `state()` snapshot, and
  `Checkpoint`.
- Define ledgers as derived state, walked by code and rendered to Markdown, with
  asserted — not intended — bounds.
- Fix which parts of the record are immutable to the agent, and why.

## Non-goals

- A storage backend. The workspace is a trait; a deployment supplies a local
  directory, a sandbox mount, or an object store behind it.
- Cross-run knowledge. A ledger describes one run. Anything spanning runs is
  `vendor/tinyflows/crates/adaptive`.
- The event journal, which is [`observability.md`](observability.md) and which
  is deliberately outside the layout.

## Proposed behavior

### Layout

A `Layout` is an allowlist of write kinds, each mapped to a location. A write
names its kind; the workspace decides the path. Two consequences follow.

A write of an unlisted kind is rejected. Nothing lands outside the allowlist, so
"where could this run have written?" is answered by reading the layout rather
than by scanning a disk.

**The workspace reports the move rather than performing it silently.** When a
write is filed somewhere other than where the caller proposed, the returned
record says so, and that record reaches the next brief. The failure this
prevents is specific and common: a model not told where its file went writes the
next one to the same original path, and then cannot read either back. Two files
exist, the model believes in one, and it is looking in the wrong place for it.

### The path check

Every path is validated before use. Traversal segments and absolute paths are
rejected. Then the canonical parent directory is re-verified immediately before
the write, because a path that validated and a path that is written are
different moments, and a symlink swapped between them is exactly the gap the
first check appears to close.

### Bounded command output

Command output is bounded **as it arrives**, not after. A cap applied to a
completed buffer is not a cap: the process reaches the hard memory limit and is
killed, and everything the run had accumulated dies with it — including the
output that would have explained the failure. Bounding the stream keeps the head
and the tail, records how much was dropped between them, and leaves the run
alive to report.

### `state()`

`state()` returns a snapshot: where the run is, and what changed since the last
mark. It is computed after every action and carried into the next brief. Without
it, each brief re-derives the situation from the transcript, spending model
calls to rediscover facts the workspace already knows exactly.

The snapshot is small by construction — it is a summary of the workspace, not
its contents — because it is paid for on every turn.

### `Checkpoint`

A `Checkpoint` commits into a **side** repository after every successful write.
The side repository keeps the run's history without touching whatever version
control the work itself lives in, so a checkpoint is never confused with, and
never interferes with, the change under construction.

A failed checkpoint never fails the work it was recording. Recording is
subordinate to doing: a run whose checkpoint backend is unavailable continues,
emits the failure as an event, and loses history rather than progress.

### Ledgers

A ledger is **derived state**: a structure walked by code and rendered to
Markdown. It is not a file an agent writes.

**No agent writes one; the write path refuses a derived file.** The refusal is
by folder — the folder name is the invariant, not a per-file rule that a new
file inside the folder escapes by being new. Rendering is the only way bytes
enter it.

**Every section caps rows, truncates prose, and says what it left out plus where
the rest is.** All three, in every section. A cut list that reads as complete is
worse than a long one: the reader — model or person — concludes nothing more
exists and stops looking. A section that dropped 40 of 47 rows must say it
dropped 40 and name the call that returns them.

**Those bounds are asserted, not intended.** A deliberately absurd fixture — far
more entries than any real run, each with prose far longer than any real note —
must render output within the stated bound, and that is a test. The failure this
prevents is measured: a ledger's table was bounded, the prose sections beneath
it were not, and the file grew to 86 KB and became a third of one prompt before
anybody counted. The table's bound was real; nobody had asserted the file's.

**A prompt carries the index, never the ledger.** The index is one line per
entry — its identity and its status — ending in the call that fetches the rest.
The ledger is what that call returns. A prompt that inlines the ledger has
turned a growing record into a growing prompt.

**One write operation.** An event names an entry and merges fields into it.
There is no delete: closing an entry keeps it, with its reason recorded. A
deleted entry is indistinguishable from an entry that never existed, and the
next pass re-derives it.

**Evidence carries its collector, or it is a claim.** Every evidence record has
an `evidence_origin` field with two values: `collected` and `supplied`. Only the
executing tool may set `collected`, and it sets it for output it produced
itself. A transcript you were given is a claim; a transcript you collected is
evidence. Both are worth recording, and conflating them is how a run concludes a
test passed on the strength of somebody's assertion that it did.

**The spec is immutable to the agent.** A run's goals and completion criteria
are structured data, written once when the run is configured. Every criterion
starts `false`. The write path refuses them exactly as it refuses any other
derived file. An agent that can edit its own completion criteria does not have
completion criteria; it has a preference. Criteria are satisfied by evidence
recorded against them, never by assignment.

## Invariants and constraints

- A write of a kind absent from the layout is rejected, and no bytes land.
- Every write returns where it landed. A relocated write says it was relocated,
  and the relocation reaches the next brief.
- Traversal segments and absolute paths are rejected before any file system
  call. The canonical parent is re-verified immediately before the write.
- Command output is bounded during capture. Peak memory for a command's output
  is a function of the bound, not of the command.
- `state()` is recomputed after every action and is bounded in size.
- A checkpoint follows every successful write. A checkpoint failure is emitted
  as an event and never propagates into the write's result.
- Bytes enter a ledger folder only through rendering. The write path refuses the
  folder, not a list of filenames.
- Every rendered section states its own omissions and where the omitted content
  can be fetched.
- Rendered output is bounded regardless of input size, asserted against a
  fixture larger than any plausible run.
- A prompt carries the index, never the rendered ledger.
- Entries are created and merged; they are never removed. Closing records a
  reason and retains the entry.
- `evidence_origin` is settable only by the executing tool. Nothing else in the
  run can promote `supplied` to `collected`.
- Goals and completion criteria are immutable for the run's duration. Every
  criterion is `false` at the start and changes only through recorded evidence.

## Acceptance criteria

- Writing an unlisted kind returns an error and leaves the workspace byte-for-
  byte unchanged.
- A write the layout relocates returns a record naming both the proposed and the
  actual location, and a test asserts that record appears in the following
  brief.
- Path inputs containing `..`, a leading separator, or a symlinked parent
  swapped between validation and write are each rejected, one test per case.
- A command emitting output far larger than the bound completes, the process
  survives, and the captured output states how many bytes were dropped.
- Two consecutive actions produce two `state()` snapshots whose difference names
  exactly the files the second action touched.
- A checkpoint backend that errors on every call leaves every write successful,
  and the run finishes with checkpoint-failure events recorded.
- An attempted direct write anywhere inside a ledger folder is refused,
  including to a filename the implementation has never seen.
- A fixture with an order of magnitude more entries and prose than any real run
  renders output within the documented bound. This is the regression test for
  the 86 KB ledger.
- Every truncating section's output contains its omission count and its fetch
  call; a test asserts this for each section rather than for the file as a
  whole.
- Closing an entry leaves it present with its reason; no operation removes one.
- An evidence record created from supplied text reports `supplied`, and no
  public API permits changing it to `collected`.
- Every attempt to write the goals or criteria structures during a run is
  refused, and a criterion moves to `true` only through recorded evidence.

## Open questions

- Whether the relocation report should also carry the reason a write was
  relocated. The reason helps a person reading the log and costs prompt tokens
  on every relocated write.
- Whether the side repository is required, or whether a deployment may supply an
  append-only log instead. A log satisfies the recording requirement but loses
  the ability to diff two checkpoints, which is how a person answers "what did
  turn 6 change?".
