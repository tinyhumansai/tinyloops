# Plan: budget, observability, presets, and the worked example

- **Status:** Not started
- **Specification:** [`../specs/budget.md`](../specs/budget.md) and
  [`../specs/observability.md`](../specs/observability.md), with the seams the
  bounds attach to in [`../specs/seams.md`](../specs/seams.md) and the layout
  the journal must stay outside of in
  [`../specs/workspace-and-ledgers.md`](../specs/workspace-and-ledgers.md).

## Goal

Build `budget/`, then `observe/`, then `presets/`, and finish with a
`research_loop` example that exercises the whole framework offline. The end
state is a run that carries all six bounds, returns its cost as a field of its
result, emits one ordered event stream a person can read, and can be run from a
single `cargo run` with no credentials.

## Ordering

Strictly ordered across groups: **B** (`budget/`) → **O** (`observe/`) → **P**
(`presets/`) → **E** (the example). `observe/` names the tripped bound in a
`BudgetTripped` event and carries `Spend` in the `Report`, so it needs B's
types; `presets/` assembles a loop out of everything; the example runs a preset.

Parallel within a group: B1 and B2 are independent, and B3 needs both. O1
(vocabulary) and O2 (the recorder) can be written together against the enum
sketch in O1's interface block; O3, O4, and O5 each need O2. P1 and P2 are
independent.

Every task ends with `cargo test -p tinyloops <module>` and
`cargo clippy --all-targets --all-features -- -D warnings`.

## Prerequisites

- `state/`, `policy/`, and the four groups of
  [`loop-kernel.md`](loop-kernel.md).
- The five seams of [`seams.md`](seams.md). In particular `Role` (task H1)
  carries the `RoleCaps` this plan's task B1 defines; if H1 landed first it
  defined `RoleCaps` in `budget/types.rs` and B1 extends it rather than writing
  a second copy.

## A deviation this plan makes, deliberately

[`../specs/observability.md`](../specs/observability.md) requires the recorder to
implement both `tinyflows::observability::RunObserver`
(`vendor/tinyflows/src/observability.rs:189`) and the harness's `EventListener`
(`vendor/tinyagents/src/harness/observability/types.rs`). `tinyagents` is an
**optional** dependency and nothing under `crates/tinyloops/src/` is gated on it,
because a host loading the `cdylib` must resolve neither the harness nor its HTTP
client. So `observe/` defines this crate's own `CallSink` trait for the
model-and-tool-call plane and implements `RunObserver` directly; the
`EventListener` bridge lives in `examples/tinyagents_harness.rs`, behind the
`tinyagents` feature. One ordered stream is still the result, and O2's test
asserts it. Record this in `observe/README.md` so the divergence from the spec's
letter is visible rather than discovered.

## Task B1: `RoleCaps`, and the caps that cannot both trip

**Files:** `crates/tinyloops/src/budget/mod.rs`, `src/budget/types.rs`,
`src/budget/test.rs`, `src/error/mod.rs`

1. Failing tests:
   - `a_tool_call_cap_reachable_before_the_model_call_cap_is_rejected`, asserting
     an error that names **both** caps. The tool-call cap is set far above reach
     so the model-call cap trips first, because a graceful "stop with partial
     results" is honoured on the model-call path and not on the tool-call path.
   - `tool_timeout_at_or_above_run_timeout_is_rejected`, at `==` and at `>`, two
     cases. If the run clock can expire while a tool call is outstanding, the
     tool's graceful path is unreachable: an expired tool call returns what it
     captured, tagged with a timeout status, while an expired run loses its
     context and its report.
   - `a_role_with_no_caps_is_rejected` — a role is not given the loop's caps by
     default.
2. Implement `RoleCaps { model_calls, tool_calls, run_timeout, tool_timeout,
   turn_tokens }` and `RoleCaps::new(..) -> Result<Self>` performing all three
   checks. Add `Error::TwoReachableCaps { first, second }`,
   `Error::TimeoutOrdering { tool_timeout, run_timeout }`, and
   `Error::RoleWithoutCaps { role }`.
3. `tool_timeout` maps to the field the harness already carries,
   `ToolRuntime::timeout_ms` and `ToolTimeout`
   (`vendor/tinyagents/src/harness/tool/types.rs:321`); name that in the rustdoc
   so the mapping is one hop rather than a search.

## Task B2: the six concentric bounds

**Files:** `crates/tinyloops/src/budget/bounds.rs`, `src/budget/test.rs`

