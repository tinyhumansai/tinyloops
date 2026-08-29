# Routing and policy

- **Status:** Draft
- **Owner:** Maintainers
- **Related:** [`loop-kernel.md`](loop-kernel.md),
  [`orchestrator.md`](orchestrator.md),
  [ADR 0004](../adr/0004-routing-in-the-graph-steps-in-rust.md)

## Problem

Once a pass has produced a report and its evaluation arms have folded, something
has to decide where the run goes next: try again, try differently, stop and
report, or stop because the run is blocked. That decision is the highest-leverage
code in a loop framework and the code a live run is least able to demonstrate
cheaply — a wrong ordering strands a run that was working, or spends a whole
budget diversifying away from an answer already on disk, and either way the run
completes and reports something plausible.

It is also the code most exposed to drift, because the same decision has to be
expressed twice: once as Rust the crate tests, and once as jq the engine
actually runs. This specification defines the types, the ladder, the ordering,
the parsing rules, and the parity requirement that keeps the two the same
decision.

## Goals

- Define `Thresholds`, `Route`, `Judgement`, and `Autonomy` as typed values with
  their rationale attached.
- Fix the ladder's order, and state what each rung's position buys.
- Separate the two questions a pass asks — *is the answer right* and *was the
  attempt conducted well* — into two roles with different powers.
- Define how an unreadable verdict resolves, in the direction that cannot throw
  work away.
- Require that the rendered jq and the Rust agree, exhaustively.

## Non-goals

- The graph shape the ladder is rendered into. That is
  [`loop-kernel.md`](loop-kernel.md).
- Which model answers a reflection or a judge, and how it is prompted. Those
  cross a `tinyflows::caps::LlmProvider` or `AgentRunner` the embedder
  implements.
- Cross-run learning — scored lessons, workflow selection, promotion. That is
  `vendor/tinyflows/crates/adaptive`.

## Proposed behavior

### `Thresholds`

A plain, copyable struct of counter bounds. Every field carries a rustdoc
paragraph saying what evidence set it, because **a threshold without a recorded
rationale is the failure worth naming**: a `stuck` of 2 is a defensible
methodological commitment, and an unrecorded 2 is a number nobody can argue
with, revise, or defend in review.

| Field | Bounds |
|---|---|
| `max_attempts` | the ceiling on passes through the loop |
| `blocked` | consecutive infrastructure failures before the run stops |
| `unverified` | times an answer was reached with no second route supporting it |
| `stuck` | consecutive passes reporting no progress before diversifying |
| `computational` | passes whose only gain was a bigger instance of the same computation |
| `max_restarts` | times a judge may discard the direction and start over |

Presets ship as associated constructors. Each preset's docs state **which
methodological bet it is making** — a low `stuck` bets that variation is cheaper
than persistence, a high one bets the opposite — so choosing a preset is
choosing a stated position rather than picking a name.

### `Route`

The decision a pass reaches.

```rust
pub enum Route {
    /// Verified, or out of attempts: stop and answer.
    Solved,
    /// An answer with only one route supporting it. Stop and say so.
    Reported,
    /// Try again, carrying the lesson just learned.
    Retry,
    /// Repeated attempts are not advancing. Open new angles first.
    Diversify,
    /// The provider, not the work, is what stopped the run.
    Blocked,
}
```

### The ladder

Evaluated top to bottom against the merged state. **The order is load-bearing**,
and each rung's position is a decision, not an accident:

```text
Route::Blocked   if blocked        >= t.blocked          // infrastructure failure is not the work
Route::Solved    if solved || attempts >= t.max_attempts
Route::Reported  if unverified     >= t.unverified
Route::Diversify if unproductive   >= t.stuck || computational >= t.computational
Route::Retry     otherwise
```

- **`Blocked` first, ahead of the attempt ceiling.** An attempt that died on the
  provider is not evidence about the work, so spending the ceiling on more of
  them spends the run's one budget on a condition no attempt can affect. Without
  this rung, a quota error burns every attempt in seconds and the run reports
  "not solved within N attempts" — which reads as a failure of the work and is
  not one.
- **`Solved` second**, and it absorbs the attempt ceiling: a run out of attempts
  stops and answers with what it has rather than falling through to a rung that
  would spend more.
- **`Reported` ahead of the two stuck arms.** An attempt that reaches the same
  answer it already had reports no progress, so `unproductive` is exactly what an
  unverified run accumulates. Below the stuck rungs, that run diversifies —
  spending fresh work looking for a new line of attack on a question whose answer
  is already on disk, when the thing actually missing is the second independent
  route it has just said twice it cannot build.
