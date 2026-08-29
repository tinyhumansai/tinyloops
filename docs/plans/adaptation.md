# Plan: the loop profile

- **Status:** Not started
- **Specification:** [`../specs/adaptation.md`](../specs/adaptation.md),
  with the addressing decision in
  [ADR 0006](../adr/0006-thresholds-addressed-from-run-state.md) and the
  invariant it amends in
  [`../specs/loop-kernel.md`](../specs/loop-kernel.md).
- **Followed by:** [`adaptation-tuning.md`](adaptation-tuning.md), which adds
  the tuner and the amendments. It depends on this plan entirely.

## Goal

Move every threshold out of the emitted graph and into the run's accumulator, so
that one graph serves every preset, a threshold change stops changing
`GraphSignature`, and a run that later revises its own thresholds can resume
from its own checkpoint.

Nothing here tunes anything. This plan lands the addressing change and the
`LoopProfile` that holds it; the profile is written once, at construction, and
no code path moves it.

## Non-goals

- `Amendment`, `Bounds`, the `Tuner` trait, arm muting, and the two new events.
  All of that is [`adaptation-tuning.md`](adaptation-tuning.md).
- Any change to the graph's *shape*. Nodes, edges, and ports are untouched, and
  stay hashed by `GraphSignature`.
- Cross-run learning. See [ADR 0003](../adr/0003-three-layer-split-with-tinyflows-adaptive.md).

## Assumed, already landed

`Thresholds`, `Caps`, `Preset`, `LoopState`, `LoopBuilder`, `GraphSignature`,
the two parity harnesses, and the `research_loop` preset all exist and are
green. This plan edits them; it introduces one new file.

## Ordering

Groups are **strictly ordered**: P (`policy/`) → S (`state/`) → L (`loops/`) →
A (`presets/`, `step/`) → V (docs and exports). P defines the type and the
program that reads it; S puts it in the accumulator; L stops rendering numbers
into the graph and re-points the two signature tests and the parity harness;
A re-points the drivers; V publishes. **Parallel within a group:** P2 and P3
touch different functions in the same module and can be written together; every
other task depends on the one before it.

Every task ends with `cargo test -p tinyloops <module>` and
`cargo clippy --all-targets --all-features -- -D warnings`.

## A deviation this plan makes, deliberately

The specification's invariant 5 asked for the counter space "crossed with the
declared threshold space". Measured, that is on the order of 6×10^6 jq
evaluations, each a fresh compile through `jaq`, and it would dominate the test
suite. Task L3 sweeps the counter space exhaustively against a **declared box**
of threshold tuples — every preset, the corners of `{0,3}^5`, and the three
legacy tuples — for roughly 2.4×10^5 evaluations.

That is a genuine widening over today's harness, which tests four tuples and
nothing between them, and it is chosen to contain the boundaries where an
operator bug (`>` for `>=`) actually shows. It is not a proof over the whole
space, and the test's own doc comment must say so in those words. The
specification carries the same wording, so the two do not drift.

## Task P1: the profile type

**Files:** `crates/tinyloops/src/policy/profile.rs` (new),
`src/policy/mod.rs`, `src/policy/test.rs`

1. Failing tests: `a_default_profile_carries_the_balanced_thresholds`,
   `the_profile_wire_form_is_pinned` (a `serde_json::to_value` equality against
   a literal, per the house rule for anything crossing a checkpoint), and
   `a_profile_written_without_a_revision_deserializes`.
2. Implement in `profile.rs`:

   ```rust
   #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
   #[serde(default)]
   pub struct LoopProfile {
       /// Bumped by one every time an amendment is folded. Always 0 here.
       pub revision: u32,
       /// The counter bounds the routing ladder reads.
       pub thresholds: Thresholds,
       /// The preset this profile started from.
       pub origin: Preset,
   }
   ```

   `Preset` needs `Serialize`/`Deserialize`/`Default` (`Balanced`) to sit in
   here; it is already `Copy`, `Ord`, and `#[non_exhaustive]`, and it has
   `as_str`/`parse`, so derive serde with `rename_all = "lowercase"` and pin
   that its wire names match `Preset::as_str`.
