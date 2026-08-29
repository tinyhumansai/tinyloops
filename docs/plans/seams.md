# Plan: the four seams

- **Status:** Not started
- **Specification:** [`../specs/seams.md`](../specs/seams.md), with the fourth
  seam and the ledger format in
  [`../specs/workspace-and-ledgers.md`](../specs/workspace-and-ledgers.md).

## Goal

Build `harness/`, `memory/`, `tools/`, `workspace/`, and `ledger/`: for each,
the trait, the one in-process reference implementation, and the tests. The end
state is that every bundled example and every test in this workspace runs with
no credentials, no network, and no dependence on wall-clock time or execution
order. Not here: the loop's own control flow
([`loop-kernel.md`](loop-kernel.md)) and `budget/` / `observe/`
([`observability-and-budget.md`](observability-and-budget.md)) — though the
`Role` record built in H1 carries the caps `budget/` validates, and the drop
and checkpoint-failure events named below are emitted through `observe/`'s
`Sink`.

## Ordering

The five module groups are **mutually independent** and may be built in
parallel by five implementers: nothing in `memory/` reads `tools/`, and the
loop does not wire them together until `presets/`. Within each group the tasks
are strictly ordered — trait, then reference implementation, then the failure
tests that only a double can produce.

`workspace/` (W) and `ledger/` (L) are the one exception: L3's refusal test
needs W2's write path, so L runs after W2 or accepts a stub write path it
replaces. Build W first if one implementer takes both.

Every task ends with `cargo test -p tinyloops <module>` and a clippy run over
`--all-targets --all-features -- -D warnings`.

## Shared interface

Every seam is reached from a `Step` (see [`loop-kernel.md`](loop-kernel.md)
task A2) and every *effect* crosses `tinyflows::caps::Capabilities`
(`vendor/tinyflows/src/caps/mod.rs:210`). A seam implementation opens no client
of its own. That is what makes "forgot the wrapper" unrepresentable: there is
no second way to reach a model, so there is no call that can miss the
recording. A test in H5 asserts it for the reference implementations.

## Task H1: `Role` and `RoleRegistry`

**Files:** `crates/tinyloops/src/harness/mod.rs`, `src/harness/types.rs`,
`src/harness/test.rs`, `src/error/mod.rs`

1. Failing tests: `a_role_is_a_prompt_a_grant_a_budget_and_a_tier` — the struct
   has those four fields and no fifth; `resolves_a_role_by_name`;
   `an_unknown_role_name_is_an_error`; and
   `a_role_without_caps_is_a_construction_error`, asserting
   `Error::RoleWithoutCaps { role }`. The last is not tidiness: a role that
   reads a report and answers in four lines, given an investigation's budget,
   investigates, because it has the calls.
2. Implement `Role { prompt, grant, caps, tier }` and `RoleRegistry` over a
   `BTreeMap<String, Role>`, so a call site names a role rather than assembling
   a model configuration inline.
3. `caps` is the `RoleCaps` type
   ([`observability-and-budget.md`](observability-and-budget.md) task B1).
   Until that task lands, define it in `budget/types.rs` as part of this task
   and let B1 extend it; do not define a second copy here.

## Task H2: `Delegate`, asynchronous by construction

**Files:** `crates/tinyloops/src/harness/delegate.rs`, `src/harness/test.rs`

1. Failing tests: `spawn_returns_a_ticket_without_waiting`;
   `peek_reports_status_without_settling`;
   `steer_reaches_a_running_delegation`; and
   `no_method_both_starts_and_settles_work` — an assertion over the trait's own
   method list, so the absence is checked rather than remembered.
2. Implement:

   ```rust
   pub trait Delegate: Send + Sync {
       fn spawn(&self, role: &str, brief: Brief) -> Result<Ticket>;
       fn peek(&self, ticket: &Ticket) -> Result<Status>;
       fn steer(&self, ticket: &Ticket, note: &str) -> Result<()>;
       async fn settle(&self, ticket: &Ticket) -> Result<Outcome>;
   }
   ```

   There is no `run_and_wait`, and adding one is a contract change rather than
   a convenience. The reason is measured: a production run of this design sat
   33 minutes unable to start its next attempt because it was awaiting a single
   arm — the wait was invisible, nothing else could proceed, and no observer
   could say which node was holding. A seam that offers a blocking call will
   have one used, so it offers none. Say that in the module docs.

## Task H3: the bounded `Mailbox`

**Files:** `crates/tinyloops/src/harness/mailbox.rs`, `src/harness/test.rs`

