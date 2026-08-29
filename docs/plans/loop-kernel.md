# Plan: the loop kernel

- **Status:** Not started
- **Specification:** [`../specs/loop-kernel.md`](../specs/loop-kernel.md), with
  the ladder's constants from
  [`../specs/routing-and-policy.md`](../specs/routing-and-policy.md) and the
  role bindings from [`../specs/orchestrator.md`](../specs/orchestrator.md).

## Goal

Build, in order, the four modules that turn `state` and `policy` into a running
loop: `step/`, `arm/`, `loops/`, `orchestrate/`. The end state is a builder
that emits one `tinyflows::model::WorkflowGraph` holding the whole goal run,
and a closed set of Rust steps that the graph's nodes invoke through a single
tool. Not here: the seams ([`seams.md`](seams.md)); `budget/`, `observe/`,
`presets/`, and the worked example
([`observability-and-budget.md`](observability-and-budget.md)); and anything
spanning runs (`vendor/tinyflows/crates/adaptive`).

## Assumed, already landed

`crates/tinyloops/src/state/` and `crates/tinyloops/src/policy/`, from the
concurrent module work; this plan consumes them unmodified. The names below are
the ones [`../specs/routing-and-policy.md`](../specs/routing-and-policy.md)
fixes; if those modules spell them differently, adopt their spelling and update
this block in the same commit rather than adding an alias layer.

```rust
// LoopState carries attempts, blocked, unverified, unproductive, computational,
// restarts (u32) and solved (bool); LoopState::{from_value, to_value} serde
// round-trip it to the accumulator JSON. Thresholds carries max_attempts,
// blocked, unverified, stuck, computational, max_restarts.
pub enum Route { Solved, Reported, Retry, Diversify, Blocked }
pub enum Judgement { Proceed, Steer(String), Restart(String) }
pub enum Autonomy { Report, Assisted, Unattended }
pub fn route(state: &LoopState, t: Thresholds) -> Route;
impl Route { pub fn as_str(self) -> &'static str; }   // "solved" | "reported" | …
```

## Ordering

Groups are **strictly ordered**: A (`step/`) → B (`arm/`) → C (`loops/`) → D
(`orchestrate/`). B needs A's write-marker types, C needs B's one-list edge
derivation, D binds a role to the three nodes C emits. **Parallel within a
group:** A1 and A2 are one file each and A3 joins them, while A4 needs only A1
so it can run alongside all of B; B1 and B2 are independent and merge at B3; C1
and C2 are independent and C3 joins them; D1 and D2 are independent. Every task
ends with `cargo test -p tinyloops <module>` and a clippy run over
`--all-targets --all-features -- -D warnings`, except where noted.

## Task A0: dependencies

**Files:** `Cargo.toml`, `crates/tinyloops/Cargo.toml`

1. Add `sha2` to the root `[workspace.dependencies]`, commented: it hashes the
   emitted graph for invariant 9, and `std::hash::DefaultHasher` is documented
   as unstable across releases, so a signature built on it would refuse resumes
   after a toolchain bump rather than after a topology change.
2. In `crates/tinyloops/Cargo.toml` take `sha2`, promote `serde` and
   `serde_json` to dependencies, and extend the dev-dependency `tinyflows`
   entry to `features = ["mock", "testkit"]` — `testkit` supplies
   `TestHarness`, `TestRun`, and `TestRun::assert_no_null_bindings`
   (`vendor/tinyflows/src/testkit/harness.rs:279`), while the interception seam
   is always compiled, so `StepInterceptor` needs no feature. Then `cargo build
   --all-targets --all-features` and `cargo deny check all`.

## Task A1: the capability-typed step context

**Files:** `crates/tinyloops/src/step/mod.rs`, `src/step/types.rs`,
`src/step/test.rs`, `src/error/mod.rs`, `src/error/test.rs`

1. Failing tests: `kernel_context_writes_the_accumulator` (a
   `StepCtx<CanWrite>` accepts `set_state` and the value returns from
   `into_state`) and `an_arm_context_carries_the_base_state_it_was_handed` (a
   `StepCtx<NoWrite>` exposes `base()` and nothing else).
2. Implement in `types.rs`:

   ```rust
   pub trait AccumulatorAccess: sealed::Sealed + Send + Sync + 'static {}
   pub struct CanWrite;  pub struct NoWrite;
   pub struct StepCtx<'a, A: AccumulatorAccess> { /* base, run id, pass */ }
   impl<'a> StepCtx<'a, CanWrite> { pub fn set_state(&mut self, next: LoopState); }
   ```

   `set_state` exists **only** in the `CanWrite` impl block. That is invariant
   11: an arm writing the accumulator is a missing method, not a failed
   assertion.