3. `LoopProfile::of(preset)` and `Preset::profile()` as the two spellings of the
   same construction, with the second delegating to the first.
4. It lives under `policy/` because it *is* the policy the run operates under.
   The module direction is already precedented: `policy/test.rs` reads
   `crate::presets::Preset::ALL`.
5. `caps`, `muted`, and `history` arrive in the follow-on plan;
   `#[serde(default)]` at the container level is what absorbs the addition
   without breaking a checkpoint written by this revision.

## Task P2: the ladder reads an address

**Files:** `crates/tinyloops/src/policy/ladder.rs`, `src/policy/test.rs`

1. Failing tests:
   - `the_ladder_reads_thresholds_out_of_the_accumulator` — two states differing
     only in `profile.thresholds.stuck` route differently through the *same*
     program.
   - `a_state_with_no_profile_routes_retry` — evaluate the ladder against a
     scope whose accumulator has no `profile` key and assert `Route::Retry`.
   - `the_ladder_holds_no_threshold_literal` — no default threshold value
     appears in the program text.
   - The same three for `terminal_condition`.
2. Rewrite `ladder()` and `terminal_condition()` to take no arguments and emit
   one fixed program:

   ```text
   =(.state // .item) as $s | (($s | .profile.thresholds) // {}) as $t
   | if ((($s|.blocked)//0) >= (($t|.blocked)//4294967295)) then "blocked"
     elif ...
   ```

   Every threshold read carries `// 4294967295`. That is not a style choice: a
   missing key is `null`, `null` sorts below every number under `jaq`, and
   `0 >= null` is **true** — so an unguarded read would fire the first rung and
   route `Blocked` on a state that has no profile. `u32::MAX` makes every rung
   false and falls through to `Retry`, which is the cheap outcome the house rule
   asks defaults to point at.
3. `evaluate_ladder(&state, loop_id)` and
   `evaluate_terminal_condition(&state, loop_id)` lose their `&Thresholds`
   argument. `expr_scope` is unchanged — it serializes the whole `LoopState`, so
   the profile arrives at every address the engine offers.
4. Replace `the_ladder_interpolates_thresholds_rather_than_hard_coding_them`
   (`policy/test.rs`) with `the_ladder_addresses_thresholds_rather_than_rendering_them`,
   and do the same for `the_terminal_condition_interpolates_thresholds`. These
   are the two tests that today assert the *old* invariant, so they must be
   rewritten rather than deleted: the guard is still wanted, against a different
   failure.

## Task P3: the pure functions take one argument

**Files:** `crates/tinyloops/src/policy/mod.rs`, `src/policy/types.rs`,
`src/loops/termination.rs`, `src/policy/test.rs`

1. Failing tests: `route_reads_the_profile_it_was_handed` and
   `a_route_cannot_be_computed_against_someone_elses_thresholds` (a compile-time
   fact once the argument is gone; assert it by construction in the test's
   comment and by the signature).
2. `route(&LoopState)`, `is_terminal(&LoopState)`, and
   `Outcome::classify(&LoopState)` drop `&Thresholds` and read
   `state.profile.thresholds`.
3. `TerminationCondition::{holds, evaluate, expression}` and the private
   `program` / `join` drop the argument with them.
4. Purity is preserved — still a function of the state alone, which is what
   makes exhaustive parity possible at all — and a caller can no longer hand the
   router a threshold set the run is not using.
5. **This is a breaking public API change.** Pre-1.0, so a minor bump, and it is
   named explicitly in the pull request's "public API or behavior changes"
   section per `.github/PULL_REQUEST_TEMPLATE.md`.

## Task S1: the accumulator carries the profile

**Files:** `crates/tinyloops/src/state/types.rs`, `src/state/mod.rs`,
`src/state/test.rs`

1. Failing tests:
   - `the_wire_form_is_pinned` — extend the existing literal with `"profile"`.
   - `an_older_accumulator_still_deserializes` — a state serialized without
     `profile` takes the default.
   - `no_arm_can_move_the_profile` — build two arm states with different
     profiles, fold them, and assert the merged profile is the base's.
