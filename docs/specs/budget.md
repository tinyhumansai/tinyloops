# Budget: the limits every run carries

- **Status:** Accepted — 2026-08-29
- **Owner:** Maintainers
- **Related:** [`loop-kernel.md`](loop-kernel.md), [`seams.md`](seams.md),
  [`routing-and-policy.md`](routing-and-policy.md),
  [`observability.md`](observability.md)

## Problem

A loop that can retry can also fail to stop. Every attempt looks locally
reasonable — one more pass, one more tool call, one more sub-agent — and the run
ends when something external kills it, which is the one ending that produces no
report.

A single global cap does not solve this. A run has nested scopes, each of which
can overrun independently: the loop can spin without any single step being slow,
a step can hang without the loop having taken many passes, a role given the
wrong budget can spend an investigation's worth of calls on a four-line answer,
and a request can hang forever inside a tool call that itself has a timeout.
Whichever bound trips first determines what the run can still report, so the
question is not only "is there a limit" but "which limit trips, and what
survives when it does".

## Goals

- Define the concentric bounds a run carries, outermost to innermost.
- State which bound must trip first, and why that is a correctness property
  rather than a tuning preference.
- Require per-role narrowing rather than one budget applied everywhere.
- Define what the loop meters: effective feedback alongside raw compute.
- Make cost a return value of a run, not telemetry emitted beside it.

## Non-goals

- Pricing. This crate does not carry a price table; see
  [`observability.md`](observability.md) for reading cost off the response.
- Deciding whether a run is worth its budget. The loop enforces the budget the
  caller set.
- Rate limiting or quota management across concurrent runs, which belongs to the
  deployment.

## Proposed behavior

### The concentric bounds

Outermost first:

1. **The loop.** `max_iterations`, the `until` condition, and a wall clock. All
   three together: an `until` condition alone never fires on a loop making no
   progress, and `max_iterations` alone does not bound a loop whose passes are
   slow.
2. **Per-step thresholds.** A step that exceeds its own threshold ends as a step
   outcome the router can act on, without consuming the loop's remaining
   passes silently.
3. **Per-role caps.** Model calls, tool calls, run clock, and turn tokens,
   attached to the `Role` record defined in [`seams.md`](seams.md).
4. **Per-tool timeout.** The harness already carries the field:
   `ToolRuntime::timeout_ms` and `ToolTimeout`
   (`vendor/tinyagents/src/harness/tool/types.rs:321`).
5. **Per-request timeout.** The single HTTP or provider call.
6. **A retry ladder with jitter.** Bounded attempts, backing off, jittered so a
   fleet of runs does not retry in lockstep.

### Only one cap may be the one that trips

Within a scope, the caps are not equally graceful. Some overruns end with a
report and partial results; others end with nothing. The bounds must therefore
be set so that **the cap whose overrun is graceful is the one that trips**, and
that ordering is asserted at construction, not left to whoever writes the
configuration.

Concretely: the tool-call cap is set far above reach so that the model-call cap
trips first, because a graceful "stop with partial results" is honoured on the
model-call path and not on the tool-call path. Two caps that can both trip is a
configuration bug, and the constructor rejects it.

The same rule fixes an ordering: **`tool_timeout < run_timeout`.** An expired
tool call returns the output it captured, tagged with a timeout status, and the
run continues holding everything it had. An expired run loses its context and
its report. If the run clock can expire while a tool call is outstanding, the
tool's graceful path is unreachable.

### Per-role narrowing is required

A role's budget is part of its definition, and a loop that gives every role the
same budget has not budgeted. The failure is not overspending in the abstract: a
role that reads a report and answers in four lines, given an investigation's
budget, investigates. It has the calls, so it uses them.

The observed shape: a judge running on a wide budget spent four minutes and
fifteen model calls reading source files, while the attempt it was judging —
already finished — waited on it. Nothing failed. The judgement was fine. The
loop was simply four minutes slower per pass than it needed to be, and no cap
was exceeded because none of them was narrow enough to notice.

So a `Role` without explicit caps is not given the loop's caps by default; it is
a construction error.

### A cap is a quality feature