- **`Diversify` on either stuck signal.** The `computational` arm catches the run
  that is doing well by its own report and going nowhere: every pass establishes
  something, so `unproductive` never fires, and none of them changes the method.
- **`Retry` last**, as the only rung with no threshold. Falling through means
  the run is making ordinary progress.

### Two roles, two questions

A pass asks two different questions, and conflating them is how a loop ends up
unable to stop or unable to steer.

**Reflection** asks whether the answer is *right*. It reads the attempt report
and the artifacts, and it is **the only role that may end the loop** — it is
what sets `solved`, and what reports an answer as unverified.

**Judge** asks whether the attempt was *conducted* in a way the next one should
inherit. It never ends the loop. It returns:

```rust
pub enum Judgement {
    /// The direction is sound. Carry it forward unchanged.
    Proceed,
    /// The direction is sound but misaimed. Carry this correction forward.
    Steer(String),
    /// The direction is wrong. Discard it and start over.
    Restart(String),
}
```

The two run **concurrently** as arms off the same attempt, because neither reads
the other's output. A restart is therefore not a route — by the time anything
routes, the reflection has already happened and there is no reflection left to
skip. What a restart does is what the judge step records: the direction is
discarded, `restarts` is incremented, the correction is written for the next
attempt, and the pass is marked unproductive.

### Rules

Each rule is stated with the failure it prevents.

**A `Solved` verdict requires three things at once**: the literal completion
marker in the reply, a **mechanical** signal that the work happened (an artifact
on disk, a ledger delta, a check that passed), and internal consistency between
the claim and the artifact. Any one missing is not `Solved`.

*Why.* A claimed answer with no artifact behind it is the signature failure of
this whole class of system. A model that has been asked for eight passes whether
it is finished eventually says yes, and the marker alone cannot distinguish that
from finishing.

**An unreadable or unparseable verdict resolves to the cheap outcome.** A judge
reply the loop cannot parse is `Proceed`, not `Restart`. A progress field the
loop cannot read moves no counter.

*Why.* A verdict the loop cannot read must not throw work away by accident. The
corollary is the one that bites: a misspelling that falls through to a
*terminal* default kills runs, silently, and the run reports the terminal state
as though it had been reached on the evidence. Defaults point at the cheap
outcome in every direction, and the expensive outcomes require an explicit,
recognised answer.

**Restarts are bounded by `max_restarts`.** A judge that dislikes the whole
approach otherwise resets the run every pass until the attempt ceiling stops it,
and the run ends having explored nothing to its conclusion — maximum spend,
minimum depth.

**The attempt ceiling outranks a restart.** A run on its last attempt reflects
on what it has rather than discarding it. Stopping with nothing, one pass from
the end, is worse than stopping with a partial answer and saying it is partial.

**Every `Thresholds` value ships with its rationale**, and every preset states
its bet. See the `Thresholds` section above.

### `Autonomy`

```rust
pub enum Autonomy {
    /// Decide nothing. Produce a plan and a report; take no action.
    Report,
    /// Act, but ask before anything the policy marks as gated.
    Assisted,
    /// Act throughout, within the budget.
    Unattended,
}
```

`Autonomy` gates what a loop may do without a human present, and it maps onto
the engine's `ApprovalProvider`
(`vendor/tinyflows/src/caps/approval.rs:143`). The mapping is deliberate rather
than incidental: with **no** provider injected, an approval falls back to
pausing the run for `tinyflows::engine::resume`
(`vendor/tinyflows/src/model/node_kind.rs`, `NodeKind::Approval`) rather than
proceeding. A host that forgets to wire approvals gets a paused run it can see,
not an unattended one it cannot.

`Report` and `Assisted` therefore emit gated nodes; `Unattended` emits the same
graph without them, and that difference is visible in the topology rather than
in a prompt. **A prompt instruction is not a control.**

### Evidence

Two of the rules above are principled, not matters of taste, and the evidence
belongs beside them.

