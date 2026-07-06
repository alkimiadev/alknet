---
id: architecture/safe-exit-blocker-task-mechanism
name: Establish the tasks/architecture/ blocker-task half of the Safe Exit protocol
status: completed
depends_on: []
scope: moderate
risk: low
impact: project
level: planning
tags: [convention]
---

## Description

During the open-questions.md decompose (July 2026), I found that the role
spec's Safe Exit protocol says "create a blocker task in `tasks/architecture/`
that names the dependency" when deferring a decision — but that directory has
never existed. The four `deferred(scope)` OQs in the current file (OQ-32,
OQ-41, OQ-44, OQ-46) carry a `Blocked on:` field but no corresponding blocker
task files anywhere in the repo.

So the deferral *field* exists (the visibility half), but the *task* half of the
safe-exit mechanism is unenforced/unused. This task is about establishing the
convention so future deferrals create the corresponding blocker task, and
backfilling the existing deferred OQs.

## Context

The project's broader pattern (per the July 2026 discussion): questions often
lead to decisions which lead to tasks — a graph not unlike how implementation
tasks are modeled as a DAG. The `tasks/architecture/` directory is where the
"this decision is blocked on X" blocker tasks live, so that:

1. A deferred OQ has a visible `Blocked on:` condition in the index (the
   open-questions.md "Deferred / Blocked" section — already in place after the
   decompose).
2. A deferred OQ has a corresponding blocker task that names the dependency and
   can itself be depended on by downstream work that needs the decision
   resolved.

The two halves serve different audiences: the index section is for the
architect ("what's currently parked and why"), the blocker task is for the
implementation agent / planner ("what unblocks this, and what's waiting on
it").

## Work

1. Confirm the task file format (YAML frontmatter: `id`, `name`, `status`,
   `depends_on`, `scope`, `risk`, `impact`, `level`) is the right shape for
   architecture blocker tasks, or adjust if the architecture workflow needs
   different fields (e.g., a `blocks:` field pointing back at the OQ).
2. Backfill blocker tasks for the four existing `deferred(scope)` OQs:
   - OQ-32 (Multi-Hop Federation) — blocked on a concrete multi-hop use case
   - OQ-41 (Stream Operators Library) — blocked on a handler needing operators
     beyond existing combinators
   - OQ-44 (Terminal Modes) — blocked on a concrete mode-control use case
   - OQ-46 (Runner API Surface) — blocked on a concrete runner-policy use case
3. Document the convention (one line in `docs/sdd_process.md` or the architect
   role spec) so future deferrals create the blocker task as part of the Safe
   Exit step.

## Summary

Completed in the July 2026 architecture-task pass. Decisions:

- **Format**: adopted the existing taskgraph frontmatter verbatim — no new
  `blocks:` field. One edge type (`depends_on`) keeps the graph simple; the
  reverse lookup uses `taskgraph dependents`. The OQ's `Blocked on:` text is
  the human-readable pointer; the task `depends_on` is the machine-readable
  edge. They serve different audiences (architect vs. planner/agent).
- **External-trigger tasks**: the four `deferred(scope)` OQs (32, 41, 44, 46)
  plus the two legacy `deferred` OQs (09, 10) each got an external-trigger
  tracker task under `tasks/architecture/` tagged `[external-trigger,
  deferred-oq]`. These represent the external condition (a use case arriving,
  a crate being specced) that would unblock the OQ — they are not actionable
  work, so `risk: trivial` and `level: research`.
- **OQ-09/10 backfill**: added a structured `Blocked on:` field to both
  (previously they used legacy `deferred` status with the reason in the
  Resolution prose). The `open-questions.md` index now surfaces all six
  deferred OQs with concrete blocking conditions in the Deferred/Blocked
  section — no more "_(no explicit blocking condition recorded)_" placeholders.
- **Convention doc**: the `docs/sdd_process.md` Task File Format section is
  updated with the architecture-task level mapping and the Safe Exit
  blocker-task pattern (separate edit).

## Out of scope

- The DB-backed backend (no manual links, vector/text search) — that's a future
  evolution that this file-based pattern informs, not something to build now.
- Re-resolving any of the deferred OQs — this is about the tracking mechanism,
  not the decisions themselves.

## Verification

- `tasks/architecture/` contains one external-trigger task per deferred OQ
  (six total: OQ-09, 10, 32, 41, 44, 46).
- Each OQ's `Blocked on:` field names its tracker task ID, and the
  `open-questions.md` Deferred/Blocked section surfaces the condition inline.
- `taskgraph validate` passes for `tasks/architecture/`.
- `docs/sdd_process.md` references the convention.