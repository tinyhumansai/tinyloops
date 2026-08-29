# Plan: the loop kernel

- **Status:** Not started
- **Specification:** [`../specs/loop-kernel.md`](../specs/loop-kernel.md), with
  the ladder's constants from
  [`../specs/routing-and-policy.md`](../specs/routing-and-policy.md) and the
  role bindings from [`../specs/orchestrator.md`](../specs/orchestrator.md).

## Goal

Build, in order, the four modules that turn `state` and `policy` into a running
loop: `step/`, `arm/`, `loops/`, `orchestrate/`. The end state is a builder that
emits one `tinyflows::model::WorkflowGraph` holding the whole goal run, and a
closed set of Rust steps that the graph's nodes invoke through a single tool.

## Non-goals

- The seams the steps call into. `harness/`, `memory/`, `tools/`, `workspace/`,
  and `ledger/` are [`seams.md`](seams.md).
- `budget/`, `observe/`, `presets/`, and the worked example, which are
  [`observability-and-budget.md`](observability-and-budget.md).
- Anything spanning runs. That is `vendor/tinyflows/crates/adaptive`.

## Assumptions

`crates/tinyloops/src/state/` and `crates/tinyloops/src/policy/` are already
landed by the concurrent module work. This plan consumes them and does not
modify them. The names below are the ones
[`../specs/routing-and-policy.md`](../specs/routing-and-policy.md) fixes; if the
landed modules spell them differently, adopt their spelling and update the
interface block below in the same commit rather than adding an alias layer.

```rust
// crates/tinyloops/src/state/  (assumed, already landed)
pub struct LoopState;                  // serde round-trips to the accumulator JSON
impl LoopState {
    pub fn from_value(v: &serde_json::Value) -> Result<Self>;
    pub fn to_value(&self) -> serde_json::Value;
}
// counters read by the ladder: attempts, blocked, unverified, unproductive,
// computational, restarts (u32), and solved (bool).

// crates/tinyloops/src/policy/  (assumed, already landed)
pub struct Thresholds { /* max_attempts, blocked, unverified, stuck,
                           computational, max_restarts */ }
pub enum Route { Solved, Reported, Retry, Diversify, Blocked }
pub enum Judgement { Proceed, Steer(String), Restart(String) }
pub enum Autonomy { Report, Assisted, Unattended }
pub fn route(state: &LoopState, t: Thresholds) -> Route;
impl Route { pub fn as_str(self) -> &'static str; }   // "solved" | "reported" | …
```

## Ordering

- **Strictly ordered:** group A (`step/`) → group B (`arm/`) → group C
  (`loops/`) → group D (`orchestrate/`). `arm/` needs `StepCtx`'s write-marker
  types from A; `loops/` needs `ArmSet`'s one-list edge derivation from B;
  `orchestrate/` binds a role to the three nodes C emits.
- **Parallel within a group:** A1/A2 are one file each and can be written
  together; A3 depends on both. B1 and B2 are independent (trait versus fold)
  and merge at B3. C1 (ids and shape) and C2 (ladder rendering) are independent;
  C3 joins them. D1 (the board) and D2 (the registration checks) are
  independent.
- Task A0 is the only prerequisite outside the crate's own source and can be
  done first, alone.

## Task A0: dev-dependency features and one workspace dependency

**Files:** `Cargo.toml`, `crates/tinyloops/Cargo.toml`

1. Add `sha2` to the root `[workspace.dependencies]` with the comment this
   workspace expects — it hashes the emitted graph for invariant 9, and
   `std::hash::DefaultHasher` is documented as unstable across releases, so a
   signature built on it would refuse resumes after a toolchain bump rather than
   after a topology change.
2. Take it in `crates/tinyloops/Cargo.toml` as `sha2 = { workspace = true }`.
   Add `serde` and `serde_json` as ordinary dependencies there; both are already
   in `[workspace.dependencies]` and `serde_json` is currently a dev-dependency
   only.
3. Extend the dev-dependency `tinyflows` entry to
   `features = ["mock", "testkit"]`. `testkit` is what supplies `TestHarness`,
   `TestRun`, and `TestRun::assert_no_null_bindings`
   (`vendor/tinyflows/src/testkit/harness.rs:279`); the interception seam itself
   is always compiled, so `StepInterceptor` needs no feature.
4. Run `cargo build --all-targets --all-features` and `cargo deny check all`.