2. Add `pub profile: LoopProfile` to `LoopState`.
3. `LoopState::apply` carries it through from `self` untouched, in the same
   block that already carries `board` and `answer`. That is the whole of
   invariant 2 for this plan: no `Delta` field and no `Contribution` field
   reaches the profile, so an arm cannot move it however it is wired, and the
   test above is the proof rather than the promise.
4. Add `LoopState::with_profile(goal, profile)`. `LoopState::new(goal)` seeds
   `LoopProfile::default()` so every existing caller keeps the balanced
   thresholds it has today.

## Task L1: the head's cap stops being a threshold

**Files:** `crates/tinyloops/src/loops/builder.rs`, `src/loops/test.rs`,
`src/presets/assembled.rs`

1. Failing tests:
   - `the_head_is_capped_by_the_budget_not_by_the_thresholds` — two builders
     differing only in `max_attempts` emit the same `max_iterations`.
   - `every_preset_can_reach_its_attempt_ceiling` — for each `Preset::ALL`,
     assert `preset.thresholds().max_attempts <= caps.max_iterations`. This one
     fails today for a reason worth keeping: `Preset::Persistent` sets
     `max_attempts: 12` against a default `Caps::max_iterations` of 8, so a
     persistent run is truncated four attempts short of its own ceiling and
     nothing says so.
2. `LoopBuilder` gains the run's `Caps` (a `.caps(Caps)` builder method,
   defaulting to `Caps::default()`), and `head()` reads
   `caps.max_iterations` for `max_iterations`. `on_exceeded: "continue"` is
   unchanged, so reaching the cap emits on `done` rather than failing the run.
3. `head().config.until` becomes `self.termination.expression()` — no argument.
4. `AssembledLoop::graph()` passes `self.budget.caps()`.
5. Fix the truncation the second test exposes: either raise the shipped
   `Caps::max_iterations` or lower `Persistent::max_attempts`. Raising the cap
   is the smaller change and the one that keeps the preset's stated bet intact;
   whichever is chosen, the rustdoc on the changed constant says why it moved.

## Task L2: the signature stops moving

**Files:** `crates/tinyloops/src/loops/test.rs`

No change to `signature.rs`. `GraphSignature::of` hashes each node's `config`
whole, and after L1 and P2 there is no threshold in any `config`.

1. Invert the existing test that asserts two threshold sets give **different**
   signatures into `a_graph_is_the_same_graph_under_every_preset`: build a graph
   for each `Preset::ALL` and assert one signature across all four. This is the
   test that today encodes the behavior being removed, so it is rewritten, not
   deleted.
2. Add `the_emitted_graph_holds_no_threshold_literal` — serialize the graph and
   assert no default threshold value appears in it.
3. Add `a_checkpoint_taken_under_one_preset_resumes_under_another` — record a
   signature from one preset's graph and `verify_resume` it against another's.
   That is the property this whole plan exists to buy, so it gets its own named
   test rather than being implied by the equality above.
4. Keep the arm-set test that asserts a *smaller* graph has a different
   signature. Topology still moves the hash; only values left it.

## Task L3: the parity harness

**Files:** `crates/tinyloops/tests/routing_parity.rs`,
`crates/tinyloops/src/policy/test.rs`

1. Failing tests: `the_rendered_ladder_and_the_rust_router_agree_over_the_box`
   and `the_sweep_covers_every_preset_and_every_corner`.
2. The two harnesses currently sweep **different** threshold sets — the in-crate
   one derives from `Preset::ALL`, the integration one hard-codes
   `default`/`impatient`/`patient`. Unify them on one function returning every
   preset tuple, the corners of `{0,3}^5`, and the three legacy tuples, and
   assert from both sides that the set contains every preset.
3. The switch's program is now constant, so `routing_program(&graph)` is read
   once rather than per tuple, and the threshold under test is varied by
   building the `LoopState` rather than by rebuilding the graph. Counters sweep
   `0..=4` on all five routing fields plus `solved`; the tuple set is the outer
   loop; scoping and the per-value thread stay as they are.
