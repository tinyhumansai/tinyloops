# Seams: harness, memory, tools, workspace

- **Status:** Draft
- **Owner:** Maintainers
- **Related:** [`loop-kernel.md`](loop-kernel.md),
  [`workspace-and-ledgers.md`](workspace-and-ledgers.md),
  [`budget.md`](budget.md), [`observability.md`](observability.md)

## Problem

A loop is not a model call. Running one goal to completion needs somewhere to
delegate work, somewhere to keep what was learned, a set of tools the attempt
may use, and a place to write files. Each of those is supplied by the
deployment, not by this crate: one embedder has a durable agent harness and a
vector store, another has a single blocking model client and a temp directory.

If those four are wired in as concrete types, the loop is only testable against
a live deployment, every example needs credentials, and the failure modes below
— an await that never returns, a write that never landed, a tool withheld by
asking the model nicely — are discovered in production rather than in a test.

This specification defines the four seams a loop is built out of, the contracts
each one must keep, and the requirement that every seam ships one offline
reference implementation so an example runs deterministically with no
credentials and no network.

## Goals

- Define `harness`, `memory`, and `tools` as traits with named operations, and
  point at [`workspace-and-ledgers.md`](workspace-and-ledgers.md) for the
  fourth.
- State the contract each seam keeps, and name the observed failure each part
  of that contract prevents.
- Require one in-process reference implementation per seam, sufficient to run
  every bundled example without credentials, network, or wall-clock dependence.
- Fix how a seam's effects reach the engine: through
  `tinyflows::caps::Capabilities`, never around it.

## Non-goals

- Choosing a model vendor, a store, a sandbox, or a transport. Every one of
  those is the embedder's implementation of a trait defined here.
- The loop's own control flow — state, steps, arms, routing — which is
  [`loop-kernel.md`](loop-kernel.md) and
  [`routing-and-policy.md`](routing-and-policy.md).
- Cross-run learning. Anything that spans runs belongs to
  `vendor/tinyflows/crates/adaptive`.

## Proposed behavior

### The harness seam

A `Role` is four things and nothing else: a prompt, a tool grant, a budget, and
a model tier. A `RoleRegistry` resolves a role name to that record, so a call
site names a role rather than assembling a model configuration inline. Budgets
attached here are the per-role caps [`budget.md`](budget.md) requires.

`Delegate` is asynchronous by construction:

```rust
let ticket = delegate.spawn(role, brief)?;   // returns immediately
let status = delegate.peek(&ticket)?;        // separate call
delegate.steer(&ticket, note)?;              // separate call
let outcome = delegate.settle(&ticket).await?;
```

**A blocking delegation is not offered at all.** There is no `run_and_wait`,
and adding one is a contract change, not a convenience. The reason is measured:
in a production run of this design the loop sat 33 minutes unable to start its
next attempt because it was awaiting a single arm — the wait was invisible,
nothing else could proceed, and no observer could say which node was holding.
A seam that offers a blocking call will have one used, so it offers none.

`Mailbox` is bounded post/collect. `post` on a full mailbox drops the note and
reports the drop; it does not block, and it does not grow. Back-pressure that
blocks the solve is the wrong trade: the note is an aside, the solve is the
work. A dropped note is recorded as an event (see
[`observability.md`](observability.md)) so the drop is visible rather than
inferred.

### The memory seam

`recall` and `remember` operate over an explicit `Scope`, and the module offers
compaction over the accumulated history. Four contracts govern it.

**Absence is `None` at wiring time, never an erroring stub.** A deployment
without memory supplies no provider, and the loop reads that as a capability it
does not have. This matches the engine, where `Capabilities::memory` is
`Option<Arc<dyn MemoryProvider>>` (`vendor/tinyflows/src/caps/mod.rs:245`). A
stub that accepts calls and fails them turns a wiring fact into a run-time
error, and the loop cannot tell "no memory here" from "memory is broken".