## Task A1: the capability-typed step context

**Files:** `crates/tinyloops/src/step/mod.rs`,
`crates/tinyloops/src/step/types.rs`, `crates/tinyloops/src/step/test.rs`,
`crates/tinyloops/src/error/mod.rs`

1. Write the failing tests in `step/test.rs`:
   - `kernel_context_writes_the_accumulator` — a `StepCtx<CanWrite>` accepts
     `set_state` and the value comes back from `into_state`.
   - `an_arm_context_carries_the_base_state_it_was_handed` — a
     `StepCtx<NoWrite>` exposes `base()` and nothing else.
2. Implement the marker types in `types.rs`:

   ```rust
   pub trait AccumulatorAccess: sealed::Sealed + Send + Sync + 'static {}
   pub struct CanWrite;
   pub struct NoWrite;
   pub struct StepCtx<'a, A: AccumulatorAccess> { /* base, run id, pass */ }
   impl<'a> StepCtx<'a, CanWrite> { pub fn set_state(&mut self, next: LoopState); }
   ```

   `set_state` exists **only** in the `CanWrite` impl block. That is invariant
   11: an arm writing the accumulator is a missing method, not a failed
   assertion.
3. Add `Error::UnknownStep { name: String }` and `Error::StepFailed { step:
   String, reason: String }` to `src/error/mod.rs`, with message assertions in
   `src/error/test.rs`.
4. Run `cargo test -p tinyloops step`.

## Task A2: the `Step` trait and the closed registry

**Files:** `crates/tinyloops/src/step/mod.rs`,
`crates/tinyloops/src/step/registry.rs`,
`crates/tinyloops/src/step/test.rs`

1. Failing tests:
   - `resolves_a_registered_step_by_name`
   - `rejects_an_unregistered_step_by_name` — asserts
     `Error::UnknownStep { name: "nope" }`, not `Ok`.
   - `rejects_a_second_registration_of_the_same_name` — a duplicate is a
     construction error, so the closed set has one meaning per name.
2. Implement:

   ```rust
   pub trait Step: Send + Sync {
       fn name(&self) -> &'static str;
       fn run(&self, ctx: &mut StepCtx<'_, CanWrite>, input: &Value)
           -> Result<LoopState>;
   }
   pub struct StepRegistry { /* BTreeMap<&'static str, Arc<dyn Step>> */ }
   impl StepRegistry {
       pub fn register(&mut self, step: Arc<dyn Step>) -> Result<()>;
       pub fn resolve(&self, name: &str) -> Result<&Arc<dyn Step>>;
       pub fn names(&self) -> impl Iterator<Item = &'static str>;
   }
   ```

   A `BTreeMap` rather than a `HashMap`: `names()` feeds the graph builder, and
   the builder must be byte-for-byte deterministic for invariant 9's signature.
3. Run `cargo test -p tinyloops step`.

## Task A3: `run_loop_step`, the one tool a node body is

**Files:** `crates/tinyloops/src/step/invoker.rs`,
`crates/tinyloops/src/step/test.rs`

1. Failing tests:
   - `runs_the_named_step_and_returns_its_state`
   - `an_unknown_step_name_is_a_node_error` — the invoker returns
     `EngineError::Capability`, and the test asserts the message names the step.
     This is the acceptance criterion "an unknown step name produces a node
     error, and the run does not advance"; a no-op here is the failure
     `assert_no_null_bindings` would only catch one layer too late.
   - `an_unknown_tool_slug_is_a_node_error` — the invoker answers only
     `run_loop_step`.
   - `a_missing_step_argument_is_a_node_error`.
2. Implement `LoopStepInvoker`, a `tinyflows::caps::ToolInvoker`
   (`vendor/tinyflows/src/caps/mod.rs:137`):

   ```rust
   async fn invoke(&self, slug: &str, args: Value, conn: Option<&str>)
       -> tinyflows::error::Result<Value>;
   ```

   `slug` must equal `RUN_LOOP_STEP` (`"run_loop_step"`); `args.step` names the
   step; `args.input` is the node's item. Every rejection maps to
   `EngineError::Capability` carrying this crate's `Error` display text, so a
   graph naming a step that does not exist fails the node rather than routing on
   a state nobody advanced.
3. Export `RUN_LOOP_STEP`, `Step`, `StepCtx`, `StepRegistry`, and
   `LoopStepInvoker` from `crates/tinyloops/src/lib.rs`.
