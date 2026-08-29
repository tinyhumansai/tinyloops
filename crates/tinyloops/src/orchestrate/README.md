# `orchestrate/`

The role that drives a goal run: what to attempt, who to commission it from,
and what the run concluded.

## Why the module exists

The loop kernel decides where a run *goes*. Something still has to decide what
to *attempt*. Handed to one general-purpose agent with a full toolbox, that
decision collapses in a way that is easy to predict and hard to see afterwards:
the driver runs the experiment instead of commissioning it, spends the pass
doing one specialist's job badly, and writes an attempt report about the tool
call it made rather than about the goal. Every routing decision after that is
correct reasoning about the wrong subject.

That is not a prompting failure and it has no prompting fix. **A prompt
instruction is not a control.** "Delegate, do not execute" holds right up until
executing is the locally cheaper path, which is exactly the moment it matters.

So every constraint here is a property of a constructor.

## The public surface

| Item | What it is |
| --- | --- |
| `Orchestrator` | The registration: a `ToolGrant` that refuses editing and executing groups, and a closed `DelegateSet` with no extend method |
| `DelegateSet` | The specialists the role may reach, fixed at construction |
| `TaskBoard`, `Task`, `TaskId`, `TaskStatus` | The decomposition, as values |
| `Plan`, `Attempt`, `Report` | The three steps, bound to the three nodes |
| `Decompose`, `Specialists`, `Compose` | The seams each step delegates its judgement to |
| `FixedPlan`, `Inline`, `Summarize` | Reference implementations that run offline |
| `AttemptReport` | The one artifact per pass that every evaluation arm reads |

## Design

### The board is values, not prose

A plan written into a prompt cannot be counted, cannot be checked, and cannot be
routed on. "Three of five tasks discharged" is a fact only if the tasks are
values, so the decomposition is a `TaskBoard` carried in `LoopState`.

That placement has a consequence worth stating: the board is written by the loop
head and nothing else, per invariant 1 of the loop kernel. `Plan` and `Attempt`
return a whole state containing an updated board; neither writes the accumulator
slot. It is therefore checkpointed and resumed with every counter, which is what
makes a task id stable across a crash rather than only across a pass.

`TaskBoard::add` refuses an id it already holds. Restating a task keeps its id;
a new task gets a new one. Without that rule "task 3 is still open" means a
different thing on either side of a re-plan and nothing reports the change.

### `plan` runs on a cadence

Pass 0 always plans, and after that only every `Thresholds::plan_interval`
passes. Re-planning every pass spends a full role's work rewording a
decomposition nothing has yet tested, and it makes the board unstable, which
destroys every count that spans passes.

### `attempt` never discharges a task

A briefed task moves to `TaskStatus::InFlight` and no further. The criterion that
would discharge it is checked against the workspace, not against a reply, and
`LoopState::established` is likewise counted off artifacts rather than off
anybody's account of their own work.

### Blocked is not unproductive

A pass whose specialists all failed to start is `blocked`: the machinery would
not run, so the run learned nothing about the goal. A pass that ran and came
back empty is `unproductive`. The two rungs of the routing ladder sit at
different distances from the exit, and conflating them either exits a run that
was merely stuck or grinds one whose sandbox is dead.

"Only outcome" is literal. One specialist that could not start alongside one
that ran and found nothing is unproductive, not blocked.

### A failed delegation is a result

A specialist that timed out, was capped, or failed returns a readable
`DelegationOutcome` naming what it was asked, how it ended, and what it left
behind. The ordinary way a long delegation ends is its own cap killing it, which
destroys the reply and leaves every file it wrote; `salvage` turns that into a
usable attempt. Without it the pass reports nothing, the reflection has nothing
to evaluate, `unproductive` increments on a pass that in fact produced work, and
the ladder spends a diversify on a run that was not stuck.

### Dispatch is one call

`Specialists::dispatch` takes briefs and returns outcomes rather than offering a
spawn and a separate collect. In the emitted graph that pair is a
`NodeKind::Spawn` and a `NodeKind::Gate`, and the engine's `TaskRunner` owns the
overlap; with no runner injected the engine runs the same work inline and the
tickets come back already settled. The pass must compute the same answer either
way, which is what lets a host run the loop before it has a scheduler.

## Operational constraints

- Both of the orchestrator's sets are fixed at construction. There is no extend
  method, because a capability acquired after construction is one nobody
  reviewed.
- `Orchestrator::spawn` checks the declared set *before* reaching the
  `Delegate`, so an undeclared name never touches the harness. There is no
  fallback to the host registry anywhere on the path.
- `Orchestrator::verify_declared_in` fails at wiring time when a declared
  delegate is missing from the role registry, rather than on the pass that first
  needs it.
- The mailbox is bounded and a post at capacity drops the note and records the
  drop. Dropping is the only one of the three failure modes that leaves the loop
  running: an unbounded queue turns a slow consumer into unbounded memory, and a
  blocking send turns one into a stalled loop.
- Nothing here holds a ledger, a catalogue, or a score. Cross-run state is
  `vendor/tinyflows/crates/adaptive`, and the two layers compose by nesting
  rather than by sharing a state object.

See [`docs/specs/orchestrator.md`](../../../../docs/specs/orchestrator.md).
