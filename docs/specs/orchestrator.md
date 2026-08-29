# Orchestrator

- **Status:** Accepted — 2026-08-29
- **Owner:** Maintainers
- **Related:** [`loop-kernel.md`](loop-kernel.md),
  [`routing-and-policy.md`](routing-and-policy.md),
  [ADR 0005](../adr/0005-an-orchestrator-role-that-holds-no-execution-tools.md)

## Problem

The loop kernel says where a run goes; something still has to decide *what to
attempt*. Left to a single general-purpose agent with a full toolbox, that
decision collapses in a predictable way: a driver that can run the experiment
runs it instead of commissioning it, spends the pass doing one specialist's work
badly, and produces an attempt report about the tool call it made rather than
about the goal. The run then routes on that report, and the loop's whole routing
apparatus is deciding about the wrong thing.

The failure is not a prompting failure. Telling an agent "delegate, do not
execute" is an instruction it will follow until the moment execution is the
locally cheaper path. **A prompt instruction is not a control.** What is needed
is a role whose *registration* makes the wrong action unavailable.

## Goals

- Define an orchestrator role bound to exactly three nodes of the kernel graph:
  `plan`, `attempt`, and `report`.
- Make the decomposition **derived state** that code can read and route on,
  rather than prose inside a prompt.
- Make delegation asynchronous by construction, and make a failed delegation a
  result rather than an end.
- Express every constraint as a registration-time fact about the role, provable
  from the registry and the emitted graph.
- State how this composes with `tinyflows-adaptive` without either reading the
  other's state.

## Non-goals

- Defining the specialists. They are host-supplied agents behind
  `tinyflows::caps::AgentRunner` (`vendor/tinyflows/src/caps/agent/runner.rs:56`)
  and the workflow's own `agents` registry
  (`vendor/tinyflows/src/model/mod.rs:168`).
- The evaluation arms and how their answers fold. That is
  [`loop-kernel.md`](loop-kernel.md).
- Cross-run workflow selection, authoring, scoring, or promotion. That is
  `vendor/tinyflows/crates/adaptive`.

## Proposed behavior

### The three bindings

The orchestrator is one role, bound to three nodes. It is the same role at all
three, holding the same registration and the same closed delegate set; the nodes
differ in what they are handed and what they must produce.

#### `plan`

Turns the goal into named tasks, each with an explicit completion criterion, on
a **`TaskBoard`**. The board is a typed, serializable structure that lives in the
run accumulator: a list of tasks, each with an id, a statement, a completion
criterion, a status, and the pass that last touched it. Code reads it, the
routing ladder can read counts off it, and the report renders from it.

It is a board rather than a paragraph in a prompt because a plan expressed as
prose cannot be checked, cannot be counted, and cannot be routed on. "Three of
five tasks discharged" is a fact only if the tasks are values.

`plan` runs on a **cadence**, not every pass. It runs once before the loop, and
then only after an interval of passes has elapsed. A decomposition is worth
rewriting only once the run has established something that could discharge one
of its parts; re-planning every pass spends a full role's work rewording a
decomposition nothing has yet tested, and — worse — makes the board unstable, so
"task 3 is still open" stops meaning anything across passes.

#### `attempt`

Chooses which specialists to spawn this pass and with what brief. Its inputs are:

- the previous `Route`, so a `Diversify` opens different specialists than a
  `Retry` continues with;
- the judge's `Steer` correction, when the last pass produced one;
- the lessons the run has accumulated;
- the `TaskBoard`, so a brief names which task it is meant to discharge;
- any **operator directive** drained from a bounded mailbox.

Delegation is **asynchronous by construction**: spawning returns a ticket
immediately, and the pass continues. The engine has this shape natively —
`NodeKind::Spawn` emits a ticket per started task and `NodeKind::Gate` collects
on a release policy (`all` / `any` / `first_n` / `quorum` / `timeout_partial`),
backed by the `TaskRunner` capability (`vendor/tinyflows/src/caps/tasks.rs:81`).
With no `TaskRunner` injected the work runs inline and the ticket comes back
already settled, so the graph computes the same answer without the concurrency —
which is what makes a host able to run the loop before it has a scheduler.

Having collected, `attempt` writes **one attempt report**. That single artifact
is what every evaluation arm reads. One report is what makes the arms
independent of each other and therefore concurrent: each reads the same input,
none reads another's output, so a pass costs the slowest arm rather than the sum
of all of them.

#### `report`

Composes the final answer, after `stand_down` has retired anything still
running. It reads the board, the accumulated lessons, the artifacts, and the
terminal state, and it says which tasks were discharged, which were not, and on
what evidence. It renders the terminal state honestly: a run that stopped on
`Exhausted` or `Blocked` says so.

`report` runs after `stand_down` and not before, because work still in flight
when the loop ends is work on a question already answered. Everything it
produces after the verdict costs budget and can change nothing.

### The five rules

Each is a **registration-time decision** — a property of how the role is
constructed — not a line in a prompt.

**1. It holds no execution tools.** No shell, no code runner, no program-write
capability. Its tool set is delegation, board reads and writes, mailbox drains,
and artifact reads.

*Why.* A driver that can run the experiment will run it instead of commissioning
it, and the pass then reports on a tool call rather than on the goal. Removing
the capability removes the option; keeping it and asking the model not to use it
removes nothing.

**2. Its delegates are a declared closed set.** The role names the specialists it
may spawn. It is not handed the host's whole agent registry, and a specialist not
on its list is not reachable — a spawn naming one is an error, not a fallback.