4. Keep `a_ladder_that_fails_to_compile_is_caught_by_the_sweep`.
5. Replace `the_emitted_program_is_the_generated_ladder_and_not_a_second_copy`'s
   `contains(">= 7")` assertion with one asserting the program contains
   `.profile.thresholds` and the sentinel, and still equals `ladder()` verbatim.
6. Measure the suite. If this test alone runs past ~30s, shrink the corner box
   to `{0,2}^5` before touching anything else — the boundaries are still inside
   it, and a sweep nobody waits for is a sweep somebody deletes.

## Task A1: the assembled loop

**Files:** `crates/tinyloops/src/presets/assembled.rs`,
`src/presets/types.rs`, `src/presets/test.rs`

1. Failing tests: `a_driven_run_routes_on_the_profile_it_was_built_with` and
   `the_assembled_loop_exposes_its_profile`.
2. Drop the `thresholds` field from `AssembledLoop`. `drive` seeds
   `LoopState::with_profile(goal, preset.profile())` and calls `route(&state)`;
   today it calls `route(&state, &self.thresholds)` against a field set once in
   `new` and never re-read, which is exactly the second source this plan
   removes.
3. Replace the `thresholds()` accessor with `profile()`. Anything wanting the
   thresholds reads `profile().thresholds`.
4. `Outcome::classify` and `is_terminal` calls lose their argument.

## Task A2: the step seam

**Files:** `crates/tinyloops/src/step/mod.rs`, `src/step/types.rs`,
`src/step/test.rs`

1. Failing test: `a_step_is_handed_the_thresholds_its_state_carries` — register
   a step that asserts `ctx.thresholds()` equals the profile in the state it was
   given.
2. `run_loop_step` and `StepRegistry::run{,_with}` stop taking `&Thresholds` and
   read it off the decoded state, so a host cannot hand a step a threshold set
   the run is not using.
3. `StepContext` keeps its `thresholds: &'a Thresholds` field. The caller copies
   it out of the state before the state is moved — `Thresholds` is `Copy` — and
   borrows the local. No signature on `Step`, `Arm`, or `Observer` changes.

## Task V1: exports, docs, and callers

**Files:** `crates/tinyloops/src/lib.rs`, `src/policy/mod.rs`,
`src/policy/ladder.rs`, `src/loops/builder.rs`, `tests/public_api.rs`,
`tests/e2e.rs`, `tests/loop_run.rs`, `examples/simple_loop.rs`,
`examples/research_loop.rs`, `README.md`

1. Re-export `LoopProfile` from `lib.rs`, in the `policy` group beside
   `Thresholds`.
2. Update every doctest that calls `ladder(&thresholds)`, `route(&state, &t)`,
   `evaluate_ladder(..)`, or `terminal_condition(&t)` — they are compiled and
   run by `cargo test --doc`, so they cannot be left behind.
3. Add a `LoopProfile` example to `tests/public_api.rs` using only the public
   surface.
4. `README.md` mentions the preset, not the thresholds, so it needs a line only
   where it describes what a checkpoint is compatible with.

## Task V2: full verification

Run the checklist below and read the output. In particular, `cargo test`
without `--all-features` runs in CI as its own step, and the coverage gate is
per file — `policy/profile.rs` ships with its own tests or the build fails.

## Verification

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] `cargo build --all-targets --all-features`
- [ ] `cargo test --all-features` and `cargo test`
- [ ] `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features`
- [ ] `cargo run -p tinyloops --example basic`,
      `--example simple_loop`, `--example research_loop`
- [ ] `.github/scripts/check-file-coverage.sh 90 coverage.json`

## Invariants discharged

| Tasks | `adaptation.md` invariant |
|---|---|
| P1, P2, L1, L2 | 1 — the profile is state, not topology |
| S1 | 2 — the head is still the accumulator's only writer |
| P3, L3 | 5 — the route stays pure, parity is proved over a declared box |

Invariants 3, 4, and 6 through 9 have nothing to attach to until there is an
amendment to bound; they are discharged by
[`adaptation-tuning.md`](adaptation-tuning.md).