1. Failing tests: `posting_at_capacity_drops_the_note_and_says_so` — `post`
   returns a `Posted::Dropped`, never an `Err` the caller must handle by
   waiting; `a_drop_emits_an_event`;
   `the_loop_takes_its_next_step_in_the_same_test` after a drop, so the drop is
   proved non-blocking rather than assumed; and
   `capacity_is_declared_at_construction`.
2. Implement `Mailbox::new(capacity)`, `post`, and `collect`. Back-pressure
   that blocks the solve is the wrong trade: the note is an aside, the solve is
   the work. An unbounded queue turns a slow consumer into unbounded memory and
   a blocking send turns it into a stalled loop; dropping is the only one of
   the three that leaves the loop running, and recording it is what keeps it
   from being invisible.

## Task H4: `Outcome` and the salvage path

**Files:** `crates/tinyloops/src/harness/outcome.rs`, `src/harness/test.rs`

1. Failing tests: `a_timed_out_delegation_is_a_readable_outcome` — the outcome
   names the brief, how it ended, and what it left behind;
   `a_killed_delegation_that_wrote_an_artifact_is_salvaged`, with the outcome
   citing the artifact; and `a_failed_delegation_is_not_an_error_return`.
2. Implement `Outcome { brief, ending, artifacts, reply }` and
   `salvage(brief, artifacts) -> Outcome`. The ordinary way a long delegation
   ends is its own cap killing it, which destroys its reply and leaves every
   file it wrote; without salvage the pass reports nothing and the ladder spends
   a diversify on a run that was not stuck.

## Task H5: the offline reference harness

**Files:** `crates/tinyloops/src/harness/reference.rs`, `src/harness/test.rs`,
`src/lib.rs`

1. Failing tests: `scripted_outcomes_settle_in_the_order_the_script_declares`;
   `the_same_script_produces_the_same_events_on_every_run`, run twice in one
   test so determinism is asserted rather than hoped;
   `it_opens_no_transport_of_its_own` — the reference delegate holds a
   `Capabilities` and has no field that is a client.
2. Implement `ScriptedDelegate`, backed by a declared list of outcomes per
   role. Export `Role`, `RoleRegistry`, `Delegate`, `Ticket`, `Mailbox`,
   `Outcome`, and `ScriptedDelegate` from `src/lib.rs`.

## Task M1: the `Memory` trait, and absence as `None`

**Files:** `crates/tinyloops/src/memory/mod.rs`, `src/memory/types.rs`,
`src/memory/test.rs`

1. Failing tests: `recall_and_remember_take_an_explicit_scope`;
   `a_deployment_without_memory_wires_none`; and
   `the_loop_under_none_is_covered_separately_from_the_store_error_path` — two
   tests, deliberately distinct, because a stub that accepts calls in order to
   fail them turns a wiring fact into a run-time error and the loop can no
   longer tell "no memory here" from "memory is broken".
2. Implement `Memory` with `recall(scope, query)` and `remember(scope,
   record)`, and wire it as `Option<Arc<dyn Memory>>`, matching the engine,
   where `Capabilities::memory` is `Option<Arc<dyn MemoryProvider>>`
   (`vendor/tinyflows/src/caps/mod.rs:245`). No erroring stub ships.

## Task M2: the write-path verification probe

**Files:** `crates/tinyloops/src/memory/probe.rs`, `src/memory/test.rs`,
`src/error/mod.rs`

1. Failing tests:
   - `a_store_that_accepts_every_write_and_retains_nothing_fails_the_probe` —
     the store double answers success and keeps nothing, and `remember` returns
     `Error::WriteNotDurable { scope }`. This is the regression test for the
     production run that logged 193 successful `remember` calls and stored zero
     documents while the backend answered `200 {"status":"running"}` and
     dropped the work; every one of those calls was reported as a success by
     the only signal available.
   - `the_probe_verdict_is_cached_per_scope` — a second write to the same scope
     costs no second read-back, asserted on the double's call log.
   - `the_cached_verdict_expires` — asserted with an injected clock, not a
     sleep, so the test stays deterministic.
2. Implement the bounded read-back after a write, plus the per-scope cache.
   "The store accepted it" and "the store has it" are different observations,
   and only the second is a write.

## Task M3: compaction, recorded and idempotent

**Files:** `crates/tinyloops/src/memory/condense.rs`, `src/memory/test.rs`

