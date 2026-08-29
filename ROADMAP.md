# Roadmap

Short and honest: what exists, what is next, and what is deliberately out of
scope. A roadmap that lists everything is a roadmap nobody trusts.

## Shipped

- the two-crate layout: `tinyloops-bus` as the wire contract, `tinyloops` as
  the framework, with the rename from the generated scaffolding complete
- lint configuration in `[workspace.lints]`, enforced identically locally and
  in CI
- CI: format, clippy, build, test, per-file coverage, rustdoc, MSRV, an
  assertion that the contract crate stays transport-free, and supply-chain
  checks
- a manual release workflow that versions, tags, builds the TinyBus module for
  every supported platform, and creates a GitHub release
- the loop kernel: run state, policy and routing, steps, arms, and the graph
  builder that wires them, with the generated routing ladder proved against the
  Rust router exhaustively for every shipped preset
- the seams — harness, memory, tools, workspace, ledger — as traits an embedder
  implements, each with a reference implementation that runs offline
- budget and observability: the six bounds a run carries and the events it
  emits, folded into one ordered stream
- the orchestrator: a role bound to `plan`, `attempt`, and `report`, holding no
  execution tools and a closed delegate set, over a `TaskBoard` that lives in
  the accumulator
- the `research_loop` preset and its two evaluation arms, plus four threshold
  sets that each say what bet they are making
- three runnable loop examples — `simple_loop` in plain Rust, `research_loop`
  driving the whole preset, and `tinyagents_harness` under the durable harness —
  all offline against TinyFlows' mock capabilities

## Next

In roughly this order, because each one is what the following one needs:

- widen the step interface so a node body can reach its node's arguments. The
  emitted `merge` node is handed each arm's output through its tool arguments,
  but `run_loop_step` passes a step only the decoded state, so the delta fold
  cannot run inside the graph today. It is written and tested — `ArmSet::merge`,
  which `AssembledLoop::drive` calls — and a driven loop folds correctly; a loop
  run through the engine does not yet.
- run an assembled loop through the engine end to end, not only through
  `AssembledLoop::drive`
- a `Decompose` and a `Compose` backed by a model rather than by a fixture, so a
  deployment has something to start from

## Out Of Scope

Not never, but not now, and nothing here is blocking the list above:

- porting the riemann math runtime onto the framework
- standing teams, and multi-school parameterisation
- a TinyBus control surface for driving or inspecting a running loop
- any remote observability exporter; `observe/` emits events, and shipping them
  somewhere is the embedder's decision
- anything that cannot be tested deterministically