*Why.* A specialist reachable by accident is one nobody chose. A registry grows;
a role's delegate list is a decision, and it should fail loudly when the two
diverge rather than quietly acquiring capabilities.

**3. It is the sole author of the report.** No specialist writes the final
answer, and no arm appends to it.

*Why.* A report assembled from fragments each written by a role that saw part of
the run is a report nobody has checked for consistency. One author means one
account, and it means the claim in the report is attributable when the
reflection contradicts it.

**4. A delegation that fails is a result, not an end.** A specialist that times
out, errors, or is killed at its cap returns a **readable outcome** describing
what it was asked, how it ended, and what it left behind. The salvage path
reconstructs an attempt from the artifacts a killed specialist wrote.

*Why.* The ordinary way a long delegation ends is its own cap killing it, which
destroys its reply and leaves every file it wrote. Without salvage the pass
reports nothing, the reflection has nothing to evaluate, `unproductive`
increments on a pass that in fact produced work, and the routing ladder spends a
diversify on a run that was not stuck. Salvage turns a run killed at its cap
into a usable attempt rather than a blank one.

**5. It never blocks on an arm.** Work that must not gate the loop posts into a
**bounded mailbox**, drained at the next `attempt`. A full mailbox **drops the
note** rather than stalling the loop, and records the drop.

*Why.* An unbounded queue turns a slow consumer into unbounded memory; a
blocking send turns a slow consumer into a stalled loop. Neither is acceptable
for work that was, by definition, optional. Dropping is the only failure mode of
the three that leaves the loop running, and recording the drop is what keeps it
from being invisible.

### Relation to `tinyflows-adaptive`

Both layers choose. They choose different things, at different timescales, and
neither reads the other's state.

- `adaptive::intake::decide`
  (`vendor/tinyflows/crates/adaptive/src/intake/mod.rs:111`) chooses **which
  graph to run**, across episodes. It sees the workflow catalogue with its
  score counters, the ledger rows this episode has already produced, and the
  lessons earlier episodes left; it selects a stored workflow or authors one.
- The orchestrator chooses **which specialists to spawn**, within one pass of
  one run of one graph. It sees this run's board, route, steer, lessons, and
  mailbox.

The boundary is the same one `adaptive` states from its own side: the engine may
know about one run, and anything that spans runs lives there. The orchestrator
therefore has no access to the ledger and no way to consult scores; `adaptive`
has no access to the board and no way to influence a pass. They compose by
nesting — an episode runs a graph, and that graph runs the loop — not by sharing
a state object.

## Invariants and constraints

- The orchestrator's tool set and delegate set are fixed at construction and
  cannot be extended at run time.
- The `TaskBoard` lives in the loop accumulator and is therefore written only by
  the loop head, per [`loop-kernel.md`](loop-kernel.md) invariant 1. `plan` and
  `attempt` return a whole state containing the updated board; neither writes the
  slot.
- Board task ids are stable across passes. A re-plan may add, close, or restate
  a task, but reusing an id for a different task breaks every count that reads
  the board across passes.
- Exactly one attempt report is produced per pass, at a known address every arm
  reads.
- The mailbox has a declared capacity, and `post` is non-blocking and infallible
  from the caller's perspective — a full mailbox returns "dropped", never an
  error the caller must handle by waiting.
- `report` has no outgoing edge to any node that can start work.
- Every constraint above is checkable against the emitted graph and the role's
  registration without running the loop.

## Acceptance criteria

- Constructing the orchestrator with a shell, code-runner, or file-write tool in
  its set fails at construction with a named error; a test asserts the message
  names the offending tool.
- A spawn naming a delegate outside the declared set returns an error, and a test
  asserts the run does not fall back to the host registry.
- `plan` runs at pass 0 and then only on its cadence; a test over N passes
  asserts the exact set of passes on which it ran.
- A test asserts the board survives a checkpoint and resume with every task id
  and status intact.
- With no `TaskRunner` injected, a pass spawning three specialists produces the
  same attempt report as the same pass with a `TaskRunner` that runs them
  concurrently.
- A specialist mocked to time out yields a readable outcome naming the brief and
  the timeout, the pass still writes an attempt report, and the run does not mark
  the pass unproductive on the strength of the timeout alone.
- A specialist mocked to be killed after writing an artifact yields a salvaged
  attempt that cites the artifact.
- Posting to a mailbox at capacity returns immediately, drops the note, and
  records the drop in the run's events; a test asserts the loop's pass duration
  is unchanged.
- `report` is reachable only after `stand_down` in the emitted graph, asserted on
  the graph's edges.
- A test asserts no arm or specialist writes the report address.
- A test asserts the orchestrator's context type exposes no ledger, catalogue, or
  score handle, so an attempt to read `adaptive` state does not compile.

## Open questions

- Whether the re-plan cadence should be a pass count or a signal — "a task was
  discharged since the last plan" is a better trigger than "four passes have
  gone by", but it requires the board's completion criteria to be mechanically
  checkable, which not every domain can supply.
- Whether an operator directive should be able to force a re-plan out of cadence,
  and if so whether that is a directive kind or a separate control surface.
- Whether the mailbox's drop policy should be oldest-first rather than
  newest-first. Dropping the newest loses the freshest observation; dropping the
  oldest loses the one that has been waiting longest to be acted on.
- Whether a specialist may itself hold a bounded delegate set — recursion here is
  useful and is also how a closed set stops being closed.