1. Failing tests:
   - `every_bound_is_present_and_none_defaults_to_unbounded` — asserted over the
     constructed value, one assertion per bound.
   - `a_loop_that_makes_no_progress_stops_on_max_iterations` and
     `a_loop_whose_passes_are_slow_stops_on_the_wall_clock` — two tests, neither
     relying on the other's bound, because an `until` condition alone never fires
     on a loop making no progress and `max_iterations` alone does not bound a
     loop whose passes are slow. Both drive an injected clock rather than
     sleeping, so they stay deterministic.
   - `a_step_over_its_threshold_yields_a_step_outcome_the_router_receives`, and
     the loop's remaining pass count is unchanged by it.
   - `a_retry_ladder_jitters_without_a_clock_or_an_rng` — jitter comes from an
     injected `Jitter` trait with a seeded reference implementation, so a fleet
     of runs does not retry in lockstep and the test still asserts exact delays.
2. Implement `Bounds` covering, outermost first: the loop (`max_iterations`,
   `until`, wall clock), per-step thresholds, per-role caps, the per-tool
   timeout, the per-request timeout, and the retry ladder.

## Task B3: a tripped bound is a routed outcome, and cost is a return value

**Files:** `crates/tinyloops/src/budget/trip.rs`, `src/budget/meters.rs`,
`src/budget/test.rs`, `src/lib.rs`

1. Failing tests:
   - `tripping_the_model_call_cap_mid_pass_returns_the_partial_work` — the
     result carries the work completed, the tripped bound's identity, and the
     accumulated cost. It is never a bare error: the loop stops, reports what it
     has, and says which bound stopped it.
   - `an_expired_tool_timeout_returns_the_captured_output_and_the_run_continues`.
   - `ten_passes_with_signal_and_ten_without_reach_the_same_raw_compute_and_different_effective_feedback`,
     and the report shows both. A loop that counts only turns cannot distinguish
     ten productive passes from ten that each learned nothing, and those are the
     two cases a budget most needs to tell apart.
   - `the_cost_sums_generation_verification_and_a_retried_call` — the expected
     total is the sum of all three legs, written out in the test.
   - `no_test_in_this_module_achieves_a_bound_by_instructing_a_model` — stated in
     the module docs and asserted by the absence of any prompt fixture here.
2. Implement `Trip { bound, work_completed, spend }`, `Meters { raw, effective }`,
   and `Spend`. Export `RoleCaps`, `Bounds`, `Trip`, `Meters`, and `Spend`.

## Task O1: the event vocabulary

**Files:** `crates/tinyloops/src/observe/mod.rs`, `src/observe/types.rs`,
`src/observe/test.rs`

1. Failing tests:
   - `every_named_transition_has_exactly_one_variant` — one test enumerating the
     eleven transitions the spec lists: pass started; step entered; step finished
     with its duration; arm started; arm finished; merge with its deltas;
     judgement with its score; route with its reason; delegation and its
     completion; operator directive received; budget bound tripped naming the
     bound; loop finished with its outcome.
   - `every_event_names_its_pass` — so the stream is reconstructable into passes
     without consulting anything else.
   - `a_route_event_carries_the_rung_that_fired`, serialized verbatim, so a
     reader of a finished run can see which rung it was.
2. Implement `LoopEvent` and `Sink`:

   ```rust
   pub trait Sink: Send + Sync { fn emit(&self, event: &Entry); }
   pub struct Entry { pub who: Who, pub pass: u32, pub payload: Payload }
   ```

## Task O2: the recorder, one ordered stream

**Files:** `crates/tinyloops/src/observe/recorder.rs`, `src/observe/test.rs`

1. Failing tests:
   - `a_node_activation_and_a_model_call_in_one_pass_appear_in_their_true_order`,
     each with a distinct `who` label. The recorder is registered as a
     `RunObserver` and as this crate's `CallSink`.
   - `a_child_view_and_its_parent_share_one_journal`, and
     `the_parents_counters_include_the_childs`. One journal, many views: a
     per-role view is a filter over the single stream, not a second stream that
     has to be reconciled with the first afterwards.
   - `a_sink_that_errors_drops_its_entry_and_records_the_drop`, and the loop
     takes its next step in the same test — no observability call blocks the
     loop.