**Retry versus diversify is a crossing point, and `stuck` estimates it.**
Sequential self-revision — take the last attempt and improve it — outperforms
sampling several attempts in parallel and selecting among them only when the
feedback driving the revision is highly accurate; below roughly 90% feedback
accuracy the parallel-and-select strategy wins, because a revision driven by
wrong feedback moves away from the answer. Kamoi et al., *When Can LLMs Actually
Correct Their Own Mistakes? A Critical Survey of Self-Correction of LLMs*
(TACL, 2024) surveys the condition; Huang et al., *Large Language Models Cannot
Self-Correct Reasoning Yet* (ICLR, 2024) shows intrinsic self-correction
degrading performance without reliable external feedback; Snell et al., *Scaling
LLM Test-Time Compute Optimally Can Be More Effective Than Scaling Model
Parameters* (2024) shows the sequential/parallel optimum moving with problem
difficulty. Consecutive unproductive passes are the run's own observable
estimate that its feedback is on the wrong side of that crossing, which is why
`stuck` is the trigger for `Diversify` and not a cosmetic patience setting.

**Iteration should be bounded by saturation, not by a large maximum.** Iterative
self-improvement saturates: successive rounds return less, and past a few rounds
they return approximately nothing while continuing to cost. Song et al., *Mind
the Gap: Examining the Self-Improvement Capabilities of Large Language Models*
(ICLR, 2025) is the primary source: self-improvement is governed by the gap
between a model's ability to verify and its ability to generate, and iterating
against that gap saturates rather than converging. Madaan et al., *Self-Refine*
(NeurIPS, 2023) reports the same shape empirically, gains flattening across
iterations. So a run should stop when the last *n*
rounds produced no verdict improvement — the `Stalled` terminal state of
[`loop-kernel.md`](loop-kernel.md) invariant 10 — and `max_attempts` is a
backstop against a runaway rather than the intended stopping rule. A large
`max_attempts` with no saturation detector buys nothing but spend.

## Invariants and constraints

- `route` is a **pure function** of the merged counters and a `Thresholds`. It
  reads no clock, no provider, and no file, which is what makes exhaustive
  parity testing possible at all.
- The ladder's Rust and the ladder's rendered jq are generated from the same
  `Thresholds` value. No threshold literal appears in graph JSON.
- Parity is proved exhaustively across the whole counter space, for **every**
  shipped preset, not for one. A preset with a higher `max_attempts` gets a
  sweep that reaches past it rather than a fixed range that stops short and
  calls the untested room agreement.
- Parity proves the *translation*, never the answer. Both sides read the same
  number, so a wrong threshold is wrong in both and agrees with itself.
- Every parsing path has a defined result for unrecognised input, and that
  result is the cheaper of the available outcomes.
- `Judgement` cannot set `solved`. Enforced by the capability-typed step context
  of [`loop-kernel.md`](loop-kernel.md) invariant 11, not by review.
- A `Route` value is serializable and appears verbatim in the run's events, so a
  reader of a finished run can see which rung fired.

## Acceptance criteria

- Unit tests pin each rung's boundary at `t - 1`, `t`, and `t + 1` for every
  field, and a test asserts each rung's *position* by constructing a state that
  satisfies two rungs at once and asserting the higher one wins.
- A test constructs a state with `blocked >= t.blocked` and
  `attempts >= t.max_attempts` and asserts `Blocked`, not `Solved`.
- A test constructs a state with `unverified >= t.unverified` and
  `unproductive >= t.stuck` and asserts `Reported`, not `Diversify`.
- The parity harness sweeps every counter combination past every threshold for
  every preset and reports the first disagreement with the preset's name and the
  offending state.
- A `Solved` claim with the marker present and no mechanical signal does not set
  `solved`, and the test names the artifact it was missing.
- An empty judge reply, a truncated one, and one with the verdict misspelled all
  yield `Proceed`; a test asserts none of them yields `Restart`.
- A judge returning `Restart` at `restarts == t.max_restarts` does not increment
  past the bound and does not discard the direction.
- A run at `attempts == t.max_attempts - 1` receiving `Restart` still reflects,
  and the run ends `Solved` or `Reported` rather than with nothing.
- Every `Thresholds` field and every preset has rustdoc stating its rationale;
  a doc lint asserts no field is undocumented.
- Building under `Autonomy::Assisted` emits approval-gated nodes and under
  `Unattended` does not; the difference is asserted on the emitted graph, not on
  a prompt string.

## Open questions

- Whether `computational` generalises off its originating domain, or whether the
  right shape is a caller-supplied classifier returning "progress of a kind this
  run has already had" so the counter has a domain-agnostic meaning.
- Whether the saturation window for `Stalled` should be a `Thresholds` field or
  a separate detector composed with `&`, given it is the one bound read by the
  loop head's `config.until` rather than by `route`.
- Whether `Reported` should be reachable when no artifact exists at all, or
  whether an unverified answer with nothing on disk is closer to `Stalled`.