3. Add `Error::UnknownStep { name }` and `Error::StepFailed { step, reason }`,
   with message assertions in `src/error/test.rs`.

## Task A2: the `Step` trait and the closed registry

**Files:** `crates/tinyloops/src/step/registry.rs`, `src/step/mod.rs`,
`src/step/test.rs`

1. Failing tests: `resolves_a_registered_step_by_name`;
   `rejects_an_unregistered_step_by_name`, asserting `Error::UnknownStep`; and
   `rejects_a_second_registration_of_the_same_name`, so a name in the closed
   set has one meaning. Implement:

   ```rust
   pub trait Step: Send + Sync {
       fn name(&self) -> &'static str;
       fn run(&self, ctx: &mut StepCtx<'_, CanWrite>, input: &Value) -> Result<LoopState>;
   }
   pub struct StepRegistry { /* BTreeMap<&'static str, Arc<dyn Step>> */ }
   // register(&mut self, Arc<dyn Step>) -> Result<()>
   // resolve(&self, &str) -> Result<&Arc<dyn Step>>
   // names(&self) -> impl Iterator<Item = &'static str>
   ```

   `BTreeMap` rather than `HashMap`: `names()` feeds the builder, which must be
   byte-for-byte deterministic for invariant 9's signature.

## Task A3: `run_loop_step`, the one tool a node body is

**Files:** `crates/tinyloops/src/step/invoker.rs`, `src/step/test.rs`,
`src/lib.rs`

1. Failing tests: `runs_the_named_step_and_returns_its_state`;
   `an_unknown_step_name_is_a_node_error`, asserting an
   `EngineError::Capability` whose message names the step;
   `an_unknown_tool_slug_is_a_node_error`; and
   `a_missing_step_argument_is_a_node_error`. The second is the acceptance
   criterion that an unknown step name does not advance the run — a no-op there
   is the failure `assert_no_null_bindings` catches one layer too late.
2. Implement `LoopStepInvoker`, a `tinyflows::caps::ToolInvoker`
   (`vendor/tinyflows/src/caps/mod.rs:137`). Its `invoke` requires `slug ==
   RUN_LOOP_STEP` (`"run_loop_step"`), reads the step name from `args.step` and
   the node's item from `args.input`, and maps every rejection to
   `EngineError::Capability` carrying this crate's `Error` display text. Export
   `RUN_LOOP_STEP`, `Step`, `StepCtx`, `StepRegistry`, and `LoopStepInvoker`
   from `src/lib.rs`.

## Task A4: the arm context does not compile against the accumulator

**Files:** `crates/tinyloops/tests/compile_fail/arm_cannot_write_state.rs`,
`tests/compile_fail.rs`, `crates/tinyloops/Cargo.toml`

1. Add `trybuild` as a dev-dependency, commented as existing to prove invariant
   11 — "this arm wrote the accumulator" is a compile failure, not a review
   comment. Write the case calling `set_state` on a `StepCtx<'_, NoWrite>` and
   its `.stderr` fixture, then run `cargo test -p tinyloops --test
   compile_fail`. Depends only on A1, so it may run alongside group B.

## Task B1: the `Arm` trait

**Files:** `crates/tinyloops/src/arm/mod.rs`, `src/arm/types.rs`,
`src/arm/test.rs`

1. Failing tests: `an_arm_reads_the_report_it_was_handed` and
   `an_arm_returns_a_whole_state_not_a_patch`; a whole `LoopState` is what
   makes B2's fold a delta of two whole values. Implement:

   ```rust
   pub trait Arm: Send + Sync {
       /// Node id and fold key. Declared, never derived from position.
       fn id(&self) -> &'static str;
       /// The step name this arm's node passes to `run_loop_step`.
       fn step(&self) -> &'static str;
       fn evaluate(&self, ctx: &StepCtx<'_, NoWrite>, report: &Value) -> Result<LoopState>;
   }
   ```

   `evaluate` takes the report, never the accumulator: invariant 3, enforced by
   the type — `StepCtx<'_, NoWrite>` cannot reach `=.nodes.<loop>.state`.

## Task B2: the delta fold, and its trait law

**Files:** `crates/tinyloops/src/arm/fold.rs`, `src/arm/test.rs`,
`src/error/mod.rs`

