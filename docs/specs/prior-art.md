# Prior art: what the literature and the surveyed harnesses settle

**Status:** Accepted — 2026-08-29
**Owner:** TinyLoops maintainers
**Bibliography:** [`prior-art-bibliography.md`](prior-art-bibliography.md)

## Problem

Every default in a loop framework is an argument about method: how many
attempts before changing approach, who is allowed to judge an attempt, what
survives compaction, when to stop. A framework that ships those defaults
without recording why is asking every consumer to re-derive them, and — worse —
leaves the maintainers unable to tell a considered choice from an accident when
one later needs changing.

This specification is the evidence layer. It states what the published record
actually establishes, what it merely suggests, and what it contradicts, and it
maps each finding onto the invariant it justifies elsewhere in `docs/specs/`.
Where a claim could not be verified against a primary source, it says so.

## Goals and non-goals

**Goals.** Record the findings this framework's defaults rest on, with a
citation and, where reported, a number. Name the anti-patterns the record warns
against. Make every design commitment traceable to the finding behind it.

**Non-goals.** This is not a literature survey for its own sake, and not an
argument that the framework is novel. It does not specify behavior — the
specification files it points at do that. It does not settle disputes the
literature has not settled; where the record is genuinely contested, it says so
and the framework makes the choice configurable rather than picking a winner
quietly.

## Proposed behavior

Nothing here is executable. What this file produces is a set of **commitments**,
each written as `C-n` and referenced by the specification that implements it.

### Verification is the binding constraint

The strongest result in the record is negative, and three independent groups
reach it: intrinsic self-correction — a model critiquing its own reasoning with
no external signal — does not improve reasoning and sometimes degrades it
(Huang et al. 2023; Valmeekam et al. 2023; Stechly et al. 2023). Stechly's
sharpest finding is that where external verification *does* help, the
improvement is attributable to the correct answer being present in the sampled
completions and recognised, not to the content of the critique. CRITIC (Gou et
al. 2023) is the counterweight and it agrees on the mechanism: self-correction
works when validation comes from tool interaction.

Self-Refine (Madaan et al. 2023) reports roughly 20 points of improvement from
ungrounded self-critique and is *not* a contradiction: its seven tasks are
dominated by open-ended generation where "better" is a preference judgment. The
scope difference is the finding. Self-critique helps where the quality signal is
stylistic and the model can perceive it, and fails where correctness is the
signal and perceiving the error is exactly the hard part.

> **C-1.** Grounding is a type, not a convention. A verifier is
> `Program`, `Tool`, or `Model`, and a model-only verdict on a
> correctness-bearing question requires an explicit opt-in. By default no
> refinement step fires without a verdict from a grounded source.
> Implemented by [`routing-and-policy.md`](routing-and-policy.md).

Repeated sampling raises *coverage* over four orders of magnitude — one agent
goes from 15.9% at one sample to 56% at 250 on a software benchmark (Brown et
al. 2024) — but the same paper reports that without an automatic verifier,
majority voting and reward models plateau after several hundred samples.
Optimising against a proxy reward eventually *decreases* true performance (Gao
et al. 2022), and accuracy under vote-style aggregation follows an inverted U in
the number of calls (Chen et al. 2024).

> **C-2.** Sampling breadth is bounded by verifier grounding class: generous
> under a budget for program verifiers, hard-capped for model judges. The
> framework does not offer an unbounded best-of-n against a model judgment.

Process supervision — scoring each reasoning step — outperforms outcome
supervision, which scores only the final answer (Lightman et al. 2023).

> **C-3.** The trace is step-addressable and every step is independently
> scorable. A loop that records only a terminal verdict has discarded the
> signal that empirically works better.
> Implemented by [`observability.md`](observability.md).

Self-improvement is governed by a generation-verification gap and **saturates**
after a few rounds rather than converging (Song et al. 2024).

> **C-4.** Iteration is bounded by a saturation detector — no verdict
> improvement over the last *n* rounds — not by a large fixed maximum.

An evaluator separated from the generator helps, but is still a model with
self-preference and position bias, and that bias *grows as the quality gap
narrows* (Zheng et al. 2023). Model confidence is not a usable proxy for
correctness: frontier models are strongly overconfident in wrong solutions
(Nezhurina et al. 2024).

