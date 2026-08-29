# 4. Routing declared in the graph, steps written in Rust

- **Status:** Accepted
- **Date:** 2026-08-29

## Context

Having chosen the TinyFlows graph as the substrate
([ADR 0002](0002-loop-on-the-tinyflows-graph.md)), a line has to be drawn: how
much of the loop is declared in the graph, and how much is Rust behind a node?

Two extremes were considered, and both fail.

**Everything in the graph.** Node bodies become `NodeKind::Agent` with an
`agent_ref`, and the whole loop is a JSON document. It reads well and it loses
three things that are not optional: the operator-directive drain, so a running
loop accepts direction and acts on none of it; the salvage of an attempt killed
at its own cap, so a pass that produced artifacts reports nothing; and the arms
opened *beside* the loop, started at a named node where a checkpoint can land
rather than as a side effect inside whichever step held the handles. Expressing
those in JSON means reimplementing them in jq, where a typo is not a compile
error but a condition that is silently never true
(`vendor/tinyflows/src/expr.rs` — a compile error, a run error, non-JSON output,
and empty output all yield `Value::Null`).

**Everything in Rust.** The graph becomes a single node and the loop is a
`match` in a function. The routing is then invisible: it cannot be diffed, drawn,
validated before it runs, or edited by anything but a Rust programmer, and the
run has no topology a reader can check against the specification.

## Decision

**The graph owns routing; Rust owns the steps.**

- Every branch a run can take — the ladder, the fan-out to the evaluation arms,
  the barrier, the single exit through `pass`, the back-edge to the head, the
  terminal exit through `stand_down` — is declared in the emitted
  `WorkflowGraph`.
- Every node body is one registered tool, `run_loop_step`, invoked through
  `NodeKind::ToolCall` and the host's `ToolInvoker`
  (`vendor/tinyflows/src/caps/mod.rs:137`), over a **closed** step set. An
  unknown step name is an error, never a no-op.
- Every threshold in the rendered jq is generated from the Rust `Thresholds`
  constant. No threshold literal is typed into graph JSON.
- A parity harness replays the generated jq and the Rust routing function over
  the entire counter space, exhaustively, for every shipped preset.

## Consequences

- The routing is a value. It can be rendered, validated by
  `tinyflows::validate` before a run starts, compared across versions in a diff,
  and hashed into the checkpoint signature so an incompatible resume is refused.
- The steps stay Rust: typed, unit-tested, and able to hold the drain, the
  salvage, and the beside-the-loop arms that JSON cannot express.
- The decision is written twice, and that is the accepted cost. Two engines
  deciding the same run differently is invisible in a live run and obvious only
  in a diff — a ladder reading `>` where the Rust reads `>=` changes when a run
  diversifies and fails nothing. Generating the numbers removes the class where
  the two disagree about a constant; the exhaustive sweep removes the class where
  they disagree about a comparison.
- Parity proves the *translation*, never the answer. Both sides read the same
  number, so a wrong threshold is wrong in both and agrees with itself. The
  rationale attached to each constant in
  [`../specs/routing-and-policy.md`](../specs/routing-and-policy.md) is what
  covers that.
- A step set that is closed means adding a node to the graph without adding its
  step fails loudly. A graph naming a step that does not exist would otherwise
  run green, change nothing, and route on a state nobody advanced.
- Editing the loop's control flow no longer requires editing Rust, which is what
  makes the routing something an outside process — or a later `adaptive` repair —
  could propose a change to. Editing what a step *does* still does.
