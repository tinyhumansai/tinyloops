# 6. Thresholds addressed from run state, not rendered into the graph

- **Status:** Accepted
- **Date:** 2026-08-29

## Context

[ADR 0004](0004-routing-in-the-graph-steps-in-rust.md) settled that the graph
owns routing and Rust owns the steps, and it carried a fourth bullet about
*how* the routing gets its numbers:

> Every threshold in the rendered jq is generated from the Rust `Thresholds`
> constant. No threshold literal is typed into graph JSON.

That bullet has two halves, and only the second one turns out to be
load-bearing. The first — *generated* — means the emitted graph is a function of
the thresholds, so a threshold change is a change of topology. Three places
carry the consequence today: the loop head's `max_iterations`, the head's
`until`, and the routing switch's `expression`
(`crates/tinyloops/src/loops/builder.rs`). `GraphSignature::of` hashes each
node's `config` whole (`crates/tinyloops/src/loops/signature.rs`), so every one
of those numbers is inside the signature, and
[`loop-kernel.md`](../specs/loop-kernel.md) invariant 9 then refuses a resume
whose recorded signature does not match.

That is correct for a run whose thresholds are fixed before it starts, and it is
fatal to one that revises them. A run that retuned itself at pass 3 records a
signature describing a graph that no longer exists, and cannot survive a crash
at pass 4. [`adaptation.md`](../specs/adaptation.md) needs the revision;
this addressing scheme forbids it.

ADR 0004 anticipated the pressure in its own closing consequence: "Editing the
loop's control flow no longer requires editing Rust, which is what makes the
routing something an outside process — or a later `adaptive` repair — could
propose a change to."

## Decision

**The ladder addresses its thresholds out of the run's accumulator.**

- The routing ladder and the head's `until` read
  `.profile.thresholds.<field>` from the state the engine already hands them.
  ADR 0004's fourth bullet is amended: *no threshold literal is typed into graph
  JSON* stands, and *generated from the Rust constant* is replaced by *read from
  the same address the Rust reads*.
- One graph therefore serves every preset, and every revision of every preset.
  The jq is a fixed program rather than one rendered per `Thresholds` value.
- Every threshold read carries the fallback `// 4294967295`. `u32::MAX` is the
  sentinel for "no threshold", and it makes every rung of the ladder false, so a
  state with no profile falls through to `Retry`.
- The parity requirement of ADR 0004 is unchanged: the rendered jq and the Rust
  router are still proved to agree, now over one program rather than one per
  preset.
- The loop head's `max_iterations` stops being `thresholds.max_attempts` and
  becomes the run budget's `Caps::max_iterations`. It is a runaway backstop, not
  a routing decision, and `on_exceeded: "continue"` means reaching it emits on
  `done` rather than failing the run.

## Consequences

- A threshold change no longer changes `GraphSignature`, so a run that revises
  its own thresholds resumes from its own checkpoint. That is the whole reason
  for this decision.
- The sentinel is not a style choice. Under `jaq`, a missing key resolves to
  `null`, `null` sorts below every number, and `0 >= null` is therefore **true**
  — so an absent profile read without a fallback would fire the first rung and
  route `Blocked` immediately. The fallback points every default at the cheap
  outcome, which is the same rule [`routing-and-policy.md`](../specs/routing-and-policy.md)
  applies to an unparseable verdict.
- Parity gets stronger in one way and weaker in another, and both are worth
  stating. Stronger: the sweep now varies the thresholds themselves rather than
  testing the four tuples the shipped presets happen to hold. Weaker: the space
  is no longer finite by construction, so the sweep covers a declared box rather
  than everything, and the implementation plan says so in its own words rather
  than claiming an exhaustiveness the suite does not deliver.
- `route`, `is_terminal`, and `Outcome::classify` take the state alone. They
  stay pure functions — which is what made exhaustive parity possible — and a
  caller can no longer hand the router a threshold set the run is not using.
- The signature keeps meaning what it meant. Node ids, kinds, ports, edges, and
  the *addressing* are still hashed; what left the hash is a set of values that
  were never topology in the first place.
- This does not open the door to changing the graph's shape mid-run. Nodes,
  edges, and ports stay fixed at build time and stay hashed. Adaptation moves
  values, never topology.
- **Boundary check against [ADR 0003](0003-three-layer-split-with-tinyflows-adaptive.md).**
  That ADR assigns exclusion lists, scoring, and promotion to
  `tinyflows-adaptive`, and a reviewer will reasonably ask whether a run muting
  one of its own evaluation arms is an exclusion list by another name. It is
  not: the mute lives in one run's accumulator, ends with the run, and is scored
  by nothing. What crosses out to `adaptive` is the finished profile as plain
  data, which is the same seam every other cross-run fact uses.