4. Run `cargo test -p tinyloops step` and
   `cargo clippy --all-targets --all-features -- -D warnings`.

## Task A4: the arm context does not compile against the accumulator

**Files:** `crates/tinyloops/tests/compile_fail/arm_cannot_write_state.rs`,
`crates/tinyloops/tests/compile_fail.rs`, `crates/tinyloops/Cargo.toml`

1. Add `trybuild` as a dev-dependency, with a comment saying it exists to prove
   invariant 11 — that "this arm wrote the accumulator" is a compile failure
   rather than a review comment.
2. Write `arm_cannot_write_state.rs` calling `set_state` on a
   `StepCtx<'_, NoWrite>`, and the `.stderr` fixture beside it.
3. `cargo test -p tinyloops --test compile_fail`.

This task may run in parallel with B1–B3; it depends only on A1.

## Task B1: the `Arm` trait

**Files:** `crates/tinyloops/src/arm/mod.rs`,
`crates/tinyloops/src/arm/types.rs`, `crates/tinyloops/src/arm/test.rs`

1. Failing tests:
   - `an_arm_reads_the_report_it_was_handed`
   - `an_arm_returns_a_whole_state_not_a_patch` — the returned value is a full
     `LoopState`, which is what makes the fold in B2 a delta of two whole
     values.
2. Implement:

   ```rust
   pub trait Arm: Send + Sync {
       /// Node id and fold key. Stable, declared, never derived from position.
       fn id(&self) -> &'static str;
       /// The step name this arm's node passes to `run_loop_step`.
       fn step(&self) -> &'static str;
       fn evaluate(&self, ctx: &StepCtx<'_, NoWrite>, report: &Value)
           -> Result<LoopState>;
   }
   ```

   `evaluate` takes the report, never the accumulator: that is invariant 3, and
   the type is what enforces it — `StepCtx<'_, NoWrite>` has no accessor for
   `=.nodes.<loop>.state`.
3. Run `cargo test -p tinyloops arm`.

## Task B2: the delta fold, and its trait law

**Files:** `crates/tinyloops/src/arm/fold.rs`,
`crates/tinyloops/src/arm/test.rs`, `crates/tinyloops/src/error/mod.rs`

1. Failing tests, all against one base state and hand-built arm outputs:
   - `a_reset_and_an_increment_compose_from_the_same_base` — one arm returns a
     counter of 0 from a base of 3, another returns 4; the fold yields 1. This
     is invariant 5's whole reason for existing, and it is the test a
     last-writer-wins fold fails.
   - `a_list_folds_by_what_each_arm_appended`
   - `two_arms_disagreeing_on_one_scalar_is_a_refused_collision` — asserts
     `Error::ArmCollision { field, first, second }` naming both arms.
   - `folding_is_commutative_over_every_permutation` — four arm outputs, all
     24 permutations, one expected result.
   - `folding_is_associative_over_every_grouping` — the same four outputs folded
     as `((a,b),(c,d))`, `(a,(b,(c,d)))`, and `(((a,b),c),d)`.
2. Implement:

   ```rust
   pub trait ArmFold: Send + Sync {
       /// # Law
       /// Commutative and associative over the arm results, for any base.
       /// Arms complete in the order their work takes, and the engine folds in
       /// deterministic *active-set* order
       /// (`vendor/tinyflows/src/graph/reducer/mod.rs`) — reproducible, not
       /// order-independent. A reducer that reads arrival order returns a
       /// different answer after an unrelated arm rename.
       fn fold(&self, base: &LoopState, arms: &[(&str, LoopState)])
           -> Result<LoopState>;
   }
   pub struct DeltaFold;
   ```

   Sort by `id()` before folding — a cheap belt to the braces that does not
   replace the law, and does not excuse skipping the permutation test.
3. The permutation and association tests are exhaustive over a fixed fixture
   rather than generative. Deliberate: `proptest` is a dependency decision, and
   24 permutations of four values covers the property with no new crate. Note
   this in the module docs so the choice is legible.
4. Run `cargo test -p tinyloops arm`.

## Task B3: `ArmSet` — one list, both edge sets

**Files:** `crates/tinyloops/src/arm/set.rs`,
`crates/tinyloops/src/arm/test.rs`

