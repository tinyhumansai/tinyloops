# Adaptation

- **Status:** Draft — 2026-08-29
- **Owner:** Maintainers
- **Related:** [`loop-kernel.md`](loop-kernel.md),
  [`routing-and-policy.md`](routing-and-policy.md),
  [`orchestrator.md`](orchestrator.md),
  [ADR 0003](../adr/0003-three-layer-split-with-tinyflows-adaptive.md)

## Problem

Every tunable in a loop is fixed before the run starts and cannot move
afterwards. `Thresholds` is a constructor argument, the `ArmSet` is a
constructor argument, `Caps` is a constructor argument, and the re-plan cadence
is a constant. A run therefore cannot act on anything it learns about *itself*.

The run can already learn about the *work*: a reflection appends to
`LoopState::lessons`, a judge writes `LoopState::steer`, and the next attempt
reads both. What it cannot do is act on the second kind of observation, the one
about the loop rather than about the goal:

- `stuck = 2` fires on a domain where the fourth revision is the one that lands,
  so the run diversifies away from a line of attack that was working. The run
  can see this — a diversify followed by a pass more unproductive than the retry
  before it — and can write it in a lesson nothing reads.
- An arm returns a zero delta and an empty contribution on every pass. It costs
  its share of the pass for the length of the run, and no code path can retire
  it.
- A specialist times out at every brief. Salvage turns each timeout into a
  usable attempt, exactly as [`orchestrator.md`](orchestrator.md) requires, and
  the run keeps commissioning it because the delegate set is closed at
  construction.

A second problem compounds the first, and it is structural rather than a matter
of taste. [`loop-kernel.md`](loop-kernel.md) invariant 7 renders every threshold
into the graph's routing ladder as a literal, and invariant 9 hashes the emitted
topology into a signature a resume must match. Those two together mean **a
threshold change is a topology change**, so a run that retuned itself could not
resume from its own checkpoint: the signature it recorded describes a graph that
no longer exists. Adaptation is not merely unimplemented here; the current
addressing scheme forbids it.

This specification defines what a run may change about itself, who may propose
it, what bounds it, and how the change reaches the next pass without breaking
either invariant.

## Goals

- Move every tunable out of graph literals and into a typed, versioned
  `LoopProfile` carried in the accumulator, so tuning a run does not change its
  topology and a tuned run resumes.
- Define `Amendment` — a closed, bounded, budgeted set of changes — as the only
  way a profile moves.
- Define the role that proposes one, and make "any other role proposed an
  amendment" a compile failure rather than a review finding.
- Keep the routing ladder a pure function and keep the exhaustive parity sweep,
  over a threshold space that is finite because its bounds are declared.
- Emit a run's configuration history as data a cross-run layer can score,
  without this crate reading a catalogue, a ledger row, or a score.

## Non-goals

- Cross-run learning: scoring an amendment against outcomes, promoting a tuned
  profile to a preset, selecting a profile for a new run. That is
  `vendor/tinyflows/crates/adaptive`, and this specification only defines the
  data such a layer would read. See ADR 0003.
- Changing the graph's *shape* mid-run. Nodes, edges, and ports are fixed at
  build time and stay hashed by `GraphSignature`. Adaptation moves values, never
  topology.
- Prompt or brief authoring. A brief is composed by `attempt` from the board,
  the steer, and the lessons, and none of that is a profile field.
- Adding a specialist or an arm the loop was not built with. See invariant 8.

## Proposed behavior

### `LoopProfile`

One value, carried in `LoopState`, holding everything a run may revise about
itself:

```rust
pub struct LoopProfile {
    /// Bumped by exactly one every time an amendment is folded.
    pub revision: u32,
    /// The counter bounds the routing ladder reads.
    pub thresholds: Thresholds,
    /// The limits the meters are checked against.
    pub caps: Caps,
    /// Arms the `ArmSet` declares that this run is no longer paying for.
    pub muted: BTreeSet<String>,
    /// Passes between re-plans, per `orchestrator.md`'s cadence.
    pub replan_every: u32,
    /// Where the profile started, and every amendment since.
    pub origin: Preset,
    pub history: Vec<Amendment>,
}
```

`Thresholds` and `Caps` are the types that exist today, unchanged. What changes
is where they are *read from*: the ladder's jq addresses
`=nodes.<loop id>.state.profile.thresholds.<field>` instead of a rendered
number, and `route` takes `&state.profile.thresholds` from the same state it is
already handed.

### `Amendment`

```rust
pub struct Amendment {
    /// The role that proposed it. One value is legal; see invariant 2.
    pub proposer: &'static str,
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
    ReplanEvery(u32),
}
```

`Change` is a closed enum, not a JSON patch. A patch can address anything the
accumulator holds, including the counters the ladder routes on, so a tuner able
to emit one is a tuner able to write `solved`.

### `Bounds`

Each preset ships a `Bounds` alongside its starting profile: an inclusive range
per threshold field, a ceiling per cap field, a set of arms that may be muted,
`muting_window`, and `max_amendments` — the number of amendments a whole run
may fold. An
amendment outside its bound is **refused**, not clamped, and the refusal is an
event.