1. Failing tests:
   - `condensing_twice_returns_the_same_view_and_appends_one_event` — both
     halves asserted: the view, and the count of appended condensation events.
     The single hardest-to-diagnose harness regression on public record was a
     context cleanup meant to run once that ran on every turn, producing both
     forgetfulness and continuous prompt-cache misses, and it took over a week
     to locate.
   - `what_a_pass_stopped_showing_is_still_readable_from_the_history` —
     compaction is recorded, not destructive.
   - `a_pinned_policy_constraint_survives_a_compaction_below_its_own_size` —
     evidence for the pin: policy-violation rate goes from 0% to 30% after
     compaction, measured 0% when the governing constraint survived and 38%
     when it was dropped, and pinning restored 0% for roughly 47 tokens.
2. Implement `condense(history, pins) -> Condensed`, returning the view a pass
   should use and appending a `Condensation { forgotten_ids, summary, offset }`
   event to the history.

## Task M4: the offline reference memory

**Files:** `crates/tinyloops/src/memory/reference.rs`, `src/memory/test.rs`,
`src/lib.rs`

1. Failing tests: `the_reference_probe_genuinely_reads_back` — it is not a
   constant `true`, proved by a variant constructed to drop writes;
   `the_same_calls_produce_the_same_history_on_every_run`.
2. Implement `MapMemory` over an in-process map. Export `Memory`, `Scope`,
   `Condensed`, and `MapMemory`.

## Task T1: `ToolSet`, gated in the constructor

**Files:** `crates/tinyloops/src/tools/mod.rs`, `src/tools/types.rs`,
`src/tools/test.rs`

1. Failing tests:
   - `constructing_without_a_group_leaves_it_absent_from_schemas` — and no test
     in this module achieves absence by prompt text, stated in the module docs
     so a later contributor does not add one.
   - `no_handler_decides_whether_it_is_allowed_to_run` — asserted over the
     handler signatures, which take no grant.
   - `the_model_facing_and_introspection_schema_sets_differ` — `schemas()`
     projects injected arguments out of what the model sees;
     `declared_schemas()` is the introspection view, documented upstream as
     "never put this on the wire"
     (`vendor/tinyagents/src/harness/tool/mod.rs:445-468`). `ToolSet` preserves
     the split rather than flattening it into one list.
   - `declared_schemas_output_never_reaches_a_model_request` — asserted by
     scanning a captured request for an injected-argument name.
2. Implement `ToolSet::new(grant: ToolGrant) -> Self`, returning a struct of
   optional groups. A withheld tool is withheld by not registering it, never by
   asking the model to abstain: a prompt instruction is not a control.

## Task T2: the resilient decorator, applied at construction

**Files:** `crates/tinyloops/src/tools/resilient.rs`, `src/tools/test.rs`

1. Failing tests:
   - `a_tool_error_becomes_a_model_readable_result` — the shape the harness
     already names is `ToolErrorPolicy::ReturnToError`, which returns the error
     to the model instead of failing the turn
     (`vendor/tinyagents/src/harness/tool/error_policy.rs:33`).
   - `the_same_instance_behaves_identically_through_both_paths` — one test
     reaching one tool through `Capabilities::tools`
     (`vendor/tinyflows/src/caps/mod.rs:214`) and through the harness,
     asserting the decorator's behavior on both. A `tool_call` node reaches the
     capability directly, with no middleware stack to run it through, so a
     decorator applied at *registration* is simply absent on that path and the
     two callers disagree about what the tool does.
2. Implement `Resilient::wrap(tool) -> Arc<dyn Tool>`, applied when the
   instance is built, before it is shared with either caller.

## Task T3: typed `Recovery`

**Files:** `crates/tinyloops/src/tools/recovery.rs`, `src/tools/test.rs`

1. Failing tests: `requery_feeds_the_error_back_against_a_bounded_retry_count`;
   `a_dead_sandbox_fixture_salvages_a_reconstructed_diff`, and the run reports
   a result rather than a failure;
   `fatal_is_the_only_variant_that_ends_a_step`; and
   `every_failure_appears_in_the_history_as_a_message`, so the next model call
   can see what failed and the recorded history explains the retry — errors
   never travel as out-of-band state.
2. Implement `Recovery { Requery, Salvage, Fatal }` and the sort from a tool
   error to a variant.

## Task T4: the offline reference tool set

**Files:** `crates/tinyloops/src/tools/reference.rs`, `src/tools/test.rs`,
`src/lib.rs`

1. Failing tests: `the_reference_set_separates_read_search_edit_and_execute` —
   every surveyed agent converges on those four verbs whether it exposes 1 tool
   or 37, so a `ToolSet` is reviewed on whether they are cleanly separable, not
   on how many entries it holds; `the_same_arguments_produce_the_same_result`.
