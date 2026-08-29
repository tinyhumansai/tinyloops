# 2. Build the loop on the TinyFlows graph engine

- **Status:** Accepted
- **Date:** 2026-08-29

## Context

TinyLoops is the shape of one goal run: attempt, evaluate, route, budget. That
shape has to execute somewhere, and the choice of substrate decides what the
framework can promise. A goal run is long — minutes to hours — spawns concurrent
work, and must survive a process restart without losing the counters it routes
on.

`vendor/tinyflows` is already a vendored submodule of this repository. It
compiles a `WorkflowGraph` (`vendor/tinyflows/src/model/mod.rs:148`) and lowers
it onto a durable state-graph runtime (`vendor/tinyflows/src/graph/mod.rs`). Its
`NodeKind::Loop` head (`vendor/tinyflows/src/nodes/control_flow/loop_node.rs`)
holds an iteration counter and an accumulator in run state, folded by
`config.state.update`, addressable from any expression as
`=.nodes.<id>.state`, and surviving checkpoint and resume. Its `NodeKind::Spawn`
and `NodeKind::Gate` give asynchronous delegation with a release policy, and it
is host-agnostic: every effect crosses a trait in `vendor/tinyflows/src/caps/`
that the embedder implements.

Two alternatives were considered.

**A hand-rolled driver** — a `while` loop in Rust calling the engine, or calling
providers directly. It is the smallest thing that works on day one and it has no
answer for durability: the counters live in locals, so a crash loses the run,
and a resumed run has to be reconstructed by hand. It has no barrier, so
concurrent evaluation is threads and a join, with the fold written per site. And
it has no topology anything can read, so the run's control flow exists only as
control flow.

**The TinyAgents graph** (`vendor/tinyagents`) — a durable agent and graph
harness. It is an optional dependency of `crates/tinyloops` and it resolves an
HTTP client and a harness of its own. A TinyBus module loaded into a host must
not pull those in, and building the kernel on it would make that unavoidable
rather than optional.

## Decision

Build the loop kernel as a builder that emits one `tinyflows::model::WorkflowGraph`,
executed by the TinyFlows engine as a single run.

`crates/tinyloops` depends on `tinyflows` unconditionally. `tinyagents` stays
behind an optional cargo feature and is used only by examples, so nothing under
`src/` is gated on it.

## Consequences

What the graph buys, and what the framework can therefore promise:

- **Checkpoint and resume.** The loop's counters and accumulator are in run
  state, not in a local, so a run survives a restart mid-pass.
- **Fan-out with a real barrier.** Evaluation arms are successors converging on
  a `merge`, so a pass costs the slowest arm rather than the sum of all of them,
  and the convergence is the engine's rather than ours.
- **Per-node concurrency and asynchronous delegation.** `spawn` returns a ticket
  and `gate` collects on a policy; with no `TaskRunner` injected the same graph
  computes the same answer inline, so a host can run the loop before it has a
  scheduler.
- **A topology something can render.** The run's control flow is a value. It can
  be diffed, drawn, validated before it runs, and compared against the Rust that
  generated it.

The costs, accepted:

- The kernel's routing is written twice — as Rust and as the jq the engine runs —
  and the two must be proved equal. See
  [ADR 0004](0004-routing-in-the-graph-steps-in-rust.md) and the parity
  requirement in [`../specs/loop-kernel.md`](../specs/loop-kernel.md).
- The engine's expression layer silently yields `null` on a compile error, a run
  error, non-JSON output, or empty output
  (`vendor/tinyflows/src/expr.rs`). Every graph this crate emits must therefore
  be tested with `assert_no_null_bindings`.
- The graph is frozen at compile: nothing at run time adds, removes, or rewires
  a node. Re-deciding across runs is a different layer — see
  [ADR 0003](0003-three-layer-split-with-tinyflows-adaptive.md).
- This crate is pinned to a vendored submodule. Changes to the engine are made
  in its own repository and land here as a gitlink bump.