2. Implement `Recorder`, `Recorder::child(label)`, and the `RunObserver` impl
   (`on_run_start`, `on_step_start`, `on_step_finish`, `on_item_start`,
   `on_item_finish`, `on_run_finish`, all at
   `vendor/tinyflows/src/observability.rs:189-228`).

## Task O3: entry and completion, and the spine in every view

**Files:** `crates/tinyloops/src/observe/pairing.rs`, `src/observe/test.rs`

1. Failing tests:
   - `entry_and_completion_events_pair_up_per_node_per_pass`, and a run whose
     steps never emit a completion fails it. This is the regression test for the
     62-minute silent gap in which a production run printed no driver line and
     which node was holding could only be inferred from which sub-agents happened
     to spawn during it. "The run stalled" must be a question the log answers.
   - `a_view_filtered_to_one_role_still_contains_pass_boundaries_verdicts_routes_and_budget_trips`.
     Nobody should have to be looking at the right tab to see that the run
     changed course.
2. Implement the pairing check as a public assertion helper the tests and the
   example both call, plus the spine's unconditional fan-out to every view.

## Task O4: payload-free by default, and a journal the loop cannot read

**Files:** `crates/tinyloops/src/observe/capture.rs`, `src/observe/test.rs`,
`crates/tinyloops/src/workspace/mod.rs`

1. Failing tests:
   - `with_capture_disabled_no_event_contains_prompt_or_tool_payload_text` —
     asserted by scanning the **serialized** stream for a fixture secret, not by
     inspecting fields. Disabled is the default: observability that defaults to
     recording prompts is a secret leak with a dashboard attached.
   - `with_capture_enabled_and_redaction_configured_the_fixture_secret_is_absent_from_every_sink`
     — the shape upstream already has is `RedactingSink`
     (`vendor/tinyagents/src/harness/observability/types.rs:397`).
   - `a_layout_that_includes_the_journal_path_is_rejected`, and
     `no_public_api_reads_the_journal`. One reflection step pulled its own 1.1 MB
     event log into a single 339,652-token call to re-read a verbatim replay of
     what it had already seen; a log the loop can read is a log the loop will
     eventually read.
2. Implement `Capture { Off, On }` with `Off` as `Default`, the redacting layer,
   and the layout rejection in `workspace::Layout::new`.

## Task O5: accounting, time attribution, and the `Report`

**Files:** `crates/tinyloops/src/observe/accounting.rs`, `src/observe/report.rs`,
`src/observe/test.rs`, `src/lib.rs`

1. Failing tests:
   - `a_response_reporting_a_different_model_yields_accounting_naming_the_model_that_answered`.
     Cost and token fields are read off each response body; with a fallback
     ladder the route genuinely varies per call, and a local price table prices
     the request that was intended rather than the one that happened. A companion
     test, `this_crate_holds_no_price_table`, asserts no cost constant exists.
   - `every_model_call_in_a_completed_run_carries_a_prompt_cache_hit_rate` —
     emitted per call, not derived later from token counts. The hardest-to-find
     harness regression on public record showed up first as continuous cache
     misses burning through rate limits while every other signal looked normal.
   - `a_pass_with_two_concurrent_arms_reports_a_concurrency_factor_above_one`,
     and `no_profile_reports_negative_or_unaccounted_time`. Summed arm time
     legitimately exceeds the wall clock, and a naive idle-time figure goes
     negative and is then ignored.
   - `the_printed_summary_and_the_status_payload_derive_from_one_report_value`.
     One structure for both, so the observability surface and the control surface
     cannot diverge.
   - `a_report_from_eight_repeats_states_per_attempt_outcomes_and_an_aggregate`,
     and `no_field_of_a_report_is_a_lone_success_boolean`. A 61% single-attempt
     pass rate becomes 25% when the same task must be completed over eight
     attempts, and capability rankings invert at long horizons — a single success
     bit reports the number that inverts.
2. Implement `CallRecord`, `PassProfile`, and `Report { attempts, routes, scores,
   spend, reliability, timings, undone }`. Export `LoopEvent`, `Sink`,
   `Recorder`, `Capture`, and `Report`.

## Task P1: the presets, each stating its bet

**Files:** `crates/tinyloops/src/presets/mod.rs`, `src/presets/types.rs`,
`src/presets/test.rs`