1. Failing tests, all against one base state and hand-built arm outputs:
   - `a_reset_and_an_increment_compose_from_the_same_base` — one arm returns a
     counter of 0 from a base of 3, another returns 4; the fold yields 1.
     Invariant 5's reason for existing, and the test last-writer-wins fails.
   - `a_list_folds_by_what_each_arm_appended`, and
     `two_arms_disagreeing_on_one_scalar_is_a_refused_collision`, asserting an
     `Error::ArmCollision` that names both arms.
   - `folding_is_commutative_over_every_permutation` — four arm outputs, all 24
     permutations, one expected result — and
     `folding_is_associative_over_every_grouping`, the same four folded as
     `((a,b),(c,d))`, `(a,(b,(c,d)))`, and `(((a,b),c),d)`.
2. Implement:

   ```rust
   pub trait ArmFold: Send + Sync {
       /// # Law
       /// Commutative and associative over the arm results, for any base.
       /// Arms complete in the order their work takes, and the engine folds in
       /// deterministic *active-set* order
       /// (`vendor/tinyflows/src/graph/reducer/mod.rs`) — reproducible, not
       /// order-independent. A reducer that reads arrival order returns a
       /// different answer after an unrelated arm rename, and nothing reports it.
       fn fold(&self, base: &LoopState, arms: &[(&str, LoopState)]) -> Result<LoopState>;
   }
   pub struct DeltaFold;
   ```

   Sort by `id()` before folding — a belt to the braces that neither replaces
   the law nor excuses skipping the permutation test.
3. The permutation and association tests are exhaustive over a fixed fixture,
   not generative: `proptest` is a dependency decision and 24 permutations of
   four values cover the property with no new crate.

## Task B3: `ArmSet` — one list, both edge sets

**Files:** `crates/tinyloops/src/arm/set.rs`, `src/arm/test.rs`, `src/lib.rs`

1. Failing tests: `fan_out_and_merge_edges_name_the_same_arms`;
   `removing_an_arm_removes_it_from_both_edge_sets_and_the_fold` — build three,
   drop one, assert all three derived views lost it, invariant 6's acceptance
   criterion that an arm in the fan-out but not the fold runs, costs its
   budget, and changes nothing; `an_empty_arm_set_is_a_construction_error`,
   because a loop with no evaluation cannot end; and
   `duplicate_arm_ids_are_a_construction_error`.
2. Implement:

   ```rust
   pub struct ArmSet { arms: Vec<Arc<dyn Arm>>, fold: Arc<dyn ArmFold> }
   // new(Vec<Arc<dyn Arm>>, Arc<dyn ArmFold>) -> Result<Self>
   // ids() -> Vec<&'static str>
   // fan_out_edges(from: &str) -> Vec<Edge>   merge_edges(to: &str) -> Vec<Edge>
   // merge_inputs() -> Vec<String>
   // fold(&LoopState, &[(&str, LoopState)]) -> Result<LoopState>
   ```

   There is **no** constructor taking two lists. "Every arm converges" and
   "every arm is folded" are one fact because there is one place to say it.
   Export `Arm`, `ArmFold`, `ArmSet`, and `DeltaFold` from `src/lib.rs`.

## Task C1: node identity and the emitted shape

**Files:** `crates/tinyloops/src/loops/mod.rs`, `src/loops/ids.rs`,
`src/loops/test.rs`

1. Failing tests: `emits_the_specified_node_set` — exactly `trigger`, `plan`,
   `research`, `loop`, `attempt`, `side_arms`, one node per arm, `merge`,
   `route`, `pass`, `stand_down`, `report`; then
   `pass_is_the_only_node_with_an_edge_back_to_the_head`, invariant 2 asserted
   on the edge list; `every_route_port_enters_pass`, where all five `Route`
   ports terminate at `pass` and none returns to `attempt`, because an inner
   cycle the head never sees cannot be bounded by `config.max_iterations`;
   `report_is_reachable_only_after_stand_down`; and
   `node_ids_are_declared_not_positional`.
2. Implement `NodeIds`, a struct of `&'static str` constants, and the shape.
   Kinds, from `vendor/tinyflows/src/model/node_kind.rs`: `Trigger` for
   `trigger`; `ToolCall` for `plan`, `research`, `attempt`, every arm, `pass`,
   and `report`; `Loop` for the head; `Merge` for the barrier; `Switch` for
   `route`; `Spawn` for `side_arms`; `Gate` for `stand_down`. Every `ToolCall`
   node names `run_loop_step` with a `step` argument, and none carries a bare
   `agent_ref` — `NodeKind::Agent` would lose the operator-directive drain, the
   salvage of a timed-out attempt, and the arms opened beside the loop.
