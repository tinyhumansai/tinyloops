# 3. Three layers, with `tinyflows-adaptive` as the outer one

- **Status:** Accepted
- **Date:** 2026-08-29

## Context

Three questions look similar and are not: how do I execute this graph, what
should this run attempt next, and which graph should the next episode run at
all. Answering all three in one crate produces a component whose state has no
boundary — the routing ladder reaches for the ledger, the ledger reaches for the
run's counters, and neither can be tested without the other.

`vendor/tinyflows/crates/adaptive` already answers the third question and states
its own boundary: *the engine may know about one run; anything that spans runs
lives here.* It holds ledger rows, scored lessons, workflow selection and
authoring, exclusion lists, and promotion. Its `intake::decide`
(`vendor/tinyflows/crates/adaptive/src/intake/mod.rs:111`) selects a stored
workflow or authors one; its closing phase judges, consolidates, scores, and
repairs. It runs the engine unmodified.

The gap is the middle question. `adaptive` chooses a graph and runs it; nothing
in either crate says what the *shape of one goal run* is — the attempt, the
concurrent evaluation, the routing ladder, the budget.

`adaptive` also carries persistence: its `Ledger` trait ships with SQLite and
Mongo backends behind cargo features. `crates/tinyloops` ships as a TinyBus
`cdylib` loaded into a host process, and a loadable module that drags a database
driver and its native library into every host is a module hosts will decline to
load.

## Decision

Three layers, one of them in this repository.

| Layer | Where | Knows about |
|---|---|---|
| Engine | `vendor/tinyflows` | one graph run; decides nothing |
| Loop | `crates/tinyloops` | one goal run: attempt, evaluate, route, budget |
| Adaptive | `vendor/tinyflows/crates/adaptive` | everything that spans runs |

Build on `adaptive` and **restate nothing it already holds**. This crate has no
ledger, no lesson store, no workflow catalogue, no scoring, and no promotion.
Where a loop needs cross-run knowledge, the embedder supplies it through a seam;
where `adaptive` needs a graph, this crate's kernel emits one.

`tinyflows-adaptive` is an **optional** dependency of `crates/tinyloops`, behind
a cargo feature, and nothing under `src/` is gated on it.

## Consequences

- The loadable module resolves neither SQLite nor Mongo nor an HTTP client. That
  is the whole reason the dependency is optional, and it is why the feature must
  stay off by default.
- The two decision points never read each other's state. `intake::decide`
  chooses which graph an episode runs; the orchestrator chooses which
  specialists a pass spawns. They compose by nesting, not by sharing a state
  object. See [`../specs/orchestrator.md`](../specs/orchestrator.md).
- A run's counters, board, and lessons live in the loop accumulator and end with
  the run. Anything worth keeping crosses out through the embedder, which is the
  only place the two layers meet.
- Duplication is a review failure with a named test: a type in this crate that
  scores a workflow, ranks a lesson, or excludes a candidate belongs in
  `adaptive`, and the boundary above is the argument.
- Upstream changes to `adaptive` arrive as a submodule bump and cannot conflict
  with this crate's code, because there is no overlapping code to conflict.
- The cost is indirection for a host that wants both: it wires `adaptive` and
  hands it a graph this crate built, rather than calling one entry point. That is
  the price of the two being independently testable.