Refusing rather than clamping is the difference between a tuner that is wrong
and a tuner that is wrong *and looks effective*: a clamped proposal reads as
accepted at the proposer and as a no-op in the state, and nothing joins the two.

### The tuner

Proposing is an evaluation arm like any other — it reads the attempt report and
the base state, it returns at most one amendment per pass, and it writes nothing
else. The shipped default is a **rule-based** tuner, a pure function of the
counters and the arm ledger:

- A `Diversify` followed by a pass strictly more unproductive than the retry
  that preceded it proposes `Threshold(Stuck, stuck + 1)`, once.
- An arm whose contribution has been an empty delta and an empty narrative for
  `muting_window` consecutive passes proposes `MuteArm(name)`.
- Consecutive `blocked` passes at the bound with the same provider named
  propose `Cap(...)` reductions rather than more attempts.

It is rule-based by default because a model asked mid-run whether the loop's
own configuration is wrong has no ground truth to answer from and every
incentive to answer yes: the same pressure that makes a model claim `Solved` on
the eighth pass makes it claim the threshold was the problem. A rule tuner's
whole behavior is a pure function over counters and therefore testable at every
boundary. A model tuner is permitted behind the same trait, and it is bounded by
exactly the same `Bounds`, which is the point of putting the bounds outside the
proposer.

## Invariants and constraints

Nine, each stated with the failure it prevents.

### 1. The profile is state, not topology

Every threshold the ladder reads is addressed out of the accumulator. No
threshold literal appears in graph JSON — which is the half of
[`loop-kernel.md`](loop-kernel.md) invariant 7 this keeps — and `GraphSignature`
hashes the profile's *addressing*, never its values.

*Why.* Under invariant 7 as written, the graph is generated *from* the
thresholds, so a change of one constant is a change of topology, and invariant 9
then refuses to resume a checkpoint taken before it. That is correct today and
fatal to adaptation: a run that retuned itself at pass three could not survive a
crash at pass four. Reading the values out of state means one graph serves every
preset and every revision of every preset, so the signature keeps meaning what
it means — the *shape* is unchanged — while the numbers are free to move.

**Accepting this specification requires amending invariant 7 of
[`loop-kernel.md`](loop-kernel.md)**, from "every number in the ladder is
rendered from the Rust constant" to "no number in the ladder is a literal; both
sides read the same address". The parity requirement is untouched, and invariant
9 is untouched.

### 2. One proposer, and the head is still the only writer

An amendment travels as a `Contribution` slot owned by exactly one arm, folded
by `LoopState::merge` under the exclusive-ownership law. The loop head remains
the accumulator's sole writer. A step context that is not the tuner's has no
slot that reaches the profile, so a second proposer does not compile.

*Why.* This is invariant 1 and the narrative merge law applied to the field
where a silent last-writer-wins would be least detectable. Two arms proposing
different `stuck` values have no correct resolution, and picking one is arrival
order wearing a merge's clothes — the exact failure `Contribution` exists to
refuse.

### 3. Amendments are closed, bounded per field, and budgeted per run

`Change` is a closed enum, each variant's value is checked against `Bounds`, and
a run folds at most `max_amendments` of them.

*Why.* An unbounded tuner has one strategy available for every difficulty, which
is to raise the threshold that is complaining. A run that can raise
`max_attempts` has no attempt ceiling; a run that can raise `stuck` never
diversifies; a run that can raise a cap has no budget. Each of those runs
completes, reports plausibly, and cost more than the run that was configured
correctly. The per-run budget bounds the second-order version, where each
individual amendment is inside its range and forty of them are not.

### 4. An amendment never takes effect in the pass that proposed it

The tuner proposes against the base state; the head folds the amendment at the
top of the next pass, and that pass routes on the new profile.

*Why.* This is [`loop-kernel.md`](loop-kernel.md) invariant 3 restated for the
profile. An arm that could change a threshold and have the same pass's route
read it would make the route depend on whether the tuner finished before the
routing node, which is arm arrival order deciding the run.

### 5. The route stays a pure function, and parity stays exhaustive

`route` reads counters and a `Thresholds` and nothing else. The parity sweep
covers the counter space **crossed with the declared threshold space**, for every
preset, and the second factor is finite only because invariant 3 declares its
bounds.

*Why.* Parity today is finite because presets are finite. Tunable thresholds
make the space of ladders a run can reach unbounded unless something bounds it,
and `Bounds` is that something. Without this pairing, "the jq and the Rust agree"
would degrade from a proof to a sample on exactly the configurations a run
reaches by tuning rather than by construction — the ones no preset was ever
tested at.

### 6. A muted arm still runs its node and still converges

Muting removes an arm's *work*, not its edges: the node runs, returns
`ArmOutcome::unchanged`, and converges into the merge barrier as it always did.