> **C-5.** Where a model judge is offered, position randomisation and length
> normalisation are defaults inside the judge harness rather than something
> each consumer reimplements, generator and judge default to different
> configurations, and confidence is never a stopping signal.

### The critic must not wear the generator's hat

Relabelling an identical erroneous claim from the assistant role to a
non-assistant role raises the explicit correction rate by 23 to 93 percentage
points across 10 of 12 model-domain settings.

> **C-6.** An evaluation arm receives the attempt report as *input*, never as
> its own prior assistant turn. The fan-out shape in
> [`loop-kernel.md`](loop-kernel.md) makes this natural, and it must not be
> optimised away into a continued conversation.

### Sequential revision versus parallel sampling has a crossing point

Sequential self-revision beats best-of-N only when feedback accuracy exceeds
roughly 90%; below that, sampling in parallel and selecting wins. Compute-matched
comparisons are contested in the other direction too — one recent preprint
reports sequential scaling beating parallel self-consistency in 95.6% of
configurations at matched compute (Sharma & Chopra 2025) — so this is a live
dispute rather than a settled result.

> **C-7.** `Thresholds::stuck` is documented as an estimator of that crossing,
> `Route::Diversify` is the parallel arm rather than a consolation prize, and
> the aggregation strategy is a configuration of one budget rather than two
> unrelated APIs so a consumer can compare them per token.

### Context is smaller than it is advertised to be

Accuracy on long inputs is U-shaped in the position of the relevant material
(Liu et al. 2023). Only about half of models claiming 32K context hold up at
32K on multi-hop tracing and aggregation (Hsieh et al. 2024), and 11 of 13 fall
below half their short-context baseline at 32K once matching is non-literal —
one frontier model going 99.3% to 69.7% (Modarressi et al. 2025). Degradation
begins far below the technical maximum, and **perplexity does not predict it**
(Levy et al. 2024).

> **C-8.** The usable context budget is a configured, enforced value well under
> the model maximum, prompt assembly is owned by the framework with an explicit
> positional policy that places the highest-salience material at the edges, and
> no loss- or perplexity-based proxy is used to decide when context is too long.

Compaction is the weakest-evidenced area in this file and is recorded as such.
The one directly verified study finds that recurrent context compression
degrades the influence of recent interactions, producing blocked actions,
repeated exploration, and run-to-run instability (Min et al. 2026). Several
further titles surfaced in search and could not be confirmed; they are excluded.
Against that, four independent implementations converge on recorded rather than
destructive condensation, one reporting up to 2× cost reduction with no measured
degradation.

> **C-9.** Compaction is recorded, not destructive, and reversible: the
> pre-compaction trace stays addressable. Recent steps are pinned out of the
> compaction window. Stated as a cheap hedge against a real-but-underquantified
> risk rather than as a settled result.
> Implemented by [`seams.md`](seams.md).

Pinning is better evidenced than compaction itself: policy-violation rate goes
from 0% to 30% after compaction, is 0% when the constraint survives and 38% when
it is dropped, and pinning restores 0% for roughly 47 tokens.

### Errors in the trace are not inert

Models become measurably more error-prone once their own prior erroneous outputs
are in context, and long-horizon failures on simple tasks are largely *execution*
failures rather than reasoning or planning failures (Sinha et al. 2025). The same
paper reports that small single-step accuracy gains compound into exponential
increases in achievable task length. Independently, an agent keeping only its
last five observations outperformed one keeping full history by 3.0 points.

> **C-10.** A verified-failed step's output is excisable from the conditioning
> context, replaced by a terse marker naming the attempt and the reason. The
> full content stays in the journal; it leaves the view. Three independent lines
> converge here, which is why this is a requirement rather than an option.

### Interface design beats interface volume

The ablations on one agent-computer interface are the best-verified evidence in
the record on how much presentation matters, all against an 18.0% baseline: a
30-line file window instead of 100 costs 3.7 points; putting the full file in
context costs 5.3; iterative rather than summarized search results cost 6.0;
removing lint validation from the edit command costs 3.0; removing the structured
edit command entirely costs 7.7; full history instead of last-five costs 3.0.
(These digits come from the paper's HTML rendering and are internally consistent
with secondary summaries; they were not confirmed against the PDF table.)