**A write returning 2xx is not a write.** The trait requires a write-path
verification probe: after a write, a bounded read-back that confirms the record
is retrievable, with the verdict cached per scope so the probe costs one round
trip rather than one per write. This is not hypothetical. A production run
logged 193 successful `remember` calls and stored zero documents: the backend
answered `200 {"status":"running"}` and dropped the work. Every one of those
calls was reported as a success by the only signal available. The probe is what
makes "the store accepted it" and "the store has it" different observations.

**Compaction is recorded, not destructive.** `condense` returns either the view
a pass should use or a recorded condensation event — `forgotten_ids`,
`summary`, `offset` — appended to the history. What a pass stopped showing is
still readable afterwards by anything that walks the history, including a
person debugging the run.

Compaction must be **idempotent**: condensing an already-condensed history
returns the same view and appends no second event. That is a required test, not
an aspiration. The single hardest-to-diagnose harness regression on public
record was a context cleanup meant to run once that ran on every turn; it
produced both forgetfulness and continuous prompt-cache misses, and it took over
a week to locate.

Compaction also honours a **pinned set** it may not touch. Evidence for the
pin: policy-violation rate goes from 0% to 30% after compaction, measured 0%
when the governing constraint survived and 38% when it was dropped, and pinning
the constraint restored 0% for roughly 47 tokens. A constraint that survives
compaction is the cheapest correctness measure in the loop.

### The tools seam

`ToolSet` is the registry facade the loop hands to an attempt. Its governing
rule: **a withheld tool is withheld by not registering it, never by asking the
model to abstain.** A prompt instruction is not a control. Consequently gating
is a constructor decision — the constructor takes the grant and returns a
struct of optional groups — and never a runtime `if` inside a handler that a
sufficiently determined call path can reach anyway.

Four further requirements:

- **`Resilient`.** A tool error becomes a model-readable result rather than an
  end to the run. The harness already names the shape:
  `ToolErrorPolicy::ReturnToError` returns the error to the model instead of
  failing the turn (`vendor/tinyagents/src/harness/tool/error_policy.rs:33`).
- **The decorator is applied at construction, not at registration.** The same
  tool instances are also handed to the workflow capability path, where there
  is no middleware stack to run them through: a `tool_call` node reaches
  `Capabilities::tools` (`vendor/tinyflows/src/caps/mod.rs:214`) directly. A
  decorator applied when a tool is registered with the harness is simply absent
  on that path, so the two callers disagree about what the tool does.
- **Failure sorts into a typed `Recovery`.** `Requery` feeds the error back as
  a message against a bounded retry count. `Salvage` reconstructs what it can —
  the canonical case is reconstructing a diff from the trajectory when the
  sandbox is dead, so a dead environment still yields a result instead of
  nothing. `Fatal` is the only variant that ends anything. Errors travel as
  messages in the history, never as out-of-band state, so the next model call
  can see what failed and the recorded history explains the retry.
- **The model-facing schema set and the introspection set are different.** The
  registry already splits them: `schemas()` projects injected arguments out of
  what the model sees, and `declared_schemas()` is the introspection view,
  documented as "never put this on the wire"
  (`vendor/tinyagents/src/harness/tool/mod.rs:445-468`). `ToolSet` preserves
  the split rather than flattening it into one list.

Interface granularity matters more than tool count. Every surveyed agent
converges on the same four verbs — read, search, edit, execute — whether it
exposes 1 tool or 37. A `ToolSet` is therefore reviewed on whether those verbs
are cleanly separable, not on how many entries it holds.

### The workspace seam

The fourth seam — the `Layout` allowlist, the path check, bounded command
output, the `state()` snapshot, and `Checkpoint` — is specified in
[`workspace-and-ledgers.md`](workspace-and-ledgers.md). It is named here so the
set of four is complete: a loop is a harness, a memory, a tool set, and a
workspace.

### How the seams meet the engine