Budget caps are usually argued as cost control. They are also an accuracy
signal, because the runs that overrun are disproportionately the runs that were
going to fail: for a comparable agent, successful runs take a median 12 steps
and $1.21, while failures take 21 steps and $2.52. A run that has doubled the
median is not usually about to succeed on the next pass, and stopping it early
loses less than it appears to.

This is why a tripped budget is a routed outcome rather than an error: see
[`routing-and-policy.md`](routing-and-policy.md). The loop stops, reports what
it has, and says which bound stopped it.

### Metering effective feedback

The loop meters **effective feedback**, not only turns or tokens. A pass counts
against the effective-feedback meter when it produced a usable signal — a test
result, a diff, a verdict — and passes that produced none are counted, and
reported, separately from those that did.

The reason is that raw compute is nearly uninformative about outcome. Measured
against outcomes, raw compute shows near-zero fit, while an effective-feedback
measure fits R²=0.93; budgeting on it rather than on token count improved pass
rate from 61.2% to 68.2% while cutting mean cost from 213.8 to 85.1. A loop that
counts only turns cannot distinguish ten productive passes from ten passes that
each learned nothing, and those are the two cases a budget most needs to tell
apart.

Both meters are carried: raw compute bounds the worst case, effective feedback
informs the stopping decision.

### Cost is a return value

A run returns its cost, covering generation, verification, and retries. Not a
metric emitted on the side — a field of the run's result, alongside its outcome,
because a caller deciding whether to accept a result needs to know what it cost
to produce, and a caller comparing two configurations needs both numbers from
the same call.

An evaluation harness built on this reports a cost/accuracy frontier, never
accuracy alone. An accuracy figure with no cost beside it is not a comparable
result: any configuration can be made more accurate by spending more, so the
number without its price says nothing about the design.

## Invariants and constraints

- Every run carries all six bounds. None is optional, and none defaults to
  unbounded.
- The loop bound is the conjunction of `max_iterations`, `until`, and a wall
  clock; satisfying any one of them stops the loop.
- Within a scope, exactly one cap is reachable. A configuration in which two
  caps could each trip first is rejected at construction.
- The reachable cap is the one whose overrun path preserves partial results.
- `tool_timeout < run_timeout` holds for every role, asserted at construction.
- Every role declares its own caps. A role built without them is a construction
  error, not a role inheriting the loop's.
- A tripped bound produces a routed outcome carrying which bound tripped, the
  work completed, and the results in hand. It is never a bare error.
- Both meters advance: raw compute and effective feedback. A pass that produced
  no usable signal advances the first and not the second, and the difference is
  visible in the run's report.
- Cost is a field of the run's result and accounts for generation, verification,
  and every retry.
- No bound is enforced by prompt text. A prompt instruction is not a control.

## Acceptance criteria

- Constructing a budget whose tool-call cap is reachable before its model-call
  cap is rejected, with an error naming both caps.
- Constructing a role with `tool_timeout >= run_timeout` is rejected.
- Constructing a role with no caps is rejected.
- A loop that makes no progress stops on `max_iterations`; a loop whose passes
  are slow stops on the wall clock. Two tests, neither relying on the other's
  bound.
- A step exceeding its threshold yields a step outcome the router receives, and
  the loop's remaining pass count is unchanged by it.
- Tripping the model-call cap mid-pass produces a result containing the partial
  work, the tripped bound's identity, and the accumulated cost.
- A tool whose timeout expires returns its captured output with a timeout
  status, and the run continues.
- Ten passes producing signal and ten producing none reach the same raw-compute
  total and different effective-feedback totals, and the run's report shows
  both.
- The run result's cost includes a verification call and a retried call, proven
  by a test whose expected total is the sum of all three legs.
- No test achieves a bound by instructing a model to stop.

## Open questions

- What qualifies a pass as having produced usable signal in the general case. A
  test result and a verdict are clear; a partial diff that fails to apply is
  not, and counting it either way changes the stopping decision.
- Whether the "exactly one reachable cap" check can be made total, or whether
  some scopes admit configurations where reachability depends on run-time
  behavior and can only be asserted for the common case.
