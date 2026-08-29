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
| `Gather`, `ArmStep`, `Advance`, `Converge` | The node bodies for the kernel nodes that are not the orchestrator's. `Converge` is the fold |

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
implementations of anything: `drive` invokes the registered steps with the
arguments the emitted nodes are addressed with, so both paths execute the same
bodies over the same values. The routing likewise resolves to `route` on both
sides, and the graph's ladder is generated from the same constants and proved
against that function. What differs is what owns the concurrency and the
durability. `tests/e2e.rs` asserts the two reach the same verdict.

`drive` exists because the engine's mock capabilities are a dev-only dependency
here, so the shipped library cannot start a graph run, and because a loop you
can call from a test with no runtime, no scheduler, and no provider is the loop
most people should meet first.

## Operational constraints

- **The merge folds in the graph.** The emitted `merge` node is addressed with
  `state` (the attempt's output, the shared base) and `arms` (each arm's whole
  returned accumulator, keyed by name), and `StepContext::arg` is how a body
  reaches them. `Converge` calls `ArmSet::merge` — the same function `drive`
  calls — so there is one fold, not two.
- An arm's narrative claim rides through the graph *as state*: `ArmStep` applies
  the `Contribution` to the accumulator it returns and `Converge` reads it back
  with `Contribution::claimed_from`. The two are inverses and are tested as
  such. If they stop being inverses, a lesson or a steer silently stops reaching
  the accumulator.
- An arm's output that is missing or `null` at the merge is an error, never a
  smaller fold. Under this engine an expression that failed to resolve yields
  `null`, so shrugging at it would turn a broken binding into a route taken on
  evidence nobody gathered.
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