1. Failing tests:
   - `fan_out_and_merge_edges_name_the_same_arms` — asserts the two returned
     edge sets have identical arm-id sets, derived from one list.
   - `removing_an_arm_removes_it_from_both_edge_sets_and_the_fold` — build a set
     of three, drop one, assert all three derived views lost it. This is
     invariant 6's acceptance criterion: an arm in the fan-out but not the fold
     runs, costs its budget, and changes nothing.
   - `an_empty_arm_set_is_a_construction_error` — a loop with no evaluation
     cannot end.
   - `duplicate_arm_ids_are_a_construction_error`.
2. Implement:

   ```rust
   pub struct ArmSet { arms: Vec<Arc<dyn Arm>>, fold: Arc<dyn ArmFold> }
   impl ArmSet {
       pub fn new(arms: Vec<Arc<dyn Arm>>, fold: Arc<dyn ArmFold>) -> Result<Self>;
       pub fn ids(&self) -> Vec<&'static str>;
       pub fn fan_out_edges(&self, from: &str) -> Vec<Edge>;
       pub fn merge_edges(&self, to: &str) -> Vec<Edge>;
       pub fn merge_inputs(&self) -> Vec<String>;   // the merge node's named inputs
       pub fn fold(&self, base: &LoopState, results: &[(&str, LoopState)])
           -> Result<LoopState>;
   }
   ```

   There is **no** constructor taking two lists. "Every arm converges" and
   "every arm is folded" are one fact because there is one place to say it.
3. Export `Arm`, `ArmFold`, `ArmSet`, and `DeltaFold` from `src/lib.rs`.
4. Run `cargo test -p tinyloops arm`.

## Task C1: node identity and the emitted shape

**Files:** `crates/tinyloops/src/loops/mod.rs`,
`crates/tinyloops/src/loops/ids.rs`, `crates/tinyloops/src/loops/test.rs`

1. Failing tests:
   - `emits_the_specified_node_set` — exactly `trigger`, `plan`, `research`,
     `loop`, `attempt`, one node per arm, `merge`, `route`, `pass`,
     `stand_down`, `report`, plus the `side_arms` spawn described below.
   - `pass_is_the_only_node_with_an_edge_back_to_the_head` — invariant 2,
     asserted on the edge list.
   - `every_route_port_enters_pass` — all five `Route` ports of the routing node
     terminate at `pass`; none returns to `attempt`, because an inner cycle the
     head never sees cannot be bounded by `config.max_iterations`.
   - `report_is_reachable_only_after_stand_down`.
   - `node_ids_are_declared_not_positional` — inserting a node in the builder
     leaves every other node's id unchanged.
2. Implement `NodeIds`, a struct of `&'static str` constants, and the shape
   emission. Node kinds, all from `vendor/tinyflows/src/model/node_kind.rs`:
   `Trigger` for `trigger`; `ToolCall` for `plan`, `research`, `attempt`, every
   arm, `route`'s upstream step, `pass`, and `report`; `Loop` for the head;
   `Merge` for the barrier; `Switch` for `route`; `Spawn` for `side_arms`;
   `Gate` for `stand_down`.
3. Every `ToolCall` node's config names `run_loop_step` with a `step` argument
   from A3. No node carries a bare `agent_ref`: `NodeKind::Agent` would lose the
   operator-directive drain, the salvage of a timed-out attempt, and the arms
   opened beside the loop.
4. `side_arms` (`Spawn`) and `stand_down` (`Gate`) are this plan's reading of
   "the arms opened beside the loop are started at a named node, at a place a
   checkpoint can land". `Spawn` needs no `TaskRunner` to be correct — with none
   injected the work runs inline and the ticket returns already settled
   (`vendor/tinyflows/src/model/node_kind.rs`, `NodeKind::Spawn`) — so a host
   without a scheduler computes the same answer. Record that in the module docs.
5. Run `cargo test -p tinyloops loops`.

## Task C2: rendering the ladder from `Thresholds`

**Files:** `crates/tinyloops/src/loops/ladder.rs`,
`crates/tinyloops/src/loops/test.rs`

1. Failing tests:
   - `renders_every_threshold_from_the_constant` — the rendered program contains
     each `Thresholds` field's value and the graph JSON contains no other
     integer literal in a routing position.
   - `the_rendered_program_compiles_and_answers` — evaluate it through
     `tinyflows::expr::evaluate` (`vendor/tinyflows/src/expr.rs:102`) against a
     hand-built scope and assert a non-null answer. A jq program that fails to
     compile yields `Value::Null` silently, so "it produced a route" is itself
     the assertion.
   - `rung_order_is_blocked_solved_reported_diversify_retry` — one state
     satisfying two rungs at once, asserted to take the higher.
