# Loop kernel

- **Status:** Accepted — 2026-08-29
- **Owner:** Maintainers
- **Related:** [`routing-and-policy.md`](routing-and-policy.md),
  [`orchestrator.md`](orchestrator.md),
  [`adaptation.md`](adaptation.md) — which proposes amending invariant 7,
  [ADR 0002](../adr/0002-loop-on-the-tinyflows-graph.md),
  [ADR 0004](../adr/0004-routing-in-the-graph-steps-in-rust.md)

## Problem

A goal run is a loop: attempt something, evaluate what came back, decide where
to go next, and stop when a verdict or a budget says so. Written by hand that
loop is a `while` with a counter, and it fails in ways that are invisible while
it runs — the counter lives in a local so a crash loses it, a mid-loop branch
reads a value one pass stale, two evaluators write the same field and the last
one wins, the thresholds in the code and the thresholds in the documentation
disagree, and a resumed run continues against a loop body that has since
changed shape.

`vendor/tinyflows` already executes a graph with checkpoint and resume, fan-out
with a real barrier, and a bounded loop head whose counter and accumulator live
in run state. What it does not have is the *shape* of a goal run. This
specification defines that shape: what the kernel emits, what lives where, and
which properties every implementation has to keep.

## Goals

- Define a builder that emits one `tinyflows::model::WorkflowGraph`
  (`vendor/tinyflows/src/model/mod.rs:148`) holding the whole loop, so the run
  is one engine run rather than a driver calling the engine repeatedly.
- Fix where run state lives, who may write it, and how it is addressed.
- Fix how concurrent evaluation arms converge and how their answers fold.
- Make the graph's routing and the crate's Rust routing provably the same
  decision.
- Make an incompatible resume an error rather than silent corruption.

## Non-goals

- Choosing a model, a tool vendor, or a harness. Every effect crosses a
  `tinyflows::caps` trait the embedder implements
  (`vendor/tinyflows/src/caps/mod.rs`).
- Anything that spans runs — ledger rows, scored lessons, workflow selection,
  promotion. That is `vendor/tinyflows/crates/adaptive`, and this crate does not
  restate it. See [ADR 0003](../adr/0003-three-layer-split-with-tinyflows-adaptive.md).
- The routing ladder's constants and their rationale, which are
  [`routing-and-policy.md`](routing-and-policy.md).
- An implementation order. That belongs in `docs/plans/`.

## Proposed behavior

### The thesis

**The graph owns routing; Rust owns the steps.** Every branch a run can take is
declared in the emitted graph, where it is inspectable, renderable, and
comparable against the Rust that generated it. Everything a node *does* is a
Rust step, compiled and tested against the crate's own types.

### The shape

The kernel is a builder. Given a `Thresholds`, a step set, and a list of
evaluation arms, it emits one graph:

```text
trigger → plan → research(once) → loop head ──done──→ stand_down → report
                                      │
                                      └─ body → attempt ─┬→ reflect ──┐
                                            ▲            ├→ judge ────┤
                                            │            ├→ …arms… ───┼→ merge → route
                                            └──────────── pass ←──────┘
```

- `trigger` is `NodeKind::Trigger`; `plan` and `research` run once, before the
  loop, so the first attempt already has a decomposition and a context to work
  from rather than spending the first pass acquiring them.
- The loop head is `NodeKind::Loop`
  (`vendor/tinyflows/src/nodes/control_flow/loop_node.rs`). It carries the run's
  accumulator in `config.state.init` / `config.state.update`, its ceiling in
  `config.max_iterations`, its stop test in `config.until`, and its
  cap behavior in `config.on_exceeded`.
- The body is one `attempt` node, a fan-out to the evaluation arms, a
  `NodeKind::Merge` barrier, and a routing node.
- Every route — every terminal verdict, a retry, and a diversify once its arms
  have converged — enters `pass`, and `pass` is the only node with an edge back
  to the head. That closing edge is a back-edge the engine lowers as a plain
  re-entry rather than a fan-in barrier.
- `done` leaves the loop into `stand_down`, which retires anything still running
  beside the run, and then `report`.

### Node bodies

