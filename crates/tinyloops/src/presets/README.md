# `presets/`

The batteries: threshold sets that say what they are betting, two evaluation
arms that keep a verdict mechanical, and a loop with every seam filled in.

## Why the module exists

Everything else in this crate is a part. A consumer handed only traits has to
make the two most consequential decisions in the design again, alone, and both
have a wrong answer that looks fine in a demo.

## The public surface

| Item | What it is |
| --- | --- |
| `Preset` | A named threshold set, with its bet in rustdoc. `Preset::ALL` is what the parity sweep iterates |
| `Reflect` | *Is the answer right?* The only arm that may end the run |
| `Judge` | *Was the pass conducted acceptably?* It corrects; it never concludes |
| `AssembledLoop`, `research_loop` | A loop that emits a graph and drives itself |
| `Driven` | How a driven run came out: the final accumulator, the outcome, the routes, the bound |
| `Gather`, `ArmStep`, `Advance`, `Converge` | The node bodies for the kernel nodes that are not the orchestrator's |

## Design

### The two anti-confabulation rules

**A `solved` verdict needs three things at once**: the literal `SOLVED_MARKER`
in a reply, at least one artifact from the pass, and internal consistency — the
specialist that claimed it is the one that left something behind. Any one alone
is a claim. The conjunction is evidence.

This is the one control a loop has over a verifier that is itself a model, whose
self-preference and position bias *grow* as the quality gap narrows, which is
precisely the regime a converging run is in. The third condition is the one a
naive "marker AND artifact" check misses, and it is tested separately for that
reason.

**An unreadable verdict is the cheap outcome.** A judge that cannot parse the
report returns `Judgement::Proceed`, never `Restart`. The asymmetry is the whole
rule: reading a serialization slip as a restart throws away a run's work, which
is a far worse failure than one wasted pass.

### Why the question is split in two

`Reflect` answers whether the answer is right; `Judge` answers whether the pass
was conducted acceptably. An arm that can say "good work" and an arm that can
say "we are done" answering the same prompt is one arm with two names, and
`ArmSet` refuses a second concluding arm at construction for the same reason.

Both arms read the typed `AttemptReport` as *input*, never as their own prior
assistant turn. Relabelling an identical erroneous claim away from the assistant
role raises the explicit correction rate by 23 to 93 percentage points across
most model and domain pairs. The fan-out shape makes that natural, and it must
not be optimised into a follow-up turn.

### A preset carries its bet

`stuck` is an estimator of the point where sequential revision stops beating
parallel sampling, and where that point sits depends entirely on how accurate a
domain's feedback is. `Persistent` bets that feedback is accurate enough to keep
revising; `Exploratory` bets it is not. A number with no rationale beside it is
a number nobody can argue with, revise, or tune per domain.

`Preset::ALL` is the list `src/policy/test.rs` sweeps, so a preset cannot be
added without its generated jq ladder being proved against the Rust `route`
exhaustively over the bounded counter space.

### Two ways to run, one routing

`AssembledLoop::graph` emits the `WorkflowGraph` an engine runs.
`AssembledLoop::drive` runs the same loop in this process. They are not two
implementations of the routing: both resolve to `route`, and the graph's ladder
is generated from the same constants and proved against that function. What
differs is what owns the concurrency and the durability.

`drive` exists because the engine's mock capabilities are a dev-only dependency
here, so the shipped library cannot start a graph run, and because a loop you
can call from a test with no runtime, no scheduler, and no provider is the loop
most people should meet first.

## Operational constraints

- **The graph-side merge does not fold yet.** The emitted `merge` node is handed
  each arm's output through its tool arguments, but `run_loop_step` passes a
  step only the decoded state, so `Converge` cannot reach them. The fold itself
  is written and tested — `ArmSet::merge`, which `drive` calls — so a driven loop
  folds correctly and an engine-run loop does not. Widening the step interface
  is the loop kernel's decision; `Converge` is registered rather than omitted so
  the gap is documented in one place instead of surfacing as an `UnknownStep`
  nobody can act on. See `ROADMAP.md`.
- A run stopped by a bound is never `Outcome::Success`, whatever its last pass
  claimed. The classification is adjusted after the bound is known, which keeps
  that rule in one place.
- `Advance` sets `passes` by assignment, never by increment: the fold is
  at-least-once, so a replayed activation after a resume applies the update
  twice.
- A threshold change changes the emitted topology and therefore the graph
  signature, so an old checkpoint refuses to resume onto a retuned loop.

See [`docs/specs/routing-and-policy.md`](../../../../docs/specs/routing-and-policy.md)
and [`docs/plans/observability-and-budget.md`](../../../../docs/plans/observability-and-budget.md).