2. Implement `render_ladder(t: Thresholds) -> String`, an `if/elif` chain over
   the merged state emitting the same strings as `Route::as_str`. No literal is
   typed; every one is interpolated from `t`.
3. Run `cargo test -p tinyloops loops`.

## Task C3: the builder, validated and deterministic

**Files:** `crates/tinyloops/src/loops/builder.rs`,
`crates/tinyloops/src/loops/test.rs`

1. Failing tests:
   - `the_emitted_graph_validates` — `tinyflows::validate::validate`
     (`vendor/tinyflows/src/validate.rs:35`) returns `Ok`.
   - `the_emitted_graph_compiles` — `tinyflows::compiler::compile`
     (`vendor/tinyflows/src/compiler.rs:31`) returns `Ok`.
   - `building_twice_emits_byte_identical_json` — the purity invariant the
     signature in C5 rests on.
   - `the_accumulator_update_is_an_assignment_not_an_increment` — assert the
     head's `config.state.update` program contains no `+ 1` against its own
     previous value. Invariant 4: replayed after a resume, `attempts + 1` is
     wrong by one and nothing reports it.
2. Implement:

   ```rust
   pub struct LoopBuilder { /* thresholds, autonomy, arms, steps, ids */ }
   impl LoopBuilder {
       pub fn new(thresholds: Thresholds, arms: ArmSet, steps: StepRegistry) -> Self;
       pub fn autonomy(self, autonomy: Autonomy) -> Self;
       pub fn build(self) -> Result<WorkflowGraph>;
   }
   ```

   The head carries `config.state.init`, `config.state.update`,
   `config.max_iterations` (from `t.max_attempts`), `config.until`, and
   `config.on_exceeded`, all documented at
   `vendor/tinyflows/src/nodes/control_flow/loop_node.rs`.
3. `build` returns `Err` when a step named by a node is absent from the
   registry, so the closed set is checked at build time as well as at call time.
4. Run `cargo test -p tinyloops loops`.

## Task C4: termination as a composable condition

**Files:** `crates/tinyloops/src/loops/termination.rs`,
`crates/tinyloops/src/loops/test.rs`

1. Failing tests:
   - `an_exhausted_budget_is_never_success` — asserts `Exhausted`, and asserts
     it is not `Success`. The natural `if done_or_out_of_attempts { answer }`
     violates this by construction, which is why it is a test and not a comment.
   - `a_provider_failure_reports_blocked`
   - `conditions_compose_with_and_and_or`
   - `a_condition_round_trips_through_serde` — it survives a checkpoint with the
     rest of the state.
   - `resetting_a_fired_condition_clears_it`
2. Implement `TerminalState { Success, CleanNoOp, Blocked, Stalled, Exhausted }`
   and `TerminationCondition` with `evaluate`, `reset`, serde, and `BitAnd` /
   `BitOr` impls over boxed conditions.
3. Render the composed condition into the head's `config.until` from C3, so the
   stop test the Rust holds is the stop test the engine runs.
4. Run `cargo test -p tinyloops loops`.

## Task C5: the graph signature and the refused resume

**Files:** `crates/tinyloops/src/loops/signature.rs`,
`crates/tinyloops/src/loops/test.rs`, `crates/tinyloops/src/error/mod.rs`

1. Failing tests:
   - `the_signature_is_stable_across_two_builds`
   - `changing_a_threshold_changes_the_signature` — the graph is generated
     *from* the thresholds, so a constant change is a topology change.
   - `adding_an_arm_changes_the_signature`
   - `resuming_against_a_mismatched_signature_is_a_named_error_and_runs_no_node`
     — asserts `Error::GraphSignatureMismatch { recorded, current }` and asserts
     the mock capabilities logged zero calls.
2. Implement `GraphSignature`, a SHA-256 over canonical JSON of node ids, kinds,
   ports, edges, and every rendered threshold, plus
   `verify_resume(recorded: &GraphSignature, graph: &WorkflowGraph)`.
3. Run `cargo test -p tinyloops loops`.

## Task C6: the exhaustive jq-versus-Rust parity sweep

**Files:** `crates/tinyloops/tests/routing_parity.rs`