2. Implement `PureTools`, a set of pure functions. Export `ToolSet`,
   `ToolGrant`, `Recovery`, `Resilient`, and `PureTools`.

## Task W1: `Layout`, the allowlist that reports the move

**Files:** `crates/tinyloops/src/workspace/mod.rs`, `src/workspace/types.rs`,
`src/workspace/test.rs`, `src/error/mod.rs`

1. Failing tests: `writing_an_unlisted_kind_is_refused_and_lands_no_bytes` —
   the workspace is byte-for-byte unchanged afterwards, asserted rather than
   assumed;
   `a_relocated_write_returns_both_the_proposed_and_the_actual_location`; and
   `the_relocation_record_appears_in_the_following_brief`. The last is the
   specific failure: a model not told where its file went writes the next one
   to the same original path and then cannot read either back — two files
   exist, the model believes in one, and it looks in the wrong place for it.
2. Implement `Layout`, `WriteKind`, and `write(kind, proposed, bytes) ->
   Result<Landed>`. A write names its kind; the workspace decides the path, so
   "where could this run have written?" is answered by reading the layout
   rather than by scanning a disk.

## Task W2: the path check, at both moments

**Files:** `crates/tinyloops/src/workspace/path.rs`, `src/workspace/test.rs`

1. Failing tests, one per case: `rejects_a_traversal_segment`;
   `rejects_an_absolute_path`; and
   `rejects_a_symlinked_parent_swapped_between_validation_and_write`. The third
   is the reason for the design: a path that validated and a path that is
   written are different moments, and that gap is exactly what the first check
   appears to close and does not.
2. Implement validation before any file system call, then re-verification of
   the canonical parent immediately before the write.

## Task W3: output bounded as it arrives, and `state()`

**Files:** `crates/tinyloops/src/workspace/capture.rs`,
`src/workspace/state.rs`, `src/workspace/test.rs`

1. Failing tests:
   - `a_command_far_over_the_bound_completes_and_the_process_survives`, with
     the captured output stating how many bytes were dropped between the
     retained head and tail. A cap applied to a completed buffer is not a cap:
     the process reaches the hard memory limit and is killed, and everything
     the run had accumulated dies with it, including the output that would have
     explained the failure.
   - `peak_capture_memory_is_a_function_of_the_bound_not_the_command`.
   - `a_snapshot_difference_names_exactly_the_files_the_action_touched`, over
     two consecutive actions.
   - `the_snapshot_is_bounded_in_size` — it is paid for on every turn, so it is
     a summary of the workspace, not its contents.
2. Implement streaming capture and `state()`, recomputed after every action.

## Task W4: `Checkpoint`, subordinate to the work

**Files:** `crates/tinyloops/src/workspace/checkpoint.rs`,
`src/workspace/test.rs`

1. Failing tests: `a_checkpoint_follows_every_successful_write`; and
   `a_backend_that_errors_on_every_call_leaves_every_write_successful`, with
   the run finishing and checkpoint-failure events recorded. Recording is
   subordinate to doing: a run whose checkpoint backend is unavailable loses
   history rather than progress.
2. Implement `Checkpoint` committing into a **side** repository, so a
   checkpoint is never confused with, and never interferes with, the change
   under construction.

## Task W5: the offline reference workspace

**Files:** `crates/tinyloops/src/workspace/reference.rs`,
`src/workspace/test.rs`, `src/lib.rs`

1. Failing tests: `the_temp_layout_round_trips_a_write_and_a_read_back`;
   `two_runs_of_the_same_actions_produce_the_same_snapshots`.
2. Implement `TempLayout`. Export `Layout`, `WriteKind`, `Landed`,
   `Checkpoint`, and `TempLayout`.

## Task L1: the ledger as derived state

**Files:** `crates/tinyloops/src/ledger/mod.rs`, `src/ledger/types.rs`,
`src/ledger/test.rs`

1. Failing tests: `an_event_names_an_entry_and_merges_fields_into_it`;
   `closing_an_entry_leaves_it_present_with_its_reason`; and
   `no_operation_removes_an_entry` — asserted over the public surface, because
   a deleted entry is indistinguishable from one that never existed and the
   next pass re-derives it.
2. Implement `Ledger`, `Entry`, and one write operation, `merge`. There is no
   delete.

## Task L2: bounded rendering, asserted against an absurd fixture

**Files:** `crates/tinyloops/src/ledger/render.rs`, `src/ledger/test.rs`,
`crates/tinyloops/tests/ledger_bounds.rs`

