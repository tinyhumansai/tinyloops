# Observability: the events a run emits

- **Status:** Draft
- **Owner:** Maintainers
- **Related:** [`loop-kernel.md`](loop-kernel.md), [`budget.md`](budget.md),
  [`workspace-and-ledgers.md`](workspace-and-ledgers.md), [`seams.md`](seams.md)

## Problem

Three planes already emit something during a loop run, and none of them knows
what a loop is.

The engine emits node activations: `RunObserver` reports run start, step start,
step finish with a duration and status, per-item progress, and the assembled
`Run` (`vendor/tinyflows/src/observability.rs:189-228`). It knows nodes, not
passes.

The harness emits model and tool calls with usage, through `EventListener` and
the sinks around it — `FanOutSink`, `JsonlSink`, `RedactingSink`, `JournalSink`,
and journals like `InMemoryEventJournal`
(`vendor/tinyagents/src/harness/observability/types.rs:383-441`). It knows
calls, not attempts.

And `diagnose(&WorkflowGraph, &[ExecutionStep]) -> Diagnosis`
(`vendor/tinyflows/src/diagnostics.rs:143`) reports what a green run hid: null
`=`-bindings, empty prompts, and errors an error policy swallowed. It knows one
graph, not a sequence of them.

What none of them can say is: which pass is this, which arm won, why did it
route there, what did the judge score it, and which bound stopped the run. That
vocabulary is what this module owns, and the reason it must own it is that a
person debugging a stalled run currently has to infer the loop's state from the
node activations underneath it.

## Goals

- Define the loop's own event vocabulary and the `Sink` it is delivered to.
- Fold all three planes into one ordered stream, tagged by who produced each
  entry.
- Define the derived views: per-call accounting and per-pass time attribution.
- State five rules as assertions rather than conventions.
- Define the `Report` a run returns, and make it the same payload a status call
  returns.

## Non-goals

- Storage, dashboards, and retention. This module emits; a deployment persists,
  exactly as the engine's own module states.
- Replacing either underlying plane. The engine keeps emitting node activations
  and the harness keeps emitting calls; this module joins them.
- A price table. Cost is read from the provider's response, never computed
  locally.

## Proposed behavior

### The event vocabulary

One enum, covering the loop's own transitions:

- a pass started;
- a step entered, and a step finished with its duration;
- an arm started, and an arm finished;
- a merge, with the deltas it produced;
- a judgement, with its score;
- a route, with the reason it was taken;
- a delegation, and its completion;
- an operator directive received;
- a budget bound tripped, naming the bound;
- the loop finished, with its outcome.

Each event carries the pass it belongs to, so the stream is reconstructable into
passes without consulting anything else.

### The `Sink` and the recorder

A `Sink` receives loop events. A recorder implements **both** the engine's
`RunObserver` and the harness's `EventListener`, so all three planes — loop
events, node activations, and model and tool calls — land in one ordered stream,
each entry tagged with a `who` label.

`child(label)` returns a view that shares the same counters and the same
journal, differing only in its label. One journal, many views: a per-role view
is a filter over the single stream, not a second stream that has to be
reconciled with the first afterwards.

### Derived alongside

**Per-call accounting.** Provider, model, prompt tokens, cached tokens, output
tokens, and the cost the provider reported — **read off each response body
rather than from a local price table.** With a fallback ladder the route
genuinely varies per call: the same logical request may be served by a different
provider or a different model tier than the one configured, and a local table
prices the request that was intended rather than the one that happened.

**Time attribution per pass.** Where a pass spent its wall clock. Once arms run
in parallel the profile reports a **concurrency factor** rather than idle time,
because summed arm time legitimately exceeds the wall clock and a naive
"unaccounted time" figure goes negative and is then ignored.

### Five rules

**1. Every step announces entry and duration.** One line per node per pass, on
entry and on completion. The failure this prevents: a production run printed no
driver line for 62 minutes, and which node was holding could only be inferred
from which sub-agents happened to spawn during the gap. "The run stalled" must
be a question the log answers, not one a reader answers by correlating
unrelated evidence.

**2. The loop's spine appears in every view.** Pass boundaries, verdicts,
routes, and budget trips reach a filtered per-role view as well as the whole
stream. Nobody should have to be looking at the right tab to see that the run
changed course.