This is the load-bearing test of the whole plan. It is an integration test
because it must read only the public surface — the rendered ladder and
`policy::route` — the way a reviewer would.

1. Failing test `the_rendered_ladder_and_the_rust_router_agree_for_every_preset`:
   for every shipped `Thresholds` preset, sweep the cartesian product of
   `blocked`, `unverified`, `unproductive`, `computational` over `0..=t.field+1`,
   `attempts` over `0..=t.max_attempts+1`, and `solved` over both values.
   Evaluate the rendered program with `tinyflows::expr::evaluate` against the
   scope, compare to `Route::as_str(route(state, t))`, and on the first
   disagreement panic naming the preset and the offending counters.
2. `restarts` is excluded from the sweep and the test says why in a comment: the
   ladder does not read it, and sweeping a counter no rung consults buys nothing
   but a slower test.
3. A second test, `a_ladder_that_fails_to_compile_is_caught_by_the_sweep`,
   feeds a deliberately malformed program and asserts the harness reports a
   disagreement rather than passing. Under this engine a compile error yields
   `Value::Null` silently, so the sweep must fail closed on null.
4. The sweep proves the *translation*, never the answer: both sides read the same
   number, so a wrong threshold is wrong in both and agrees with itself. State
   that in the test module's `//!` docs so nobody reads a green sweep as
   validation of the constants.
5. Run `cargo test -p tinyloops --test routing_parity`.

## Task C7: a run under the test harness

**Files:** `crates/tinyloops/tests/loop_run.rs`

1. Failing tests, all through `tinyflows::testkit::TestHarness`:
   - `a_run_completes_and_binds_every_expression` — `assert_completed` and
     `assert_no_null_bindings` (`vendor/tinyflows/src/testkit/harness.rs:279`).
     Needed because a generated routing ladder that fails to compile produces
     null rather than an error, so a green run is not by itself evidence.
   - `pass_runs_exactly_once_per_iteration` — counted off `node_output("pass")`.
   - `an_arm_reading_the_accumulator_routes_one_pass_stale` — mock the attempt
     with a `Respond` that returns a **different** answer on each call, rewire
     one arm to read `=.nodes.loop.state`, and assert the run's route history
     differs from the correctly wired run's. A constant mock passes both ways;
     say so in a comment beside the test so the weaker version is never
     mistaken for coverage. This is invariant 3's only real test.
   - `replaying_one_activation_leaves_every_counter_unchanged` — install a
     `tinyflows::interception::StepInterceptor`
     (`vendor/tinyflows/src/interception.rs`) that returns
     `StepAction::Replace` with the merge node's own previous items the second
     time it sees that node, and assert the counters match a single
     application. An interceptor rather than a mock capability because what an
     interceptor returns is *obeyed*, before and after every non-trigger
     activation, while a `RunObserver` callback returns `()` and cannot inject
     a replay.
   - `a_failed_step_routes_rather_than_aborting` — `StepAction::Fail` on the
     attempt node, asserting the run reaches `report` with `Blocked`.
2. Run `cargo test -p tinyloops --test loop_run`.

## Task C8: `Autonomy` shows in the topology

**Files:** `crates/tinyloops/src/loops/builder.rs`,
`crates/tinyloops/src/loops/test.rs`

1. Failing tests:
   - `assisted_emits_approval_gated_nodes`
   - `unattended_emits_the_same_graph_without_them`
   - `report_autonomy_emits_no_node_that_acts`
   All three assert on the emitted graph's nodes and edges, never on a prompt
   string. A prompt instruction is not a control.
2. Implement the `Autonomy` branch in `build`, emitting `NodeKind::Approval`
   nodes. With no `ApprovalProvider` injected an approval falls back to pausing
   the run for `tinyflows::engine::resume`
   (`vendor/tinyflows/src/model/node_kind.rs`, `NodeKind::Approval`), so a host
   that forgets to wire approvals gets a paused run it can see rather than an
   unattended one it cannot.
3. Run `cargo test -p tinyloops loops`.

## Task D1: the `TaskBoard`

**Files:** `crates/tinyloops/src/orchestrate/mod.rs`,
`crates/tinyloops/src/orchestrate/board.rs`,
`crates/tinyloops/src/orchestrate/test.rs`

