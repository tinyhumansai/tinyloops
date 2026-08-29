# Documentation

This directory holds documentation that does not belong in rustdoc: the shape
of the system, the reasoning behind it, and the constraints a reader needs
before touching the code. API reference lives in doc comments next to the code,
where it cannot drift.

## Layout

```text
docs/
├── README.md      # this index
├── specs/         # behavior and architecture specifications
├── plans/         # implementation plans derived from approved specs
└── adr/           # architecture decision records, numbered and immutable
```

- **[`specs/`](specs/README.md)** — one file per feature, module, or subsystem,
  describing its behavior, public surface, invariants, and acceptance criteria.
- **[`plans/`](plans/README.md)** — implementation-ordered, test-first steps for
  delivering an approved specification. Plans name exact files and verification
  commands, and are updated as the work progresses.
- **`adr/`** — a dated record per significant decision. Use
  [`adr/0001-record-architecture-decisions.md`](adr/0001-record-architecture-decisions.md)
  as the template. An accepted ADR is not edited; it is superseded by a later
  one.

Complex modules also carry a module-level `README.md` inside `src/<module>/`
covering their design, public surface, and important constraints.

## The framework

The loop framework is specified across seven files, and `AGENTS.md` carries the
same list for readers who start there. Read them in this order: the shape
([`specs/loop-kernel.md`](specs/loop-kernel.md)), what drives it
([`specs/orchestrator.md`](specs/orchestrator.md)), how a pass chooses the next
one ([`specs/routing-and-policy.md`](specs/routing-and-policy.md)), then the
four seams and their supporting contracts
([`specs/seams.md`](specs/seams.md),
[`specs/workspace-and-ledgers.md`](specs/workspace-and-ledgers.md),
[`specs/budget.md`](specs/budget.md),
[`specs/observability.md`](specs/observability.md)).

[`specs/prior-art.md`](specs/prior-art.md) sits beside them rather than in the
sequence. It holds the evidence each default rests on, numbered `C-1` … `C-19`
and pointing at the specification that implements each one, so a constant can be
changed by someone who can see what it was chosen against. Its sources, with
every unverified figure marked as such, are in
[`specs/prior-art-bibliography.md`](specs/prior-art-bibliography.md).

The module-release contract is separate, in
[`specs/tinybus-module-release.md`](specs/tinybus-module-release.md), with its
implementation sequence in
[`plans/tinybus-module-release.md`](plans/tinybus-module-release.md).

## Conventions

- Keep every Markdown file at 500 lines or fewer. When a topic outgrows that,
  split it into focused files and link them from the nearest `README.md`.
- Update documentation in the same commit as the behavior it describes.
- Prefer a concrete example over an abstract description.
- Link between documents rather than duplicating content; one fact lives in one
  place.
- Write a specification before a plan: the spec defines the outcome and
  constraints, while the plan defines the implementation sequence.
