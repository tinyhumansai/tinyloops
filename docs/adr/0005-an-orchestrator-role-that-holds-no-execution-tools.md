# 5. An orchestrator role that holds no execution tools

- **Status:** Accepted
- **Date:** 2026-08-29

## Context

Something has to decide what a pass attempts. The obvious construction is one
capable agent holding the union of every tool the run might need: a shell, a
code runner, file writes, search, and the ability to delegate.

That construction fails the same way every time. A driver that *can* run the
experiment runs it, because executing is locally cheaper than writing a brief
and waiting. The pass then produces an attempt report about the tool call it
made rather than about the goal, and the loop's routing ladder — which is
carefully ordered, threshold-bounded, and parity-tested — decides about the
wrong thing. The evaluation arms evaluate a shell command. The run completes and
reports something plausible.

The other half of the same failure is the delegate list. An orchestrator handed
the host's whole agent registry can reach any specialist in it, including ones
added later for an unrelated purpose. A specialist reachable by accident is one
nobody chose, and the run's behaviour then changes when an unrelated part of the
host grows.

Both have an obvious cheap fix — write it in the prompt — and the cheap fix is
not a fix. An instruction is followed until the moment it is locally
inconvenient, and nothing reports the moment it stops being followed. **A prompt
instruction is not a control.**

## Decision

Register the orchestrator as a role whose *construction* forbids what the prompt
would only discourage.

- **No execution tools.** No shell, no code runner, no program-write capability.
  Its tool set is delegation, task-board reads and writes, mailbox drains, and
  artifact reads. Constructing it with an execution tool in its set fails with a
  named error.
- **A declared closed delegate set.** The role names the specialists it may
  spawn. A spawn naming one outside the set is an error, not a fallback to the
  host registry.
- **Sole author of the report.** No specialist and no evaluation arm writes the
  final answer.
- **A failed delegation is a result.** A specialist that times out, errors, or is
  killed at its cap returns a readable outcome, and the salvage path reconstructs
  an attempt from what it left behind.
- **It never blocks on an arm.** Optional work posts into a bounded mailbox,
  drained at the next `attempt`; a full mailbox drops the note and records the
  drop rather than stalling the loop.

The full behaviour, its three node bindings, and the acceptance criteria are in
[`../specs/orchestrator.md`](../specs/orchestrator.md).

## Consequences

- The wrong action is unavailable rather than discouraged, and every one of the
  five rules is checkable against the role's registration and the emitted graph
  without running the loop.
- Removing execution capability is what makes the attempt report a report about
  the goal, which is what makes the routing ladder's decision meaningful.
- A closed delegate set fails loudly when the registry and the role diverge,
  instead of quietly acquiring capabilities as the host grows.
- Salvage is what keeps a killed specialist from looking like an unproductive
  pass. Without it, `unproductive` increments on a pass that produced work, and
  the ladder spends a `Diversify` on a run that was not stuck.
- The bounded mailbox drops. That is a deliberate choice among three bad options:
  an unbounded queue turns a slow consumer into unbounded memory, a blocking send
  turns it into a stalled loop, and only dropping leaves the loop running.
  Recording each drop is what keeps it from being invisible.
- The cost is that adding a specialist is a code change to a declared list rather
  than a registry entry, and that a genuinely new capability for the orchestrator
  requires a new ADR rather than a new tool in a set. That friction is the
  intended effect.
