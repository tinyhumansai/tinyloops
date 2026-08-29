# TinyLoops

A loop engineering framework. It owns the shape of one goal run: attempt
something, evaluate what came back, route to the next turn, and stop when a
verdict or a budget says to.

It is the middle of three layers, and that split is what keeps each of them
small.

[TinyFlows](https://github.com/tinyhumansai/tinyflows) is the engine. It
executes one graph run and decides nothing. A loop's unit of work is a
`WorkflowGraph` it compiles once and runs per turn. Every effect (models, tools,
HTTP, code execution, persistence) goes through a capability trait, so a loop
picks its own vendors and the examples here run offline against in-memory mocks.

TinyLoops, this repository, is the run itself: attempt, evaluate, route, budget,
plus the orchestrator, roles, tools, memory, workspace, and observability a run
needs to do that honestly.

[`tinyflows-adaptive`](vendor/tinyflows/crates/adaptive) is what spans runs:
ledger rows, scored lessons, workflow selection and authoring, promotion. Its
own rule draws the line this repository works to. The engine may know about one
run; anything that spans runs lives there.

## Architecture

```text
┌──────────────────────────────────────────────────────────────┐
│ tinyflows-adaptive    what spans runs: ledger rows, scored   │
│                       lessons, workflow selection, promotion │
└──────────────────────────────┬───────────────────────────────┘
                               │ picks a graph, records how it went
┌──────────────────────────────▼───────────────────────────────┐
│ tinyloops  (here)     one goal run: attempt, evaluate,       │
│                       route, budget                          │
│                                                              │
│   orchestrate/   plan, attempt, report: the run's own agent  │
│   loops/         emits the graph below, from state + policy  │
│   policy/        thresholds, verdicts, the routing ladder    │
│   harness/ memory/ tools/ workspace/ ledger/  the seams      │
│   observe/       one ordered event stream, and the Report    │
└──────────────────────────────┬───────────────────────────────┘
                               │ compiles the graph once, runs it per turn
┌──────────────────────────────▼───────────────────────────────┐
│ tinyflows             one graph run, and no decisions.       │
│                       every effect behind a capability trait │
└──────────────────────────────────────────────────────────────┘
```

The graph `loops/` emits is the whole run, not one turn of it, so a pause and a
resume are the engine's problem rather than a driver's:

```text
  trigger ─▶ plan ─▶ research ─▶ ┌─────────────┐ ── done ─▶ stand_down ─▶ report
             (once)   (once)     │  loop head  │
                                 │ accumulator │
                                 └──────┬──────┘
                                        ▼
                                     attempt          one report, written once
                                        │
                       ┌────────────────┼────────────────┐
                       ▼                ▼                ▼
                    reflect           judge         ...more arms
                       │                │                │
                       └────────────────┼────────────────┘
                                        ▼
                                      merge   folds every arm by delta
                                        │
                                        ▼
                                      route   jq generated from Rust constants
                                        │
                                        ▼
                                      pass ───▶ the only edge back to the head
```

Three things in that picture are load-bearing. The loop head is the only writer
of the accumulator, so a checkpoint and a resume carry the counters rather than
losing them with a stack frame. Every verdict, retry, and diversify leaves
through the single `pass` node, because the engine's node map is cumulative and
a fold that reads "the merge if it ran, otherwise the reflection" will read a
stale merge forever. And the arms read the attempt report, never the
accumulator, since the head folds at the top of a pass and mid-body the
accumulator is one pass behind.

The arms are concurrent and they converge on a real barrier. They also receive
the attempt as input rather than as their own prior turn, which is not a
stylistic choice: relabelling an identical wrong claim away from the assistant
role raises the correction rate by 23 to 93 points across most model and domain
pairs. `docs/specs/prior-art.md` carries that evidence and the rest of it.

## Two crates

`crates/tinyloops-bus` is the wire contract: member names, payload types, and
the contract version, with no transport and no behavior. `crates/tinyloops` is
the implementation. A host that only makes calls depends on the contract crate
alone and compiles neither the module nor `tinybus` itself.

`tinyloops` depends on the contract crate and re-exports all of it, so a payload
type named through the framework and the same type named through the contract
are one type rather than structural twins. A parallel set of payload types for
hosts would mean a conversion at every call site that nothing checks.

Two more dependencies are vendored rather than reimplemented.
[TinyAgents](https://github.com/tinyhumansai/tinyagents) is the optional durable
harness: once a loop has to pause, checkpoint, resume, or be watched, a `while`
statement stops being enough and the control flow moves into a harness graph. It
sits behind the `tinyagents` cargo feature, so a shipped module never resolves
it. [TinyBus](https://github.com/tinyhumansai/tinybus) is how a loop is reached
from outside the process; the workspace builds as both an `rlib` and the
`cdylib` TinyBus loads.

## The loop, in miniature

Compile the graph once, run it per turn, feed each result back in, and stop on a
judge or a budget. That is the whole shape, and it is the complete
[`simple_loop`](crates/tinyloops/examples/simple_loop.rs) example minus its
graph definition:

```rust
// Compile once, outside the loop: a compiled workflow is the reusable artifact.
let step = compile(&refine_step())?;
let capabilities = mock_capabilities();

let mut state = json!({ "score": 0 });
let mut turns = 0;

while turns < MAX_TURNS {
    let outcome = run(&step, state.clone(), &capabilities).await?;
    // The run state is keyed by node id; the loop carries the last node forward.
    state = outcome.output["nodes"]["refine"]["items"][0]["json"].clone();
    turns += 1;

    if state["score"].as_i64().unwrap_or(0) >= TARGET_SCORE {
        break;
    }
}
```

Swap `refine_step()` for a graph that calls a model, and `score` for a real
judge, and nothing around them changes. The turn budget belongs to the loop
rather than to a caller who remembers to add one. A loop without a budget is a
way to spend an afternoon discovering that a judge never says yes.

When the loop needs durability, those same two decisions become harness nodes
and the `while` statement disappears:

```rust
let loop_graph = GraphBuilder::<LoopState, LoopState>::overwrite()
    .add_node("refine", move |state: LoopState, _ctx: NodeContext| {
        refine(state, Arc::clone(&workflow), Arc::clone(&capabilities))
    })
    .set_entry("refine")
    .add_conditional_edges(
        "refine",
        |state: &LoopState| judge(state),          // "again" or "done"
        [("again", "refine"), ("done", END)],
    )
    .compile()?;

let finished = loop_graph.run(LoopState::new()).await?;
```

TinyFlows never learns that a harness is driving it, and TinyAgents never learns
what the workflow does. See
[`tinyagents_harness`](crates/tinyloops/examples/tinyagents_harness.rs) for the
running version.

## Examples

```sh
cargo run -p tinyloops --example simple_loop                              # the loop, in plain Rust
cargo run -p tinyloops --example research_loop                            # the whole preset, end to end
cargo run -p tinyloops --features tinyagents --example tinyagents_harness # the loop, under a harness
cargo run -p tinyloops --example basic                                    # ordinary library API usage
```

`research_loop` is the one to read first. It assembles the shipped preset over
the reference seams, drives it to a terminal state, and prints every pass
boundary, every step, every arm, the merge, the verdict, and the route each pass
took with the counters it was taken on.

`tuned_research_loop` is the same loop with a third arm that may revise the
run's own configuration — its thresholds, its spend, and which arms it is still
paying for — within the room its preset declares. Every revision and every
refusal is an event and a line in the report; nothing here scores them, because
scoring a configuration against outcomes spans runs and lives in
`tinyflows-adaptive`.

Every example runs against TinyFlows' mock capabilities or the reference
implementations, so they are deterministic, offline, and need no provider
credentials. `tinyagents` is optional: the harness example declares
`required-features`, so a default build skips it instead of failing to compile.

## What you get

| Area | What is configured |
| --- | --- |
| Layout | A cargo workspace under `crates/`, split into a dependency-light wire contract and the framework that implements it; one directory module per concern, a crate-wide error type, integration tests, and runnable examples |
| Lints | `unsafe_code` forbidden, `missing_docs`, clippy `all` plus `pedantic`, and no `unwrap`/`expect`/`panic`/`todo` in library code, all declared once in `[workspace.lints]` so every crate, local run, and CI run agree |
| CI | Format, clippy, build, test (default and all features), a run of each bundled example, an assertion that the contract crate stays transport-free, at least 90% line coverage in every source file, rustdoc with `-D warnings`, an MSRV build, and a `cargo-deny` supply-chain check |
| Release | Manual `workflow_dispatch` bump that validates, versions, tags, and creates installable native module packages for every supported platform |
| Community | Issue and pull request templates, Dependabot, contributing, security, support, and code of conduct docs |
| Agents | [`AGENTS.md`](AGENTS.md) as the single source of truth, symlinked as `CLAUDE.md`, plus a `.claude/settings.json` allowlist for the standard commands |
| Loop | The engine seam: a workflow compiled once and run per turn, a judge that decides whether to go again, and a budget the loop enforces rather than the caller |
| Seams | Harness, memory, tools, and workspace are traits the embedder implements, so nothing here picks a model vendor, a store, or a runtime for you |
| Vendor | TinyBus host types and module SDK, the TinyFlows workflow engine and its adaptive loop, and the TinyAgents harness, each pinned as a `vendor/` build-time submodule |

## Layout

```text
Cargo.toml              # virtual workspace: members, shared metadata, lints
crates/
├── tinyloops-bus/      # the wire contract: what crosses the bus
│   ├── README.md       # why the contract is its own crate
│   └── src/
│       ├── lib.rs      # crate docs + the entire public re-export surface
│       ├── names/      # interface, object path, one constant per member
│       ├── <family>/   # payload types, one directory per family
│       │   ├── mod.rs
│       │   ├── types.rs
│       │   └── test.rs
│       └── version/    # contract version and the host bind rule
└── tinyloops/          # the framework: behavior, adapter, and the cdylib
    ├── src/
    │   ├── lib.rs      # crate docs + public surface, re-exporting the contract
    │   ├── error/      # crate-wide `Error` and `Result<T>`
    │   ├── state/      # what one goal run carries from turn to turn
    │   ├── policy/     # the decision a turn's outcome feeds: stop, retry, route
    │   ├── step/       # one unit of work, compiled once and run per turn
    │   ├── arm/        # the alternatives a route can choose between
    │   ├── loops/      # the graph that wires state, steps, arms, and policy
    │   ├── budget/     # turn, token, wall-clock, and cost limits
    │   ├── observe/    # the events a run emits, and who receives them
    │   ├── orchestrate/# what drives a goal run end to end
    │   ├── harness/    # the seam a durable driver plugs into
    │   ├── memory/     # the seam recall plugs into
    │   ├── tools/      # the seam a tool provider plugs into
    │   ├── workspace/  # the seam run artifacts are written through
    │   ├── ledger/     # the rows a run leaves behind for the next one
    │   ├── presets/    # assembled loops, ready to run
    │   └── tinybus_module/   # bus interface, setup, and ABI v1 exports
    ├── tests/
    │   └── public_api.rs     # integration tests against the public API only
    └── examples/
        ├── simple_loop.rs            # the loop, in plain Rust
        ├── research_loop.rs          # the shipped preset, driven end to end
        ├── tinyagents_harness.rs     # the same loop under a durable harness
        ├── basic.rs                  # ordinary library API usage
        ├── verify_module.rs          # local dynamic-module verification
        └── verify_github_release.rs  # tagged-release download and bus call
vendor/
├── tinybus/            # pinned TinyBus git submodule: host types, module SDK
├── tinyflows/          # pinned workflow engine and its adaptive loop
└── tinyagents/         # pinned durable agent + graph harness
docs/
├── README.md           # documentation index and conventions
├── specs/              # behavior and architecture specifications
├── plans/              # implementation-ordered delivery plans
└── adr/                # immutable architecture decision records
```

Directories under `crates/tinyloops/src/` that do not exist yet are where that
work lands. [`ROADMAP.md`](ROADMAP.md) says which is which.

Within each crate, feature areas use directory modules: implementation and
exports live in `mod.rs`, substantial types move to `types.rs`, and unit tests
live in `test.rs`. [`AGENTS.md`](AGENTS.md) holds the complete repository
guidance, and `CLAUDE.md` is a symlink to it so every coding agent reads one
source of truth.

## Development

Clone with submodules, or initialize them before building:

```sh
git submodule update --init --recursive
```

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo build --all-targets --all-features
cargo test --all-features
cargo run -p tinyloops --example simple_loop
cargo run -p tinyloops --example research_loop
cargo run -p tinyloops --features tinyagents --example tinyagents_harness
cargo build -p tinyloops --release --lib   # produces the installable cdylib
```

The first four are exactly what CI runs. Optional extras:

```sh
cargo doc --no-deps --all-features   # CI builds this with RUSTDOCFLAGS="-D warnings"
cargo deny check all                 # supply-chain check; see deny.toml
cargo install cargo-llvm-cov         # once, before running the coverage gate
.github/scripts/check-file-coverage.sh 90 coverage.json
```

## Releasing

Run the **Release** workflow from the Actions tab with a `patch`, `minor`, or
`major` bump. Use `current` only to resume an interrupted release whose version
commit and tag already exist. The workflow revalidates the workspace, versions
and tags it (one `[workspace.package]` version that every member inherits),
builds `crates/tinyloops` as a TinyBus `cdylib`, and creates a GitHub release.

Assets follow `tinyloops-<version>-<platform>.<tar.gz|zip>` and contain the
native module, its SHA-256 `modules.toml`, the license, and
[`MODULE.md`](MODULE.md). Every release also publishes `checksum.toml`, which
TinyBus uses to verify an archive before extraction. The workflow then loads the
published Ubuntu archive through TinyBus's GitHub release API and calls its
`Greet` method before declaring the release successful. TinyBus itself is not
shipped by this repository; the pinned submodule is the build-time SDK.

The stable native matrix covers Ubuntu 22.04 and 24.04 on x86_64 and ARM64;
Fedora 43 and 44 on x86_64 and ARM64; rolling Arch Linux on its officially
supported x86_64 architecture; macOS 15 and 26 on Intel and Apple Silicon;
Windows Server 2022 and 2025 on x86_64; and Windows 11 on ARM64. Preview,
deprecated, and unofficial architecture images are not release gates. Do not
hand-edit the version in the root `Cargo.toml`.

## Documentation

- [`AGENTS.md`](AGENTS.md) for repository guidelines, human and agent alike
- [`ROADMAP.md`](ROADMAP.md) for what is built, what is next, what is not
- [`CONTRIBUTING.md`](CONTRIBUTING.md) for how to propose a change
- [`docs/specs/`](docs/specs/README.md) for behavior and architecture specs
- [`docs/plans/`](docs/plans/README.md) for test-first implementation plans
- [`docs/adr/`](docs/adr/0001-record-architecture-decisions.md) for architecture
  decision records
- [`SECURITY.md`](SECURITY.md) for how to report a vulnerability

## License

GPL-3.0-only. See [LICENSE](LICENSE).