*Why.* [`loop-kernel.md`](loop-kernel.md) invariant 6 derives the fan-out edges
and the convergence edges from one declared list. A mute that dropped a
convergence edge would leave the barrier waiting on an arm nothing will
activate, which is a hung pass rather than a saved one — and it would make the
two edge sets settable independently, which is the drift that invariant
makes unrepresentable.

### 7. An amendment may not add anything the loop was not built with

`UnmuteArm` names an arm the `ArmSet` already declares. There is no change that
adds an arm, a step, a delegate, or a node.

*Why.* The closed step set and the closed delegate set are registration-time
facts that the orchestrator's rule 2 and the kernel's node-body rule both rest
on. A run that can extend either has a closed set only until it decides
otherwise, and the failure mode is a capability nobody chose being reachable
from a rationale nobody reviewed.

### 8. Every amendment and every refusal is an event

`Event::Amended { pass, revision, change, because }` and
`Event::AmendmentRefused { pass, change, reason }`, and the ledger carries the
same rows.

*Why.* A run that quietly retuned itself and then succeeded taught nobody
anything, and it is indistinguishable in its report from a run that succeeded as
configured. The configuration history is the only evidence that separates "the
loop worked" from "the loop was changed until it stopped objecting". The
refusals matter as much as the acceptances: a tuner proposing forty refused
amendments is a broken tuner reporting nothing.

### 9. The final profile is an output, and this crate scores nothing

`Driven` carries the final `LoopProfile` with its `history`. Nothing here reads
a catalogue, a ledger row from another run, or a score.

*Why.* This is where a run's self-observation stops and cross-run learning
starts, and ADR 0003 puts the second in `adaptive`. Keeping the boundary means a
run *emits a proposal about its own configuration* as plain data; whether that
proposal is worth carrying into the next run is a judgement that needs outcomes
this run cannot see. A crate that scored its own amendments would be scoring
them on a single sample, which is the failure that makes a tuner confident.

### Constraints

- `LoopProfile` and `Amendment` are `serde` types with pinned representations,
  per the house rule for anything that crosses a checkpoint.
- `LoopProfile` carries `#[serde(default)]` at the container level, so an
  accumulator written before this existed deserializes into the preset's
  starting profile rather than failing.
- The tuner is optional. A loop built without one is exactly the loop that ships
  today, and no pass costs anything for the absent arm.
- Adding the profile to the accumulator does not change what the ladder reads
  about the *work*: every routing field stays a plain counter.

## Acceptance criteria

- A graph built from two different `Thresholds` values emits the same
  `GraphSignature`, and a checkpoint taken under one resumes under the other.
- No emitted graph JSON contains a threshold literal; a test greps the serialized
  graph for every default threshold value and asserts it appears nowhere.
- The parity sweep runs the counter space crossed with the declared threshold
  space for every preset and reports the first disagreement with the preset name,
  the profile revision, and the offending state.
- An amendment proposed at pass *n* is absent from the route computed at pass *n*
  and present in the route computed at pass *n + 1*; a test asserts both.
- Proposing an amendment from any arm other than the tuner does not compile,
  proved by a `trybuild`-style test.
- Two tuners in one `ArmSet` fail at construction with an error naming both.
- An amendment outside its bound is refused, leaves `revision` unchanged, and
  emits `AmendmentRefused` naming the bound; a test asserts the profile is
  byte-identical before and after.
- A run that has folded `max_amendments` refuses the next one and continues, and
  a test asserts the run neither stops nor routes differently on account of the
  refusal.
- A muted arm's node still runs, still converges, and contributes a zero delta;
  a test asserts the merge waits on the same arm count before and after a mute.
- `UnmuteArm` naming an arm outside the declared `ArmSet` is an error, and a test
  asserts the run does not fall back to registering it.
- A rule-tuner test drives a fixed counter sequence and asserts the exact set of
  passes on which an amendment was proposed, and its content.
- `Driven` exposes the final profile and its full history, and a test asserts the
  history's order is the fold order rather than the proposal order.
- A test asserts the tuner's context type exposes no catalogue, ledger, or score
  handle, so reading `adaptive` state does not compile.
- Deserializing an accumulator serialized before `profile` existed yields the
  preset's starting profile.

## Open questions

- Whether tuning should be reachable under `Autonomy::Report` at all. A run that
  takes no action but revises its own thresholds has changed what a later run
  would do, from a mode whose whole promise is that it decides nothing.
- Whether an accepted amendment should force a re-plan out of cadence. A changed
  `stuck` changes what "this task is going nowhere" means, and the board was
  decomposed under the old meaning.
- Whether `Bounds` belongs to the preset or to the embedder. As the preset's, it
  is part of the stated methodological bet; as the embedder's, a deployment can
  bound a preset more tightly than its author did, and both readings are
  defensible.
- Whether `MuteArm` should be reversible by the tuner at all, or whether an arm
  a run stopped paying for should stay muted for the run. Unmuting gives the
  tuner a two-state oscillation the amendment budget bounds only by exhausting
  it.
- Whether the amendment history should be pinned in the memory seam so a later
  run in the same scope can read it. That is the last decision before this stops
  being a within-run concern and becomes `adaptive`'s.
