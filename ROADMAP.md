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
- two runnable loop examples — `simple_loop` in plain Rust, and
  `tinyagents_harness` under the durable harness — both offline against
  TinyFlows' mock capabilities

## Next

In roughly this order, because each one is what the following one needs:

- the loop kernel: run state, policy and routing, steps, arms, and the graph
  builder that wires them
- the orchestrator that drives a goal run end to end
- the seams — harness, memory, tools, workspace — as traits an embedder
  implements, so the framework picks no vendor
- budget and observability: the limits a run carries and the events it emits
- the `research_loop` preset and the example that runs it

## Out Of Scope

Not never, but not now, and nothing here is blocking the list above:

- porting the riemann math runtime onto the framework
- standing teams, and multi-school parameterisation
- a TinyBus control surface for driving or inspecting a running loop
- any remote observability exporter; `observe/` emits events, and shipping them
  somewhere is the embedder's decision
- anything that cannot be tested deterministically