3. `side_arms` (`Spawn`) and `stand_down` (`Gate`) are this plan's reading of
   "the arms opened beside the loop are started at a named node, at a place a
   checkpoint can land". `Spawn` needs no `TaskRunner`: with none injected the
   work runs inline and the ticket returns already settled
   (`vendor/tinyflows/src/model/node_kind.rs`), so a host without a scheduler
   computes the same answer. Say so in the module docs.

## Task C2: rendering the ladder from `Thresholds`

**Files:** `crates/tinyloops/src/loops/ladder.rs`, `src/loops/test.rs`

1. Failing tests: `renders_every_threshold_from_the_constant`, where the
   rendered program contains each field's value and the graph JSON contains no
   other integer literal in a routing position;
   `the_rendered_program_compiles_and_answers`, evaluating it through
   `tinyflows::expr::evaluate` (`vendor/tinyflows/src/expr.rs:102`) against a
   hand-built scope and asserting a non-null answer — a jq program that fails
   to compile yields `Value::Null` silently, so "it produced a route" is itself
   the assertion; and `rung_order_is_blocked_solved_reported_diversify_retry`,
   one state satisfying two rungs at once, asserted to take the higher.
2. Implement `render_ladder(t: Thresholds) -> String`, an `if/elif` chain over
   the merged state emitting the same strings as `Route::as_str`. No literal is
   typed; every one is interpolated from `t`.

## Task C3: the builder, validated and deterministic

**Files:** `crates/tinyloops/src/loops/builder.rs`, `src/loops/test.rs`

1. Failing tests: `the_emitted_graph_validates`
   (`vendor/tinyflows/src/validate.rs:35`) and `the_emitted_graph_compiles`
   (`vendor/tinyflows/src/compiler.rs:31`);
   `building_twice_emits_byte_identical_json`, the purity C5 rests on; and
   `the_accumulator_update_is_an_assignment_not_an_increment`, asserting the
   head's `config.state.update` contains no `+ 1` against its own previous
   value — invariant 4, where a replayed activation makes `attempts + 1` wrong
   by one and nothing reports it.
2. Implement:

   ```rust
   pub struct LoopBuilder { /* thresholds, autonomy, arms, steps, ids */ }
   // new(Thresholds, ArmSet, StepRegistry) -> Self
   // autonomy(self, Autonomy) -> Self      build(self) -> Result<WorkflowGraph>
   ```

   The head carries `config.state.init`, `config.state.update`,
   `config.max_iterations` (from `t.max_attempts`), `config.until`, and
   `config.on_exceeded`, all documented at
   `vendor/tinyflows/src/nodes/control_flow/loop_node.rs`.
3. `build` returns `Err` when a node names a step absent from the registry, so
   the closed set is checked at build time as well as at call time.

## Task C4: termination as a composable condition

**Files:** `crates/tinyloops/src/loops/termination.rs`, `src/loops/test.rs`

1. Failing tests: `an_exhausted_budget_is_never_success`, asserting `Exhausted`
   and that it is not `Success` — the natural `if done_or_out_of_attempts {
   answer }` violates this by construction, which is why it is a test rather
   than a comment; `a_provider_failure_reports_blocked`;
   `conditions_compose_with_and_and_or`;
   `a_condition_round_trips_through_serde`, so it survives a checkpoint; and
   `resetting_a_fired_condition_clears_it`.
2. Implement `TerminalState { Success, CleanNoOp, Blocked, Stalled, Exhausted
   }` and `TerminationCondition` with `evaluate`, `reset`, serde, and `BitAnd`
   / `BitOr` over boxed conditions. Render the composed condition into the
   head's `config.until` from C3, so the stop test the Rust holds is the one
   the engine runs.

## Task C5: the graph signature and the refused resume

**Files:** `crates/tinyloops/src/loops/signature.rs`, `src/loops/test.rs`,
`src/error/mod.rs`

1. Failing tests: `the_signature_is_stable_across_two_builds`;
   `changing_a_threshold_changes_the_signature`, because the graph is generated
   *from* the thresholds so a constant change is a topology change;
   `adding_an_arm_changes_the_signature`; and
   `resuming_against_a_mismatched_signature_is_a_named_error_and_runs_no_node`,
   asserting `Error::GraphSignatureMismatch { recorded, current }` and that the
   mock capabilities logged zero calls. Implement `GraphSignature`, a SHA-256
   over canonical JSON of node ids, kinds, ports, edges, and every rendered
   threshold, plus `verify_resume(&GraphSignature, &WorkflowGraph)`.