A node body is **one registered tool**, invoked through `NodeKind::ToolCall` and
the host's `ToolInvoker` (`vendor/tinyflows/src/caps/mod.rs:137`), named
`run_loop_step` and taking the step name as an argument. The step set is
**closed**: an unrecognised step name is an error from the tool, never a no-op.

Two node kinds sit beside that rule rather than under it, and the boundary is
worth stating because the specifications otherwise leave it open. Every node
*inside* the loop body — `attempt`, each evaluation arm, `merge`, `pass` — is a
`ToolCall` running a named step, so the whole pass is one closed set the graph
addresses by name. The work opened **beside** the loop is different: it does not
gate the pass, it outlives it, and it has to be retired at the end. That pair
uses `NodeKind::Spawn` to start tasks and `NodeKind::Gate` to collect or cancel
them (see [`orchestrator.md`](orchestrator.md)), which is what makes
`stand_down` a node the graph can reach rather than a cleanup call somebody
remembers to make after the workflow returns.

The reason to draw it here rather than let each builder decide: work that was
cancelled *after* the loop returned is work paid for and thrown away. A live run
of this design recorded its verdict at minute 29, kept spawning helpers for
another 62 minutes because nothing had retired them, and spent roughly 85% of
its wall clock and most of its budget after the problem was already solved.
A graph naming a step that does not exist would otherwise run green, change
nothing, and route on a state nobody advanced — which is exactly the class of
failure `assert_no_null_bindings`
(`vendor/tinyflows/src/testkit/harness.rs:279`) exists to catch, arriving one
layer too late to be caught by it.

`NodeKind::Agent` with a bare `agent_ref` is not enough, and the three things it
loses are each load-bearing:

- **The operator-directive drain.** An external control surface reaches a
  running loop by posting into a mailbox that `attempt` drains. A node that only
  names an agent accepts direction and acts on none of it.
- **The salvage of a timed-out attempt.** The ordinary way a long attempt ends
  is its own cap killing it, which destroys its report and leaves every artifact
  it wrote. Salvage is what turns that into a usable attempt rather than a blank
  one.
- **The arms opened beside the loop.** Work started but not waited on — a
  literature sweep, a background check — is started at a named node, at a place
  a checkpoint can land, rather than as a side effect inside whichever step
  happened to be holding the handles.

### State addressing

The accumulator is the run's state. The head seeds it from `state.init` and
folds each pass's body output into it with `state.update`, and it is addressable
from any expression in the graph as `=.nodes.<loop id>.state`, with the pass
counter alongside it as `=.nodes.<loop id>.iteration`. Both are written to the
head's own run-state slot, so both survive checkpoint and resume.

Expressions are the engine's `=`-prefix convention
(`vendor/tinyflows/src/expr.rs`): jq via `jaq`, resolved against a scope holding
`item`, `items`, `run`, `inputs`, and `nodes`. **A compile error, a run error,
non-JSON output, and empty output all yield `Value::Null`, silently.** Every
rule below that concerns expressions exists because of that sentence.

## Invariants and constraints

Eleven invariants. Each is a property an implementation must keep, and each is
stated with the failure it prevents.

### 1. One writer

Only the loop head writes the accumulator. `state.init` seeds it, `state.update`
folds it, and no other node writes that slot. A step returns a whole state; the
head replaces the accumulator with what came back.

*Why.* The head is the accumulator's sole writer in the engine's own design,
which is what removes the question of which branch wrote last and how a
concurrent write reduces. Letting an arm write the slot reintroduces every one
of those questions, and none of them has an answer the engine reports.

### 2. One exit

Every terminal verdict, every retry, and every diversify routes through a single
`pass` node, and `pass` alone closes the cycle.

*Why.* The engine's `nodes` scope is **cumulative**: a node's output stays
addressable long after the pass that produced it. A fold written as "the merge
if it ran, else the reflection" therefore reads a merge from three passes ago
for the rest of the run, silently reverting the state on each fold. A node that
every pass runs is never stale. Routing the merge straight back to `attempt`
looks simpler and is worse: it creates an inner cycle the head never sees, so
`config.max_iterations` cannot bound it and a run that keeps diversifying never
terminates.

### 3. Arms read the previous step, never the accumulator

An evaluation arm reads the node immediately upstream of it — the attempt
report, or the merge — and never `=.nodes.<loop>.state`.