**3. Payload-free by default.** Prompts and tool payloads are captured only on
explicit opt-in, and a redacting sink sits between capture and any sink —
`RedactingSink` is the existing shape
(`vendor/tinyagents/src/harness/observability/types.rs:397`). Observability that
defaults to recording prompts is a secret leak with a dashboard attached, and
the opt-in is the deployment's decision to make deliberately.

**4. The journal is not readable by the loop.** Its path sits outside the
workspace layout allowlist by construction, so the loop cannot open it — see
[`workspace-and-ledgers.md`](workspace-and-ledgers.md). The failure: one
reflection step pulled its own 1.1 MB event log into a single 339,652-token
call, to re-read a verbatim replay of what it had already seen. A log the loop
can read is a log the loop will eventually read.

**5. Prompt-cache hit rate is emitted per model call**, not derived later from
token counts. The hardest-to-find harness regression on public record showed up
first as continuous cache misses burning through rate limits, while every other
signal looked normal. A hit rate emitted per call turns that into a visible
step change at the moment the regression lands.

### The `Report`

At loop end the run produces a `Report`: attempts, route history, scores, spend,
repeat-reliability, per-step timing, and what it left undone.

It is both the human summary an example prints and the payload a status call
returns. One structure for both, so the observability surface and the control
surface cannot diverge: a field a person can see in the summary is a field a
caller can read over the bus, and neither can quietly gain something the other
lacks.

**Repeat-reliability rather than a single success bit.** A run reports how it
fared across repeats, not whether one attempt passed. A 61% single-attempt pass
rate becomes 25% when the same task must be completed over eight attempts, and
capability rankings invert at long horizons — the configuration that wins on one
attempt is not the configuration that wins on eight. A single success bit
reports the number that inverts.

## Invariants and constraints

- Every loop transition named in the vocabulary emits exactly one event, and
  every event names its pass.
- The recorder implements both `RunObserver` and `EventListener`, and all three
  planes share one ordered stream with a `who` label per entry.
- `child(label)` shares the parent's counters and journal. No view owns a
  separate journal.
- Cost and token fields are populated from the provider's response. No local
  price table exists in this crate.
- The time-attribution profile reports a concurrency factor, and never reports
  negative or "unaccounted" idle time.
- Every step emits both an entry event and a completion event carrying a
  duration. A step that emits one and not the other is a defect.
- Pass boundaries, judgements, routes, and budget trips are present in every
  filtered view.
- Prompt and tool payloads are absent unless capture is explicitly enabled, and
  a redacting sink is interposed whenever it is.
- The journal's path is not a member of the workspace layout allowlist, and
  cannot be added to one.
- Every model call emits a prompt-cache hit rate.
- The `Report` returned at loop end and the payload of a status call are the
  same type.
- The `Report` carries repeat-reliability, never a lone success boolean.
- No observability call blocks the loop. A sink that errors drops its entry and
  records the drop.

## Acceptance criteria

- A recorder registered as both a `RunObserver` and an `EventListener` produces
  one stream in which a node activation and a model call emitted during the same
  pass appear in their true order, each with a distinct `who` label.
- Events from `child("judge")` and its parent appear in the same journal, and
  the parent's counters include the child's.
- A run with no step ever emitting a completion event fails a test that asserts
  entry and completion events pair up per node per pass. This is the regression
  test for the 62-minute silent gap.
- A view filtered to one role still contains that run's pass boundaries,
  verdicts, routes, and any budget trip.
- With capture disabled — the default — no event in the stream contains prompt
  or tool payload text, asserted by scanning the serialized stream for a fixture
  secret.
- With capture enabled and a redacting sink configured, the fixture secret does
  not appear in any sink's output.
- Constructing a workspace layout that includes the journal path is rejected,
  and the loop has no API that reads the journal.
- A provider response reporting a model different from the configured one yields
  accounting naming the model that answered.
- Every model call in a completed run carries a prompt-cache hit rate.
- A pass with two arms running concurrently reports a concurrency factor above
  one, and reports no negative time.
- The example's printed summary and the status call's payload are produced from
  one `Report` value in a test that asserts both derive from it.
- A `Report` from a run of eight repeats states per-attempt outcomes and an
  aggregate reliability, and no field of it is a single success boolean.

## Open questions

- Whether a provider that reports no cost on its response should yield an absent
  cost or an estimate flagged as an estimate. An absent field is honest and
  makes the frontier in [`budget.md`](budget.md) incomputable for that run.
- What the per-pass profile does when a delegation outlives the pass that
  spawned it. Attributing it to the spawning pass overstates that pass; leaving
  it unattributed loses it entirely.
