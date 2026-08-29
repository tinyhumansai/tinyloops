# Plan: the tuner and the amendments

- **Status:** Implemented
- **Specification:** [`../specs/adaptation.md`](../specs/adaptation.md), with
  the arm laws it rests on in
  [`../specs/loop-kernel.md`](../specs/loop-kernel.md) and the boundary it must
  not cross in
  [ADR 0003](../adr/0003-three-layer-split-with-tinyflows-adaptive.md).
- **Depends on:** [`adaptation.md`](adaptation.md), entirely. Every task here
  reads `LoopState::profile`, which that plan introduces.

## Goal

Let a run revise its own configuration, within bounds it cannot widen, on
evidence it records. One role proposes; the head folds; every proposal and every
refusal is an event; the finished profile leaves the run as data nothing here
scores.

## Non-goals

- Scoring an amendment against outcomes, promoting a tuned profile to a preset,
  or selecting a profile for a new run. That is
  `vendor/tinyflows/crates/adaptive`. This plan produces the data such a layer
  would read and reads none of its state.
- Changing the graph's shape. Nodes, edges, and ports stay fixed and stay
  hashed.
- Adding an arm, a step, or a delegate the loop was not built with.

## Ordering

Groups are **strictly ordered**: T (types and the tuner seam) → B (bounds) →
F (the fold) → E (events) → D (output and docs). T defines what a proposal is
and who may make one, B defines what may be proposed, F is where a proposal
becomes a profile, E makes both visible, D publishes. **Parallel within a
group:** T1 and T2 are independent files; F1 depends on all of T and B.

Every task ends with `cargo test -p tinyloops <module>` and
`cargo clippy --all-targets --all-features -- -D warnings`.

## Task T1: what a proposal is

**Files:** `crates/tinyloops/src/policy/amendment.rs` (new),
`src/policy/mod.rs`, `src/policy/test.rs`

1. Failing tests: `the_amendment_wire_form_is_pinned`,
   `every_change_round_trips`, and
   `a_change_names_the_field_it_moves` (each `ThresholdField` maps to exactly
   one `Thresholds` field, asserted by applying it and diffing).
2. Implement:

   ```rust
   pub struct Amendment {
       /// The role that proposed it. One value is legal; see T4.
       pub proposer: String,
       /// The pass that proposed it. It takes effect on the next one.
       pub pass: u32,
       pub change: Change,
       /// The evidence, in the proposer's words. Rendered into the report.
       pub because: String,
   }

   pub enum Change {
       Threshold(ThresholdField, u32),
       Cap(CapField, u64),
       MuteArm(String),
       UnmuteArm(String),
   }
   ```

   `ThresholdField` names the seven `Thresholds` fields, `plan_interval`
   included — the re-plan cadence is a threshold like any other, and a second
   spelling of it would be a second thing to keep in step.
3. `Change::apply_to(&mut LoopProfile)`, total and infallible, called only after
   `Bounds::check` has passed.
4. **A closed enum, not a JSON patch.** A patch can address anything the
   accumulator holds, including the counters the ladder routes on, so a tuner
   able to emit one is a tuner able to write `solved`. Say so in the rustdoc;
   it is the reason the type is shaped this way and it will not be obvious to
   the next reader.

## Task T2: the tuner seam

**Files:** `crates/tinyloops/src/arm/types.rs`, `src/arm/mod.rs`,
`src/presets/steps.rs`, `src/arm/test.rs`, `src/step/types.rs`

1. Failing tests: `a_tuner_proposes_at_most_one_amendment_a_pass`,
   `a_tuner_that_proposes_nothing_folds_as_unchanged`, and a `compile_fail`
   doctest, pinned to its error code, showing that an `impl Arm` has no method
   that produces an `Amendment`.
2. Implement a trait distinct from `Arm`:

   ```rust
   pub trait Tuner: Send + Sync {
       fn name(&self) -> &'static str;
       fn propose(
           &self,
           base: &LoopState,
           report: &Value,
           ctx: StepContext<'_, NoWrite>,
       ) -> Result<Option<Amendment>>;
   }
   ```

   and a `TunerArm` adapter implementing `Arm` over it — the only code in the
   crate that writes `LoopState::proposed`.
3. A separate trait rather than a third capability marker beside `CanWrite` and
   `NoWrite`. The marker is the shape the `AccumulatorAccess` docs invite, and
   it does not fit here: `Arm::evaluate` takes a concrete
   `StepContext<'_, NoWrite>` and `ArmSet` holds `Arc<dyn Arm>`, so making the
   context generic costs object safety. The adapter buys the same guarantee —
   an `impl Arm` has no way to mint one — for no change to the arm surface.
4. Add `pub proposed: Option<Amendment>` to `LoopState`, and extend the wire-form
   pin in `state/test.rs`.
5. Prove it the way this repo already proves invariant 11: a `compile_fail`
   doctest with a pinned error code, matching the pair on `Observer`. There is
   no `trybuild` in the tree and this does not add one.

## Task T3: the narrative slot