1. Failing tests:
   - `every_preset_builds_a_graph_that_validates_and_compiles`, over the whole
     shipped set.
   - `every_threshold_field_and_every_preset_has_rustdoc_stating_its_rationale`
     — a doc lint over the module asserts no field is undocumented. A `stuck` of
     2 is a defensible methodological commitment; an unrecorded 2 is a number
     nobody can argue with, revise, or defend in review.
   - `the_presets_are_the_set_the_parity_sweep_reads` — the sweep in
     [`loop-kernel.md`](loop-kernel.md) task C6 iterates this same list, asserted
     from one exported constant so a new preset cannot be added without being
     swept.
2. Implement the presets as associated constructors on `Thresholds`, each with a
   rustdoc paragraph naming its bet: a low `stuck` bets that variation is cheaper
   than persistence, a high one bets the opposite.

## Task P2: assembled loops, ready to run

**Files:** `crates/tinyloops/src/presets/assembled.rs`, `src/presets/test.rs`,
`src/lib.rs`

1. Failing tests:
   - `an_assembled_loop_runs_end_to_end_over_the_reference_seams` — under
     `tinyflows::testkit::TestHarness`, with `assert_completed` and
     `assert_no_null_bindings`
     (`vendor/tinyflows/src/testkit/harness.rs:279`), which is what catches a
     generated ladder that failed to compile and silently yielded null.
   - `assembling_twice_produces_the_same_graph_signature`.
   - `the_assembled_loop_carries_all_six_bounds`.
2. Implement `AssembledLoop`, wiring `LoopBuilder`, the `StepRegistry`, the
   `ArmSet`, the reference seams, `Bounds`, and a `Recorder`. Export it.

## Task E1: the `research_loop` example

**Files:** `crates/tinyloops/examples/research_loop.rs`, `README.md` is **not**
edited here — the example is discovered from `examples/` and built by
`cargo test`, and the command below belongs in the crate docs the loop modules
already own.

1. Write the example: assemble a preset over the reference harness, memory, tool
   set, workspace, and ledger; run it to a terminal state; print the `Report`.
   It runs offline against the reference implementations, so it needs no
   credentials, no network, and no wall clock.
2. It prints, in order: each pass boundary, the route each pass took and the rung
   that fired, the merge deltas, the terminal state, and the `Spend`. An accuracy
   figure with no cost beside it is not a comparable result.
3. Add a test in `crates/tinyloops/tests/offline.rs`
   ([`seams.md`](seams.md) task X1) asserting the example's `main` returns
   `Ok` with provider environment variables cleared.
4. `cargo run -p tinyloops --example research_loop`.

## Task E2: documentation

**Files:** `crates/tinyloops/src/budget/README.md`,
`crates/tinyloops/src/observe/README.md`, `src/lib.rs`

1. `budget/README.md`: the six bounds outermost to innermost, the
   exactly-one-reachable-cap rule and why it is a correctness property rather
   than a tuning preference, and the two meters.
2. `observe/README.md`: the three planes, the `who` labels, the deviation
   recorded above, and the rule that the journal's path can never be added to a
   layout.
3. Extend the crate-level docs in `src/lib.rs` with a `research_loop` pointer.
4. `cargo test --doc` and
   `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features`.

## Verification

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] `cargo build --all-targets --all-features`
- [ ] `cargo test --all-features`
- [ ] `cargo run -p tinyloops --example research_loop`
- [ ] `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features`
- [ ] `cargo deny check all`

## Invariants discharged

| Tasks | Invariants |
|---|---|
| B1 | `budget.md`: exactly one reachable cap; `tool_timeout < run_timeout`; no role without caps |
| B2 | `budget.md`: all six bounds present, none unbounded; the loop bound is a conjunction |
| B3 | `budget.md`: a tripped bound is a routed outcome; both meters advance; cost is a field of the result; no bound enforced by prompt text |
| O1 | `observability.md`: one event per transition, each naming its pass |
| O2 | `observability.md`: one ordered stream with a `who` label; `child` shares journal and counters; no observability call blocks the loop |
| O3 | `observability.md` rules 1 and 2: entry and completion pair up; the spine reaches every view |
| O4 | `observability.md` rules 3 and 4: payload-free by default; the journal is outside the layout and unreadable by the loop |
| O5 | `observability.md` rule 5 and the `Report`: per-call cache hit rate; no price table; a concurrency factor, never negative time; one `Report` for summary and status; repeat-reliability, never a success bit |
| P1 | `routing-and-policy.md`: every threshold and preset ships its rationale, and every preset is swept |
| P2, E1 | `seams.md`: every bundled example runs offline and deterministically |