*Why.* The head folds at the **top** of a pass. Mid-body, the accumulator holds
the state as of the *previous* pass, so an arm reading it is routing on a stale
answer. This is worth stating explicitly because it is undetectable by the
obvious test: a mock that returns a constant produces the same value on every
pass, so one-pass-behind and current are indistinguishable. Only a mock whose
answer varies per call can fail on it.

### 4. The fold is at-least-once

Every accumulator update must be an **assignment** — the next value computed
whole — never an increment of the previous one.

*Why.* An activation replayed after a resume applies `state.update` twice. The
engine's own module docs record this for the iteration counter and note that an
idempotent update is immune. `attempts + 1` executed twice is wrong by one and
nothing reports it; `attempts = <count from the step's returned state>` executed
twice is right.

### 5. The merge folds by delta

Every arm is handed the same base state and returns a whole state. The merge
computes each arm's value *minus the base* and sums those deltas onto the base.
Lists fold by what was appended past the base's length; text and flags fold by
difference, and two arms differing on one scalar field is a collision the fold
refuses rather than resolves.

*Why.* Delta folding is what lets a reset and an increment compose in the same
superstep. One arm zeroes a counter (delta −3) while another increments it
(delta +1), both from the same base, and the result is the base minus two rather
than whichever arm happened to be folded last. The alternative — a table saying
which arm owns which field — drifts the first time an arm learns to write
somewhere new.

### 6. Fan-out and barrier come from one list

The arm list is declared once. The builder derives both the fan-out edges from
`attempt` and the convergence edges into the merge from that same list, and the
merge folds exactly the arms that list names.

*Why.* "Every arm converges" and "every arm is folded" must be the same fact. As
two facts they can drift, and the drift is silent: an arm added to the fan-out
but not to the fold runs, costs its budget, and changes nothing.

### 7. Thresholds are generated, and parity is proved

Every number in the graph's routing ladder is rendered from the Rust `Thresholds`
constant. No threshold is typed into graph JSON. A parity harness replays the
generated jq and the Rust routing function over **every** combination of the
counters across a range that reaches past every threshold, and asserts they
agree on all of them.

*Why.* Two engines deciding the same run differently is invisible in a live run
and obvious only in a diff. A ladder reading `>` where the Rust reads `>=`
changes when a run diversifies and fails nothing. Sharing the numbers removes
the class of failure where the two simply disagree about a constant; the
exhaustive sweep removes the class where they disagree about a comparison. The
sweep is a pure function of a handful of small-range integers on both sides, so
it is cheap enough that sampling would buy nothing and could miss exactly the
off-by-one it exists to catch.

The harness cannot check whether the shared answer is *right* — both sides read
the same number, so a wrong threshold is wrong in both and agrees with itself.
That is what the rationale in [`routing-and-policy.md`](routing-and-policy.md)
is for.

### 8. The merge reducer is commutative, as a trait law

The reducer that folds arm results is documented and tested as commutative and
associative. Its trait carries that as a stated law, and the test suite proves it
over permuted arm orders.

*Why.* Arms complete in whatever order their work takes. `tinyflows` folds
channel updates "in deterministic active-set order"
(`vendor/tinyflows/src/graph/reducer/mod.rs`) — deterministic means
*reproducible*, not *order-independent*. The active set changes whenever an arm
is added, removed, or renamed, so a reducer that depends on arrival order
produces a different answer for the same evidence after an unrelated edit, and
nothing in the engine reports it. In the wider class of superstep engines the
ordering is left arbitrary outright, so a kernel that relied on it would not
survive a change of host. Sorting arms by a self-stamped tag before folding is
a cheap belt to the braces, and does not replace the law.

### 9. A checkpoint carries the graph's signature

Every checkpoint records a signature hash over the emitted graph — node ids,
kinds, ports, edges, and every rendered threshold. Resume verifies the signature
and refuses a mismatch with a named error. Node and executor identity is
**declared** — a stable id chosen by the builder — never derived from allocation
or insertion order.

