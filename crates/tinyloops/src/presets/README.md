# `presets`

Assembled loops, ready to build or to drive, and the one shipped tuner.

## What is here

- [`research_loop`] — an orchestrator, two evaluation arms, and a preset. The
  loop most callers should meet first.
- [`tuned_research_loop`] — the same loop with a third arm that may revise the
  run's own configuration.
- `Preset` — the four shipped threshold sets, each stating the bet it makes and
  the room it gives a run to revise that bet.
- `Rules` — the shipped tuner, a pure function of the counters.
- The node bodies the kernel graph reaches that are not the orchestrator's:
  `Gather`, `ArmStep`, `Converge`, `Advance`.

## The two loops are different loops

`tuned_research_loop` is a separate function rather than a flag, because it
emits a third arm's node, a third pair of edges, and a different graph
signature. A run that can revise itself and a run that cannot should not be
told apart by an argument nobody reads in a diff.

A loop assembled without a tuner costs nothing for the absent arm and proposes
nothing, and its report has no section about revisions.

## What a run may revise, and what it may not

The tuner proposes; `Preset::bounds` decides what may be proposed; the `pass`
step folds. Three constraints are worth stating operationally, because each is
the answer to a failure that is otherwise silent.

**The bounds live outside the proposer.** Swapping the rule tuner for a model
one cannot widen what a run may do to itself. That is the whole reason
`Bounds` is a value on the preset rather than a property of the tuner.

**A deployment may narrow, never widen.** `Bounds::narrow` clamps field by
field — the same operation `RunBudget::narrow` performs on caps — so a host
that distrusts a preset can tighten it and cannot loosen one, whatever it
passes. A field neither side mentions cannot be moved at all: silence is not
permission.

**An amendment is refused, never clamped.** A clamped proposal reads as
accepted at the proposer and as a no-op in the state, and nothing joins the
two. Every refusal is recorded in `LoopProfile::history` and emitted as
`Event::AmendmentRefused`, so a tuner proposing forty impossible changes is
visible rather than merely ineffective.

## The shipped tuner's rules

Ordered, and the order is the policy: infrastructure first, because a run the
machinery is failing has learned nothing about its own patience; then patience;
then what the run is paying for and not reading. At most one proposal a pass.

| Rule | Fires when | Proposes |
|---|---|---|
| blocked | `blocked` reaches one below its threshold | half the model-call allowance |
| patience | `unproductive` is strictly past `stuck` — a diversify already happened and the pass after it was unproductive too | `stuck + 1`, once |
| silence | the judge has returned the same score for `SILENT_SCORES` passes | mute the judge |

It is rule-based rather than model-based because a model asked mid-run whether
its own configuration is wrong has no ground truth to answer from and every
incentive to answer yes. A model tuner implements the same `Tuner` trait and is
bounded by the same `Bounds`.

The muting rule fires on **silence**, never on "scored worse". Eliminating the
weakest arm needs a measured reward per arm, and this loop has none — an arm
contributes a delta and a narrative, not a score of its own.

## Operational constraints

- **A muted arm still runs its node and still converges**, returning unchanged.
  Dropping its convergence edge would leave the merge barrier waiting on an arm
  nothing will activate — a hung pass rather than a saved one.
- **An amendment never takes effect in the pass that proposed it.** The fold is
  in `Advance`, the `pass` step, which is the loop's single exit and the only
  node closing the cycle. That position is what makes the timing structural
  rather than a rule someone remembers.
- **No preset may amend its attempt ceiling past the loop head's backstop.**
  Above `Caps::max_iterations` the amendment folds, reads back as raised, and
  buys nothing. `src/policy/test.rs` asserts the relationship.
- **The final profile is an output and nothing here scores it.** `Driven`
  carries it with its full history. Scoring an amendment against outcomes spans
  runs, and that is `tinyflows-adaptive`'s — see ADR 0003.

[`research_loop`]: https://docs.rs/tinyloops
[`tuned_research_loop`]: https://docs.rs/tinyloops