1. Failing tests:
   - `every_section_states_its_omission_count_and_its_fetch_call` — asserted
     per section, not for the file as a whole. A cut list that reads as
     complete is worse than a long one: the reader concludes nothing more
     exists and stops looking.
   - `an_absurd_fixture_renders_within_the_documented_bound` — an order of
     magnitude more entries than any real run, each with prose far longer than
     any real note. This is the regression test for the ledger whose table was
     bounded, whose prose sections beneath it were not, and which grew to 86 KB
     and became a third of one prompt before anybody counted. The table's bound
     was real; nobody had asserted the file's.
   - `a_prompt_carries_the_index_never_the_ledger` — the index is one line per
     entry, its identity and status, ending in the call that fetches the rest.
2. Implement `render(&Ledger) -> String` and `index(&Ledger) -> String`, each
   section capping rows, truncating prose, and naming what it left out.

## Task L3: the folder refusal, evidence origin, and the immutable spec

**Files:** `crates/tinyloops/src/ledger/guard.rs`, `src/ledger/test.rs`,
`src/error/mod.rs`

1. Failing tests:
   - `a_direct_write_anywhere_inside_a_ledger_folder_is_refused`, including to
     a filename the implementation has never seen. The refusal is by folder —
     the folder name is the invariant, not a per-file rule a new file escapes
     by being new.
   - `an_evidence_record_from_supplied_text_reports_supplied`, and
     `no_public_api_promotes_supplied_to_collected`. A transcript you were
     given is a claim; one you collected is evidence, and conflating them is
     how a run concludes a test passed on the strength of somebody's assertion
     that it did.
   - `writing_the_goals_or_criteria_during_a_run_is_refused`, and
     `a_criterion_moves_to_true_only_through_recorded_evidence`. An agent that
     can edit its own completion criteria does not have completion criteria; it
     has a preference.
2. Implement the guard in `workspace::write`'s path (task W1), the
   `evidence_origin` field settable only by the executing tool, and the run
   spec written once at configuration with every criterion `false`.
3. Export `Ledger`, `Entry`, `EvidenceOrigin`, and `RunSpec` from `src/lib.rs`.

## Task X1: every example runs offline

**Files:** `crates/tinyloops/tests/offline.rs`

1. Failing test
   `every_bundled_example_runs_with_no_credentials_and_no_network`: assemble
   each seam's reference implementation, run the loop from
   [`loop-kernel.md`](loop-kernel.md) task C7 over them, and assert completion.
   The test clears provider environment variables before running so a machine
   that happens to hold credentials does not hide a dependence on them.
2. Add module `README.md` files for `harness/`, `memory/`, `tools/`,
   `workspace/`, and `ledger/`, each covering the seam's design, public
   surface, and operational constraints.

## Verification

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] `cargo build --all-targets --all-features`
- [ ] `cargo test --all-features`
- [ ] `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features`
- [ ] `cargo deny check all`

## Invariants discharged

| Tasks | Invariants |
|---|---|
| H2 | `seams.md`: no seam trait exposes a blocking delegation |
| H3 | `seams.md`: a full mailbox drops and reports, never blocks |
| H4 | `seams.md`, `orchestrator.md` rule 4: a failed delegation is a result |
| H1, H5 | `seams.md`: deterministic references; every effect crosses `Capabilities` |
| M1 | `seams.md`: a missing capability is `None` at wiring time |
| M2 | `seams.md`: a write is not durable until a read-back verified it |
| M3 | `seams.md`: compaction is idempotent and recorded; pins survive |
| T1 | `seams.md`: grants resolve in a constructor; the schema sets stay split |
| T2 | `seams.md`: the decorator wraps instances before they are shared |
| T3 | `seams.md`: failures are messages; only `Fatal` terminates |
| W1 | `workspace-and-ledgers.md`: the allowlist, and the reported relocation |
| W2 | `workspace-and-ledgers.md`: the path check at both moments |
| W3 | `workspace-and-ledgers.md`: bounded capture; bounded `state()` |
| W4 | `workspace-and-ledgers.md`: a checkpoint failure never fails the write |
| L1 | `workspace-and-ledgers.md`: entries merge and close, never delete |
| L2 | `workspace-and-ledgers.md`: asserted render bounds; index-in-prompt |
| L3 | `workspace-and-ledgers.md`: the folder refusal, and `evidence_origin` |
| L3 | `workspace-and-ledgers.md`: goals and criteria immutable for the run |
| X1 | `seams.md`: every bundled example runs in CI with no credentials |