The same body of work reports that 51.7% of trajectories contain at least one
failed edit and that recovery probability falls from 90.5% to 57.2% after a
single edit failure. Across 13 surveyed scaffolds, every LLM-driven agent
converges on the same four capability categories — read, search, edit, execute —
whether it exposes one tool or 37.

> **C-11.** An observation is a designed, bounded artifact, not whatever the
> tool printed. A mutation carries a validator that can reject it *before* it
> lands, returning diagnostics rather than committing a broken state. Tool
> surface is judged by granularity, not count.
> Implemented by [`seams.md`](seams.md).

### Cheap disconfirmation before expensive proof

A project resolving 22,028,942 implications reports that 165 CPU-hours of
brute-force search over the smallest structures refuted 61.9% of them, and that
only 0.13% of the positive implications needed a direct proof — the rest followed
by closure under transitivity (Equational Theories Project 2025). **Correction
worth recording:** the paper does not articulate a "refutation before proof"
policy. The ordering is an inference from those numbers, not a stated project
decision, and this file presents it as such.

> **C-12.** Evaluation is tiered with early rejection — cheap filters over the
> whole candidate space, expensive verification only for survivors — and
> established results are closed under entailment before new work is scheduled,
> so nothing is re-derived that already follows.

The highest-leverage architecture in the record is neural proposal plus symbolic
or executable verification (FunSearch 2024; AlphaGeometry 2024; AlphaProof 2025),
with the evaluator as the load-bearing component and the model as a proposal
distribution. Program search of this kind is population-based: a diverse pool of
scored candidates, not one current best.

> **C-13.** Attaching a real deterministic checker is the primary integration
> point, not an afterthought.

### What is promoted between runs should be executable

An agent that promotes successful sub-trajectories into an executable, testable
skill library reached 15.3× faster milestone progress than the prior state of the
art (Voyager 2023). Set against that, verbal self-reflection stored as prose
persists incorrect reflections and re-conditions on them across trials.

> **C-14.** What crosses between runs is executable and evictable where it can
> be. Reflection is gated on an *external* failure signal and its memory is
> bounded with an eviction path — never an unbounded prose log.

### Multi-agent is an escalation, not a default

Multi-agent debate in its default configuration does not reliably outperform
simpler strategies such as self-consistency at comparable cost, and is *more*
hyperparameter-sensitive (Smit et al. 2023). The widely cited positive result
lacks an equal-compute single-agent baseline. A study of 1,600+ annotated traces
across 7 frameworks produces a 14-mode failure taxonomy in three categories, and
concludes that better prompts and refined topologies give limited gains where
structural redesign is what is needed (Cemri et al. 2025). Its third category is
**task verification and termination** — the same bottleneck as C-1, arriving by
another route.

> **C-15.** The single-threaded loop is the ergonomic default and delegation is
> an explicit escalation. Sub-agents exist for **context isolation** — keeping a
> noisy search out of the main trace — which the interface ablations support
> directly, rather than for "diverse perspectives", which the debate literature
> does not support at matched compute. Termination and cross-agent verification
> are framework guarantees, not per-application prompt engineering.

### Cost is a result, and benchmarks are contaminated

Agent evaluation optimises accuracy while ignoring cost, and jointly optimising
both greatly reduces cost at maintained accuracy (Kapoor et al. 2024). A 61%
single-attempt pass rate becomes 25% over eight attempts, and capability
rankings invert at long horizons. Meanwhile models identify buggy file paths
from issue text alone with up to 76% accuracy on one popular benchmark, dropping
to 53% on repositories outside it, and 32.67% of successful patches on that
benchmark had the solution present in the issue text.

> **C-16.** Every run returns `(result, cost)` where cost covers generation,
> verification, and retries; a harness reports a cost/accuracy frontier and
> repeat-reliability rather than a single accuracy number; and any benchmark
> integration carries its contamination caveat in the documentation.
> Implemented by [`budget.md`](budget.md) and [`observability.md`](observability.md).

### The harness is production code