Every effect a loop causes goes through `tinyflows::caps::Capabilities`
(`vendor/tinyflows/src/caps/mod.rs:210`) — the LLM call, the tool call, HTTP,
code execution, and state, plus the optional `AgentRunner`, `ShellRunner`,
`MemoryProvider`, `TaskRunner`, and `ApprovalProvider`. A seam implementation
does not open its own client and does not reach past the bundle.

That is deliberate, and it is the strongest argument for this framework owning
its loop rather than wrapping somebody else's. The most common durability bug in
this field is "forgot the wrapper": code that works correctly, passes its tests,
and is silently non-durable because one call was made outside the mechanism that
would have recorded it. Making the effect boundary a trait the call must pass
through makes that bug unrepresentable — there is no second way to reach a
model, so there is no call that can miss the recording.

### Reference implementations

Each seam ships exactly one in-process reference implementation: a role registry
and delegate backed by scripted outcomes, a memory backed by an in-process map
whose probe genuinely reads back, a tool set of pure functions, and the
workspace's temp-directory layout. They exist so every bundled example and every
test in this workspace runs with no credentials, no network, and no dependence
on wall-clock time or execution order.

## Invariants and constraints

- No seam trait exposes a blocking delegation. `spawn`, `peek`, `steer`, and
  `settle` are separate operations.
- A full mailbox drops and reports; it never blocks a caller.
- A capability the deployment lacks is `None` at wiring time. No seam ships a
  stub that accepts a call in order to fail it.
- A write is not acknowledged as durable until a read-back verified it. The
  verdict is cached per scope and bounded in time.
- `condense` applied twice yields the same view and appends one event, not two.
- Pinned entries survive every compaction.
- Tool grants are resolved in a constructor. No handler decides whether it is
  allowed to run, and no prompt is asked to enforce a grant.
- The tool decorator wraps instances before they are shared, so the harness path
  and the `Capabilities::tools` path observe identical behavior.
- Tool failures appear in the history as messages. A `Fatal` recovery is the
  only variant that terminates a step.
- `declared_schemas()`-shaped output never reaches a model request.
- Every effect crosses `Capabilities`. A seam implementation constructing its
  own transport is a defect.
- Every reference implementation is deterministic: same inputs, same events,
  same outcome, independent of test ordering.

## Acceptance criteria

- The public surface of each seam compiles with no async-blocking operation
  present; a test asserts `Delegate` has no method that both starts and settles
  work.
- Posting to a mailbox at capacity returns a drop outcome, emits a drop event,
  and leaves the loop able to take its next step in the same test.
- Wiring a deployment without memory yields `None`, and the loop's behavior
  under `None` is covered by a test distinct from the store-error path.
- A store double that answers every write with success while retaining nothing
  causes the verification probe to fail the write. This test reproduces the
  193-writes-zero-documents failure and is the regression test for it.
- `condense(condense(history)) == condense(history)`, asserted on both the
  returned view and the count of appended condensation events.
- A history whose pinned set holds a policy constraint still contains that
  constraint after compaction reduces the history below the pin's own size.
- Constructing a `ToolSet` without a group leaves the group absent from
  `schemas()`; no test achieves absence by prompt text.
- A tool reached through the workflow capability path and the same tool reached
  through the harness both exhibit the decorator's behavior in one test.
- A dead-sandbox fixture produces a `Salvage` recovery carrying a reconstructed
  diff, and the run reports a result rather than a failure.
- Every bundled example runs to completion in CI with no credentials in the
  environment and no network access.

## Open questions

- Whether `steer` is delivered as a mailbox note to a running delegation or as a
  distinct control channel. The former reuses one mechanism; the latter keeps
  steering from being dropped under back-pressure, which is exactly the case
  where steering matters most.
- What the memory probe's cache lifetime should be. Too short and it is one
  extra round trip per write; too long and a store that starts dropping writes
  mid-run is not noticed until the run ends.