## Task C6: the exhaustive jq-versus-Rust parity sweep

**Files:** `crates/tinyloops/tests/routing_parity.rs`

The load-bearing test of this plan, and an integration test because it must
read only the public surface the way a reviewer would.

1. `the_rendered_ladder_and_the_rust_router_agree_for_every_preset`: for every
   shipped `Thresholds` preset, sweep the cartesian product of `blocked`,
   `unverified`, `unproductive`, and `computational` over `0..=t.field + 1`,
   `attempts` over `0..=t.max_attempts + 1`, and `solved` over both values.
   Evaluate the rendered program with `tinyflows::expr::evaluate` against the
   scope, compare to `Route::as_str(route(state, t))`, and on the first
   disagreement panic naming the preset and the offending counters. A preset
   with a higher `max_attempts` gets a sweep that reaches past it rather than a
   fixed range that stops short and calls the untested room agreement.
2. `restarts` is excluded, and the test says why in a comment: no rung reads
   it, so sweeping it buys nothing but a slower test.
3. `a_ladder_that_fails_to_compile_is_caught_by_the_sweep` feeds a malformed
   program and asserts the harness reports a disagreement rather than passing.
   Under this engine a compile error yields `Value::Null` silently, so the
   sweep must fail closed on null.
4. The sweep proves the *translation*, never the answer: both sides read the
   same number, so a wrong threshold is wrong in both and agrees with itself.
   Say that in the test module's `//!` docs.

## Task C7: a run under the test harness

**Files:** `crates/tinyloops/tests/loop_run.rs`

All through `tinyflows::testkit::TestHarness`.

1. `a_run_completes_and_binds_every_expression` — `assert_completed` and
   `assert_no_null_bindings` (`vendor/tinyflows/src/testkit/harness.rs:279`).
   Needed because a generated routing ladder that fails to compile produces
   null rather than an error, so a green run is not by itself evidence.
2. `pass_runs_exactly_once_per_iteration`, counted off `node_output("pass")`.
3. `an_arm_reading_the_accumulator_routes_one_pass_stale` — mock the attempt
   with a `Respond` returning a **different** answer on each call, rewire one
   arm to read `=.nodes.loop.state`, and assert its route history differs from
   the correctly wired run's. A constant mock passes both ways; say so in a
   comment beside the test so the weaker version is never mistaken for
   coverage. This is invariant 3's only real test.
4. `replaying_one_activation_leaves_every_counter_unchanged` — install a
   `tinyflows::interception::StepInterceptor`
   (`vendor/tinyflows/src/interception.rs`) returning `StepAction::Replace`
   with the merge node's own previous items the second time it sees that node,
   and assert the counters match a single application. An interceptor, not a
   mock capability: what an interceptor returns is *obeyed*, before and after
   every non-trigger activation, while a `RunObserver` callback returns `()`.
5. `a_failed_step_routes_rather_than_aborting` — `StepAction::Fail` on the
   attempt node, asserting the run reaches `report` with `Blocked`.

## Task C8: `Autonomy` shows in the topology

**Files:** `crates/tinyloops/src/loops/builder.rs`, `src/loops/test.rs`

1. Failing tests `assisted_emits_approval_gated_nodes`,
   `unattended_emits_the_same_graph_without_them`, and
   `report_autonomy_emits_no_node_that_acts` — asserted on the emitted nodes
   and edges, never on a prompt string. A prompt instruction is not a control.
2. Implement the `Autonomy` branch in `build`, emitting `NodeKind::Approval`
   nodes. With no `ApprovalProvider` injected an approval falls back to pausing
   the run for `tinyflows::engine::resume`
   (`vendor/tinyflows/src/model/node_kind.rs`, `NodeKind::Approval`), so a host
   that forgets to wire approvals gets a paused run it can see, not an
   unattended one it cannot.

## Task D1: the `TaskBoard`

**Files:** `crates/tinyloops/src/orchestrate/mod.rs`,
`src/orchestrate/board.rs`, `src/orchestrate/test.rs`