**Files:** `crates/tinyloops/src/state/types.rs`, `src/state/mod.rs`,
`src/state/test.rs`

1. Failing tests: `an_amendment_travels_as_a_contribution` and
   `merge_refuses_two_arms_proposing_an_amendment` (asserting
   `Error::ContestedField { field: "amendment", .. }` names both arms).
2. Add `amendment: Option<Amendment>` to `Contribution`, wire it through
   `claimed_from` and `apply_to`, and add a `claim` slot for it in
   `LoopState::merge`.
3. `claimed_from` and `apply_to` are documented inverses whose breakage is
   silent, so both halves change in this task and the round-trip is asserted.
   A proposal that survives `apply_to` but is not recovered by `claimed_from`
   would be dropped at the merge with nothing to report it.
4. This is invariant 2: the head remains the accumulator's sole writer, and a
   second proposer in one superstep is refused rather than resolved by arrival
   order.

## Task T4: at most one tuner

**Files:** `crates/tinyloops/src/arm/types.rs`, `src/error/mod.rs`,
`src/error/test.rs`, `src/arm/test.rs`

1. Failing test: `two_tuners_in_one_arm_set_are_refused`, asserting the error
   names both arms.
2. Add `Arm::may_tune()` defaulting to `false`, returned `true` by `TunerArm`,
   and `Error::AmbiguousTuning { first, second }` with its message assertion.
3. Check it in `ArmSet::new` in the same loop that already rejects a second
   concluding arm — the `may_conclude` / `AmbiguousConclusion` pair is the exact
   shape, and putting the second check anywhere else is how the two drift.

## Task B1: the bounds

**Files:** `crates/tinyloops/src/policy/bounds.rs` (new),
`src/presets/types.rs`, `src/policy/test.rs`, `src/presets/test.rs`

1. Failing tests: `a_change_outside_its_range_is_refused`,
   `narrowing_only_ever_tightens`, `every_preset_states_its_bounds`, and
   `a_bounds_a_deployment_forgot_is_the_presets`.
2. Implement `Bounds` with an inclusive range per threshold field, a ceiling per
   cap field, the arms that may be muted, `muting_window`, and
   `max_amendments`; `Bounds::check(&Change) -> Result<()>`; and
   `Bounds::narrow(other)` clamping field by field.
3. `Preset::bounds()` beside `Preset::thresholds()`. The preset owns them: the
   room a run has to revise itself is part of the methodological bet the preset
   already states, so choosing a preset is choosing the bet *and* the room.
4. `narrow` is the same operation `RunBudget::narrow` already performs on caps —
   `.min()` field by field — so a deployment can tighten a preset it distrusts
   and can never loosen one.
5. Out of range is **refused**, never clamped. A clamped proposal reads as
   accepted at the proposer and as a no-op in the state, and nothing joins the
   two; the refusal is what makes a broken tuner visible.

## Task F1: the fold

**Files:** `crates/tinyloops/src/presets/steps.rs`, `src/presets/test.rs`

1. Failing tests:
   - `an_amendment_does_not_change_the_route_of_the_pass_that_proposed_it`
   - `an_amendment_changes_the_route_of_the_next_pass`
   - `a_refused_amendment_leaves_the_profile_byte_identical`
   - `a_run_at_its_amendment_budget_refuses_the_next_and_continues`
2. Implement in the `Advance` step, which runs at `pass`: check
   `state.proposed` against `Bounds` and `max_amendments`, apply it, bump
   `revision`, append to `history`, clear `proposed`.
3. `pass` is the right node and not merely a convenient one. It is the single
   exit every route enters and the only node closing the cycle, so "an
   amendment takes effect on the *next* pass" is a property of where the code
   sits rather than a rule someone has to remember. Folding anywhere inside the
   body would make the pass's own route depend on whether the tuner finished
   before the routing node — arm arrival order deciding the run.

## Task F2: muting

**Files:** `crates/tinyloops/src/presets/steps.rs`, `src/presets/test.rs`

1. Failing tests: `a_muted_arm_still_runs_its_node_and_still_converges`,
   `a_muted_arm_contributes_nothing`, and
   `unmuting_an_undeclared_arm_is_an_error`.
2. `ArmStep::run` returns `ArmOutcome::unchanged` when the arm is in
   `profile.muted`, without calling `evaluate`. Nothing about the edges changes.
3. Muting removes an arm's *work*, not its edges. Dropping a convergence edge
   would leave the merge barrier waiting on an arm nothing will activate — a
   hung pass rather than a saved one — and it would make the fan-out and the
   fold settable independently, which is the drift `loop-kernel.md` invariant 6
   makes unrepresentable. The test asserts the merge waits on the same arm count
   before and after a mute.
4. `UnmuteArm` names an arm the `ArmSet` already declares; there is no change
   that adds one.

## Task T5: the shipped tuner

**Files:** `crates/tinyloops/src/presets/tuner.rs` (new),
`src/presets/mod.rs`, `src/presets/test.rs`

1. Failing test: `the_rule_tuner_proposes_on_exactly_these_passes` — drive a
   fixed counter sequence and assert the exact set of passes on which it
   proposes, and the content of each proposal.
