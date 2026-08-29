# TinyLoops

A loop engineering framework: the shape of one goal run — attempt, evaluate,
route, budget — for agentic loops that run a workflow, judge what came back, and
decide whether to go round again.

It is the middle of three layers, and the split is what keeps each of them
small:

- **[TinyFlows](https://github.com/tinyhumansai/tinyflows)** is the engine. It
  executes one graph run and decides nothing. A loop's unit of work is a
  `WorkflowGraph` it compiles once and runs per turn. Every effect — models,
  tools, HTTP, code execution, persistence — goes through a capability trait, so
  a loop chooses its own vendors, and the examples here run offline against the
  in-memory mocks.
- **TinyLoops** — this repository — is one goal run: attempt, evaluate, route,
  budget, and the orchestrator, roles, tools, memory, workspace, and
  observability that a run needs to do that honestly.
- **[`tinyflows-adaptive`](vendor/tinyflows/crates/adaptive)** is what spans
  runs: ledger rows, scored lessons, workflow selection and authoring,
  promotion. Its stated rule draws the line this repository works to — the
  engine may know about one run, anything that spans runs lives there.

Two more dependencies are vendored rather than reimplemented:

- [**TinyAgents**](https://github.com/tinyhumansai/tinyagents) is the optional
  durable agent harness. Once a loop needs to be paused, checkpointed, resumed,
  or observed, the `while` statement stops being enough and the control flow
  moves into a harness graph. It is behind the `tinyagents` cargo feature so a
  shipped module never resolves it.
- [**TinyBus**](https://github.com/tinyhumansai/tinybus) is how a loop is
  reached from outside the process. The workspace builds as both an `rlib` and
  the `cdylib` TinyBus loads.

It is a two-crate cargo workspace. `crates/tinyloops-bus` is the wire contract —
member names, payload types, and the contract version, with no transport and no
behavior — and `crates/tinyloops` is the implementation. A host that only makes
calls depends on the contract crate alone and compiles neither the module nor
`tinybus` itself.

## The Loop, In Miniature

Compile the graph once, run it per turn, feed each result back in, and stop on a
judge or a budget. That is the whole shape, and it is the complete
[`simple_loop`](crates/tinyloops/examples/simple_loop.rs) example minus its graph
definition:

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

Swap `refine_step()` for a graph that calls a model and `score` for a real
judge, and nothing around them changes. A hard turn budget is part of the
template rather than something a caller remembers to add — a loop without one is
a way to spend an afternoon discovering that a judge never says yes.

When the loop needs durability, the same two decisions become harness nodes and
the `while` statement disappears:

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
cargo run -p tinyloops --features tinyagents --example tinyagents_harness # the loop, under a harness
cargo run -p tinyloops --example basic                                    # ordinary library API usage
```

Both loop examples run against TinyFlows' mock capabilities, so they are
deterministic, offline, and need no provider credentials. `tinyagents` is an
optional dependency: the harness example declares `required-features`, so a
default build skips it rather than failing to compile.

## What You Get

| Area | What is configured |
| --- | --- |
| Layout | A cargo workspace under `crates/`, split into a dependency-light wire contract and the module that implements it; directory modules with `mod.rs` / `types.rs` / `test.rs`, a crate-wide error type, integration tests, and a runnable example |
| Lints | `unsafe_code` forbidden, `missing_docs`, clippy `all` + `pedantic`, no `unwrap`/`expect`/`panic`/`todo` in library code — all declared once in `[workspace.lints]` so every crate, local run, and CI run agree |
| CI | Format, clippy, build, test (default and all features), a run of the bundled example, an assertion that the contract crate stays transport-free, at least 90% line coverage in every source file, rustdoc with `-D warnings`, an MSRV build, and a `cargo-deny` supply-chain check |
| Release | Manual `workflow_dispatch` bump that validates, versions, tags, and creates installable native module packages for every supported platform |
| Community | Issue and pull request templates, Dependabot, contributing, security, support, and code of conduct docs |
| Agents | [`AGENTS.md`](AGENTS.md) as the single source of truth, symlinked as `CLAUDE.md`, plus a `.claude/settings.json` allowlist for the standard commands |
| Vendor | TinyBus host types and module SDK, the TinyFlows workflow engine, and the TinyAgents harness, each pinned as a `vendor/` build-time submodule |

## Layout

```text
Cargo.toml              # virtual workspace: members, shared metadata, lints
crates/
├── tinyloops-bus/       # the wire contract — what crosses the bus
│   ├── README.md       # why the contract is its own crate
│   └── src/
│       ├── lib.rs      # crate docs + the entire public re-export surface
│       ├── names/      # interface, object path, one constant per member
│       ├── greeting/   # payload types, one directory per family
│       │   ├── mod.rs
│       │   ├── types.rs
│       │   └── test.rs
│       └── version/    # contract version and the host bind rule
└── tinyloops/           # the module — behavior, adapter, and the cdylib
    ├── src/
    │   ├── lib.rs      # crate docs + public surface, re-exporting the contract
    │   ├── error/      # crate-wide `Error` and `Result<T>`
    │   ├── greeting/   # one directory per feature area
    │   └── tinybus_module/   # bus interface, setup, and ABI v1 exports
    ├── tests/
    │   └── public_api.rs     # integration tests against the public API only
    └── examples/
        ├── simple_loop.rs            # the loop template, in plain Rust
        ├── tinyagents_harness.rs     # the same loop under a durable harness
        ├── basic.rs                  # ordinary library API usage
        ├── verify_module.rs          # local dynamic-module verification
        └── verify_github_release.rs  # tagged-release download and bus call
vendor/
├── tinybus/            # pinned TinyBus git submodule — host types, module SDK
├── tinyflows/          # pinned workflow engine and its adaptive loop
└── tinyagents/         # pinned durable agent + graph harness
docs/
├── README.md           # documentation index and conventions
├── specs/              # behavior and architecture specifications
├── plans/              # implementation-ordered delivery plans
└── adr/                # immutable architecture decision records
```

The split is the point. A payload type describes what a frame carries; the
behavior that answers it is a different obligation. `tinyloops` depends on
`tinyloops-bus` and re-exports all of it, so `tinyloops::GreetRequest` and
`tinyloops_bus::GreetRequest` are the *same* type rather than structural twins,
and a host is never forced to choose between linking the whole module and
redefining the vocabulary. See
[`crates/tinyloops-bus/README.md`](crates/tinyloops-bus/README.md).

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
cargo run -p tinyloops --features tinyagents --example tinyagents_harness
cargo build -p tinyloops --release --lib   # produces the installable cdylib
```

Those four checks are exactly what CI runs. Optional extras:

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
and tags it — one `[workspace.package]` version that every member inherits —
builds `crates/tinyloops` as a TinyBus `cdylib`, and creates a GitHub release.
Assets follow `tinyloops-<version>-<platform>.<tar.gz|zip>` and contain the
native module, its SHA-256 `modules.toml`, license, and
[`MODULE.md`](MODULE.md). Every release also publishes `checksum.toml`, which
TinyBus uses to verify an archive before extraction. The workflow loads the
published Ubuntu archive through TinyBus's GitHub release API and calls its
`Greet` method before declaring the release successful. TinyBus itself is not
shipped by this repository; the pinned submodule is the build-time SDK. The stable native
matrix covers Ubuntu 22.04 and 24.04 on x86_64 and ARM64; Fedora 43 and 44 on
x86_64 and ARM64; rolling Arch Linux on its officially supported x86_64
architecture; macOS 15 and 26 on Intel and Apple Silicon; Windows Server 2022
and 2025 on x86_64; and Windows 11 on ARM64. Preview, deprecated, and unofficial
architecture images are not release gates. Do not hand-edit the version in the
root `Cargo.toml`.

## Documentation

- [`AGENTS.md`](AGENTS.md) — repository guidelines for humans and agents
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — how to propose a change
- [`docs/specs/`](docs/specs/README.md) — behavior and architecture specs
- [`docs/plans/`](docs/plans/README.md) — test-first implementation plans
- [`docs/adr/`](docs/adr/0001-record-architecture-decisions.md) — architecture
  decision records
- [`SECURITY.md`](SECURITY.md) — how to report a vulnerability

## License

GPL-3.0-only. See [LICENSE](LICENSE).