1. Failing tests:
   - `a_task_carries_an_id_a_statement_a_criterion_a_status_and_a_pass`
   - `reusing_an_id_for_a_different_task_is_an_error` — every count that reads
     the board across passes depends on ids being stable.
   - `the_board_round_trips_through_the_accumulator` — into `LoopState` and back
     with every id and status intact, which is the checkpoint-and-resume test.
   - `counts_are_readable_without_parsing_prose` — "three of five discharged" is
     a fact only if the tasks are values.
2. Implement `TaskBoard`, `Task`, `TaskId`, `TaskStatus`, all serde.
3. Run `cargo test -p tinyloops orchestrate`.

## Task D2: registration-time constraints

**Files:** `crates/tinyloops/src/orchestrate/role.rs`,
`crates/tinyloops/src/orchestrate/test.rs`,
`crates/tinyloops/src/error/mod.rs`

1. Failing tests:
   - `a_shell_tool_in_the_orchestrators_set_fails_construction` — asserts the
     error message names the offending tool. Same for a code runner and a
     file-write tool, one test each.
   - `spawning_a_delegate_outside_the_declared_set_is_an_error`
   - `it_does_not_fall_back_to_the_host_registry` — the host registry holds the
     name, the declared set does not, and the spawn still fails.
   - `the_declared_delegate_set_is_checked_against_the_step_registry` — every
     name the role may spawn resolves, so a role and a registry cannot diverge
     quietly.
2. Implement `Orchestrator::new(tools: ToolGrant, delegates: DelegateSet)
   -> Result<Self>`, rejecting execution capabilities at construction. Both sets
   are fixed there and have no extend method: a driver that *can* run the
   experiment runs it, and removing the capability removes the option.
3. Add `Error::ExecutionToolInOrchestrator { tool }` and
   `Error::UndeclaredDelegate { name }`.
4. Run `cargo test -p tinyloops orchestrate`.

## Task D3: `plan` on a cadence, `attempt` per pass, `report` last

**Files:** `crates/tinyloops/src/orchestrate/steps.rs`,
`crates/tinyloops/src/orchestrate/test.rs`

1. Failing tests:
   - `plan_runs_at_pass_zero_and_then_only_on_its_cadence` — over N passes,
     assert the exact set of passes on which it ran. A board rewritten every
     pass makes "task 3 is still open" stop meaning anything.
   - `attempt_writes_exactly_one_report_per_pass_at_a_known_address`
   - `every_arm_reads_that_one_address` — asserted from `ArmSet`, so arm
     independence is a structural fact rather than a convention.
   - `a_timed_out_specialist_yields_a_readable_outcome_and_a_report` — the pass
     still writes a report, and does not increment `unproductive` on the
     strength of the timeout alone.
   - `a_killed_specialist_that_wrote_an_artifact_yields_a_salvaged_attempt` —
     the report cites the artifact. Without salvage, `unproductive` increments
     on a pass that produced work and the ladder spends a diversify on a run
     that was not stuck.
   - `a_directive_drained_from_a_full_mailbox_is_dropped_and_recorded`
   - `report_is_the_sole_author_of_the_final_answer` — no arm and no specialist
     writes that address.
2. Implement the three steps as `Step` implementations registered in the
   `StepRegistry`, each returning a whole `LoopState`. None writes the
   accumulator slot: the head does, per invariant 1.
3. Run `cargo test -p tinyloops orchestrate`.

## Task D4: publish and document

**Files:** `crates/tinyloops/src/lib.rs`,
`crates/tinyloops/src/loops/README.md`,
`crates/tinyloops/tests/public_api.rs`

1. Re-export `LoopBuilder`, `GraphSignature`, `TerminalState`,
   `TerminationCondition`, `TaskBoard`, and `Orchestrator`.
2. Add a `loops/README.md` covering the emitted shape, the eleven invariants it
   keeps, and the operational constraint that a threshold change invalidates
   every outstanding checkpoint.
3. Extend `tests/public_api.rs` with a build-validate-compile walkthrough using
   only the public surface.
4. Run `cargo test --doc` and
   `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features`.

## Verification

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] `cargo build --all-targets --all-features`
- [ ] `cargo test --all-features`
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
| C5 | 9 (checkpoint carries the graph signature) |
| C4 | 10 (termination as a composable condition) |
| A2, A3, C3 | the closed step set, and the constraint that the builder is pure |
| C8 | `routing-and-policy.md`: `Autonomy` visible in the topology |
| D1–D3 | `orchestrator.md`: the three bindings and the five rules |