1. Failing tests:
   `a_task_carries_an_id_a_statement_a_criterion_a_status_and_a_pass`;
   `reusing_an_id_for_a_different_task_is_an_error`, because every count that
   reads the board across passes depends on stable ids;
   `the_board_round_trips_through_the_accumulator` with every id and status
   intact, which is the checkpoint-and-resume test; and
   `counts_are_readable_without_parsing_prose` — "three of five discharged" is
   a fact only if the tasks are values. Implement `TaskBoard`, `Task`,
   `TaskId`, and `TaskStatus`, all serde.

## Task D2: registration-time constraints

**Files:** `crates/tinyloops/src/orchestrate/role.rs`,
`src/orchestrate/test.rs`, `src/error/mod.rs`

1. Failing tests: `a_shell_tool_in_the_orchestrators_set_fails_construction`,
   asserting the message names the offending tool, and the same for a code
   runner and a file-write tool, one test each;
   `spawning_a_delegate_outside_the_declared_set_is_an_error`;
   `it_does_not_fall_back_to_the_host_registry`, where the host registry holds
   the name, the declared set does not, and the spawn still fails; and
   `the_declared_delegate_set_is_checked_against_the_step_registry`, so a role
   and a registry cannot diverge quietly.
2. Implement `Orchestrator::new(tools: ToolGrant, delegates: DelegateSet) ->
   Result<Self>`, rejecting execution capabilities at construction. Both sets
   are fixed there and have no extend method: a driver that *can* run the
   experiment runs it, and removing the capability removes the option. Add
   `Error::ExecutionToolInOrchestrator { tool }` and `Error::UndeclaredDelegate
   { name }`.

## Task D3: `plan` on a cadence, `attempt` per pass, `report` last

**Files:** `crates/tinyloops/src/orchestrate/steps.rs`,
`src/orchestrate/test.rs`

1. Failing tests:
   - `plan_runs_at_pass_zero_and_then_only_on_its_cadence` — over N passes,
     assert the exact set of passes on which it ran. A board rewritten every
     pass makes "task 3 is still open" stop meaning anything.
   - `attempt_writes_exactly_one_report_per_pass_at_a_known_address` and
     `every_arm_reads_that_one_address`, asserted from `ArmSet` so arm
     independence is structural rather than conventional.
   - `a_timed_out_specialist_yields_a_readable_outcome_and_a_report` — the pass
     still writes a report and does not increment `unproductive` on the
     strength of the timeout alone.
   - `a_killed_specialist_that_wrote_an_artifact_yields_a_salvaged_attempt`,
     citing the artifact. Without salvage, `unproductive` increments on a pass
     that produced work and the ladder spends a diversify on a run that was not
     stuck.
   - `a_directive_drained_from_a_full_mailbox_is_dropped_and_recorded`, and
     `report_is_the_sole_author_of_the_final_answer` — no arm and no specialist
     writes that address.
2. Implement the three as `Step` implementations registered in the
   `StepRegistry`, each returning a whole `LoopState`. None writes the
   accumulator slot; the head does, per invariant 1.

## Task D4: publish and document

**Files:** `crates/tinyloops/src/lib.rs`, `src/loops/README.md`,
`crates/tinyloops/tests/public_api.rs`

1. Re-export `LoopBuilder`, `GraphSignature`, `TerminalState`,
   `TerminationCondition`, `TaskBoard`, and `Orchestrator`. Add
   `loops/README.md` covering the emitted shape, the eleven invariants it
   keeps, and the operational constraint that a threshold change invalidates
   every outstanding checkpoint. Extend `tests/public_api.rs` with a
   build-validate-compile walkthrough using only the public surface.

## Verification

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] `cargo build --all-targets --all-features`, `cargo test --all-features`
- [ ] `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features`
- [ ] `cargo deny check all`

## Invariants discharged

| Tasks | `loop-kernel.md` invariants |
|---|---|
| A1, A4, D3 | 1 (one writer), 11 (capability-typed context) |
| C1 | 2 (one exit) |
| B1, C7 | 3 (arms read the previous step) |
| C3 | 4 (the fold is at-least-once) |
| B2 | 5 (delta fold), 8 (commutative reducer, as a law) |
| B3 | 6 (fan-out and barrier from one list) |
| C2, C6 | 7 (thresholds generated, parity proved) |
| C5, C4 | 9 (graph signature), 10 (composable termination) |
| A2, A3, C3 | the closed step set; the builder is pure |
| C8 | `routing-and-policy.md`: `Autonomy` in the topology |
| D1–D3 | `orchestrator.md`: the three bindings and the five rules |