2. Implement `Rules`, a pure function of the counters and the arm ledger:
   - a `Diversify` followed by a pass strictly more unproductive than the retry
     before it proposes `Threshold(Stuck, stuck + 1)`, once;
   - an arm whose contribution has been an empty delta and an empty narrative
     for `muting_window` consecutive passes proposes `MuteArm(name)`;
   - consecutive `blocked` passes at the bound propose a cap reduction rather
     than more attempts.
3. Rule-based by default, and the rustdoc says why: a model asked mid-run
   whether its own configuration is wrong has no ground truth to answer from and
   every incentive to answer yes — the same pressure that makes a model claim
   `Solved` on the eighth pass. A rule tuner's whole behavior is a pure function
   over counters and is therefore testable at every boundary. A model tuner is
   permitted behind the same trait and is bounded by the same `Bounds`, which is
   the point of putting the bounds outside the proposer.
4. The `MuteArm` rule stays conservative on purpose. Bandit arm-elimination
   drops an arm on a *measured reward*, and this loop has no per-arm reward — so
   the rule fires on "contributed nothing measurable", never on "scored worse".
   Record that in the rustdoc, and in the open questions if the distinction
   turns out to matter in practice.

## Task E1: the events

**Files:** `crates/tinyloops/src/observe/types.rs`, `src/observe/mod.rs`,
`src/observe/test.rs`

1. Failing tests: extend `every_event()` and let
   `every_event_round_trips_through_its_wire_form` and
   `every_event_names_its_pass_and_renders_to_one_line` fail on the new
   variants.
2. Add `Event::Amended { pass, revision, change, because }` and
   `Event::AmendmentRefused { pass, change, reason }`.
3. Four matches have no `_` arm and must be updated together: `Event::pass()`,
   `Event::kind()`, `render`, and the `every_event()` fixture.
4. **The fixture is currently missing `NoteDropped`** — the variant exists, is
   rendered, and is emitted from the mailbox, but no wire-form test covers it,
   so a new variant can escape the test that exists to catch exactly this. Add
   the missing entry in this task, and add an assertion that the fixture's
   length matches the variant count so the next omission fails loudly.
5. Emit both events from the fold in F1. A run that quietly retuned itself and
   then succeeded is indistinguishable in its report from a run that succeeded
   as configured; the refusals matter as much as the acceptances, because forty
   refused proposals is a broken tuner reporting nothing.

## Task E2: the head's ceiling follows the bounds

**Files:** `crates/tinyloops/src/loops/builder.rs`, `src/loops/test.rs`

1. Failing test: `a_raised_attempt_ceiling_buys_passes` — a run whose
   `max_attempts` is amended upward actually gets the extra passes.
2. The head's `max_iterations` reads the bounds' `max_attempts` ceiling rather
   than the budget's cap where the bound is the larger of the two. Left alone,
   an amendment raising `max_attempts` folds, the profile says twelve, and the
   head still stops at the number it was built with — inert, and silent about
   it.

## Task D1: the run's output

**Files:** `crates/tinyloops/src/presets/assembled.rs`,
`src/orchestrate/steps.rs`, `src/presets/test.rs`

1. Failing tests: `a_driven_run_reports_its_final_profile` and
   `the_history_is_in_fold_order_not_proposal_order`.
2. Add `profile: LoopProfile` to `Driven`; the `report` step renders the
   amendment history — what was changed, when, and on what evidence.
3. Nothing in this crate scores it. A test asserts the tuner's context type
   exposes no ledger, catalogue, or score handle, so reading `adaptive` state
   does not compile. This is where a run's self-observation stops and cross-run
   learning starts, and a crate that scored its own amendments would be scoring
   them on a single sample.

## Task D2: exports, docs, and verification

**Files:** `crates/tinyloops/src/lib.rs`, `src/presets/README.md`,
`tests/public_api.rs`, `README.md`, `docs/specs/adaptation.md`

1. Re-export `Amendment`, `Change`, `Bounds`, `Tuner`, `TunerArm`, and the two
   field enums.
2. Write `src/presets/README.md` covering the shipped tuner's rules, the bounds
   each preset ships, and the operational constraint that a deployment may
   narrow but never widen them.
3. Add a public-surface test assembling a loop with a tuner using only
   `tinyloops::*`.
4. Mark `docs/specs/adaptation.md` **Implemented** and record any deliberately
   untested edge case in the pull request description.

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
| T2, T3, T4 | 2 — one proposer, and the head is still the only writer |
| T1, B1 | 3 — closed, bounded per field, budgeted per run |
| F1 | 4 — never takes effect in the pass that proposed it |
| F2 | 6 — a muted arm still runs and still converges |
| T1, T4, F2 | 7 — an amendment adds nothing the loop was not built with |
| E1 | 8 — every amendment and every refusal is an event |
| D1 | 9 — the final profile is an output, and this crate scores nothing |

Invariants 1 and 5 are discharged by [`adaptation.md`](adaptation.md).