*Why.* The graph is generated *from* the thresholds, so changing a constant
changes the topology. Resuming a checkpoint taken against the old topology onto
the new one restores state into slots that no longer mean what they meant, which
is silent corruption rather than a crash. Identity derived from allocation order
has the same failure in miniature: adding a node renumbers its neighbours and a
resumed run replays the wrong step.

### 10. Termination is a composable condition

Stopping is a `TerminationCondition` — stateful, resettable, serializable, and
combinable with `&` and `|` — evaluated over a named terminal state:

```text
Success | CleanNoOp | Blocked | Stalled | Exhausted
```

`CleanNoOp` is a run that correctly determined there was nothing to do.
`Blocked` is infrastructure, not the work. `Stalled` is a saturation detector
firing. `Exhausted` is a budget. The condition is resettable so a restart can
clear it, and serializable so it survives a checkpoint with the rest of the
state.

**An error or an exhausted budget is never `Success`.** That is the invariant,
and it is stated because the natural shape of a hand-written loop — `if
done_or_out_of_attempts { return the answer }` — violates it by construction.

*Why not an `if`.* An `if` cannot be reset, cannot be serialized, cannot be
composed with another stopping rule without editing it, and cannot say which of
five reasons ended the run. A run that stops has to report why it stopped, or
its outcome cannot be scored.

### 11. What a step may emit is in its type

A step receives a capability-typed context. A step registered as an evaluation
arm is handed a context with no accumulator-write capability, so "this arm wrote
the accumulator" does not compile. The same typing governs which steps may
delegate, which may write artifacts, and which may post to a mailbox.

*Why.* Invariant 1 is otherwise a convention, and a convention is checked by
review. A capability-typed context makes it checked by the compiler. This is the
kernel's instance of the house rule: **a prompt instruction is not a control**,
and neither is a doc comment.

### Constraints

- The kernel depends on `tinyflows` only. `tinyflows-adaptive` and `tinyagents`
  stay behind optional cargo features, because a loadable `cdylib` must not
  resolve their persistence backends or HTTP clients.
- The builder is pure: given the same inputs it emits the same graph, byte for
  byte, so the signature hash of invariant 9 is stable.
- Every emitted graph passes `tinyflows::validate` before it is returned.

## Acceptance criteria

- The builder emits a graph that compiles under `tinyflows::compiler::compile`
  and validates, for the default thresholds and for every shipped preset.
- A run under `tinyflows::testkit::TestHarness` completes, runs `pass` exactly
  once per iteration, and passes `assert_no_null_bindings`.
- A test whose attempt mock returns a **different** answer on each call fails if
  an evaluation arm is rewired to read `=.nodes.<loop>.state`, and passes when it
  reads its upstream node. A constant mock passes both ways, and the suite says
  so in a comment so the weaker test is not mistaken for coverage.
- Replaying the same activation twice through a `StepInterceptor`
  (`vendor/tinyflows/src/interception.rs`) leaves every counter unchanged from a
  single application.
- A property test asserts the merge reducer returns the same state for every
  permutation of the same arm outputs, and for arms grouped in any association.
- Removing an arm from the declared list removes it from both the fan-out and
  the fold; a test asserts the two edge sets are derived from one list and cannot
  be set independently.
- The parity harness sweeps the full counter space for every shipped
  `Thresholds` and reports the first disagreeing state, naming which preset
  disagreed.
- Resuming a checkpoint whose recorded signature does not match the current
  graph returns a named error and runs no node.
- Attempting to write the accumulator from an evaluation arm's context is a
  compile failure, proved by a `trybuild`-style test.
- A run that ends on an exhausted budget reports `Exhausted`, never `Success`,
  and a run that ends on a provider failure reports `Blocked`.
- An unknown step name returned to `run_loop_step` produces a node error, and
  the run does not advance.

## Open questions

- Whether the signature hash covers rendered prompt text as well as topology. It
  is not part of routing, but a resumed run that inherits a rewritten brief is a
  different run in every way that matters to the reader of its report.
- Whether `stand_down` should be able to run on a `Blocked` exit without waiting
  for arms that are themselves blocked on the same provider, or whether that is
  the operator's call through an approval gate.
- Whether the saturation detector behind `Stalled` belongs in the kernel or in
  the policy layer. It reads counters the policy owns, but it must be evaluated
  by the head's `config.until`, which the kernel renders.