Six weeks of degraded behaviour in a widely used coding agent were traced to
three product-layer changes with model weights and inference untouched: a
lowered default reasoning effort, a context cleanup meant to run once that ran
every turn, and a verbosity-limiting system prompt that cost about 3% of coding
quality. The second took over a week to find and presented as both forgetfulness
and continuous prompt-cache misses.

> **C-17.** Context-mutating optimisations require idempotence tests;
> prompt-cache hit rate is emitted on the event stream rather than derived
> later; and the system prompt is treated as production code, with per-model
> ablation and a soak period before it becomes the default.

### The durability bug everyone hits

Across the durable-execution documentation surveyed, the most-reported failure
is the same one: *forgot the wrapper* — code that works and is silently
non-durable. Wrapping a whole third-party agent loop in one activity destroys
granular durability, which is a direct argument for a framework that owns its
loop rather than delegating it.

> **C-18.** Effects reach the outside world only through a capability trait, so
> the boundary is un-forgettable rather than something an author must remember
> to call. Stable declared identity for every durable participant, and a
> topology hash on every checkpoint.
> Implemented by [`loop-kernel.md`](loop-kernel.md) and [`seams.md`](seams.md).

### What loop engineering claims, and what its own advocates concede

The term's founding text places the loop one floor above the harness — the
harness is the environment one agent runs inside; the loop schedules, spawns
helpers, and feeds itself — and names external state as a component because the
model forgets everything between runs. Its skepticism is inside the founding
text: *"Verification is still on you. A loop running unattended is also a loop
making mistakes unattended"*, and *"the model that wrote the code is way too
nice grading its own homework."* The operational reading adds a governance
ladder: report-only, assisted, then unattended, the last earned only after a
period of reliable verification.

The two disagree about what the practice is. One makes it a posture where the
human stays the engineer; the other makes it a maturity model where autonomy is
a privilege with an audit score attached. Both agree the verifier is the
constraint. The strongest critique concedes the practice and attacks only the
durability of the name, arguing the loop may be a temporary shape while tooling
catches up.

> **C-19.** `Autonomy { Report, Assisted, Unattended }` is a first-class policy
> value gating what a loop may do unattended, not a configuration flag. It maps
> onto the engine's approval capability, whose absence already pauses a run
> rather than proceeding.

## Invariants and constraints

- **Every default has a citation or is marked as a judgement.** A constant in
  this framework either points at a finding here or carries the reasoning that
  produced it. An unrecorded decision defended by nothing is the failure mode
  this repository keeps naming.
- **A contested result is recorded as contested**, and the framework makes the
  choice configurable rather than picking a winner silently. C-7 is the current
  example.
- **An unverified number does not appear as a fact.** The bibliography marks
  every figure that could not be checked against a primary source, and this
  specification does not restate one without the marker.
- **This file is superseded by evidence, not by preference.** A default changes
  when a finding changes, and the change updates the commitment here in the same
  edit.

## Acceptance criteria

- Every commitment `C-1` … `C-19` is referenced by at least one other file in
  `docs/specs/`, or is explicitly listed below as not yet implemented.
- No numeric claim appears here without either a bibliography entry or an
  inline note that it is unverified.
- Not yet implemented, and tracked in `ROADMAP.md`: C-12 (tiered evaluation and
  entailment closure), C-13 (deterministic checker integration), and C-14
  (executable skill promotion). These describe where the framework is going and
  are recorded now so the gap is visible rather than forgotten.

## Open questions

1. **C-7's direction.** Sequential-versus-parallel at matched compute is
   genuinely disputed. The framework ships both behind one budget; which is the
   *default* is unresolved and should be settled by measurement on this
   framework's own workloads rather than by citation.
2. **What replaces confidence as a stopping signal** when no grounded verifier
   is available at all. The record says what not to use and does not say what to
   use instead. Today the answer is the budget, which is honest but blunt.
3. **Whether recorded condensation is enough.** C-9 hedges against a risk the
   literature has not quantified. If a rigorous study of compaction lands, this
   commitment should be rewritten to match it rather than kept out of caution.
4. One source named in the original research brief — a practitioner discussion
   thread — could not be retrieved because the domain is blocked to automated
   fetching. Its criticism is not represented here and would need a manual read.
