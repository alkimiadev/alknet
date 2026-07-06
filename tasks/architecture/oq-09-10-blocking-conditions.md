---
id: architecture/oq-09-10-blocking-conditions
name: Add explicit Blocked on conditions to OQ-09 (WASM) and OQ-10 (Git Adapter)
status: completed
depends_on: []
scope: narrow
risk: low
impact: component
level: decomposition
tags: [convention]
---

## Description

During the open-questions.md decompose (July 2026), I found that OQ-09 (WASM
Target Boundaries) and OQ-10 (Git Adapter Scope) use the legacy `deferred`
status without a structured `Blocked on:` field. They predate the formalized
`deferred(scope)` + blocking-condition pattern established by ADR-009's Safe
Exit protocol.

The four newer deferred OQs (OQ-32, OQ-41, OQ-44, OQ-46) all carry an explicit
`Blocked on:` condition. OQ-09 and OQ-10 do not — their deferral reason lives
in the Resolution prose, not in a structured field.

This matters because the new `open-questions.md` index has a "Deferred / Blocked"
section that surfaces `Blocked on:` inline as the safe-exit visibility surface.
OQ-09 and OQ-10 currently render as "_(no explicit blocking condition recorded
— see full file)_" in that section, which weakens the visibility guarantee.

## Work

Either:
- Add a `Blocked on:` field to each (e.g., OQ-09: "blocked on: a concrete
  server-side WASM use case — currently a design constraint, not a
  deliverable"; OQ-10: "blocked on: speccing alknet-git — resolve when that
  crate is specified, not deferred past it"), or
- Reframe them as `deferred(scope)` if the original `deferred` status was
  imprecise, and confirm the blocking condition from the Resolution text.

The decision content stays unchanged — this is a metadata-structure fix, not a
re-resolution.

## Summary

Completed alongside `architecture/safe-exit-blocker-task-mechanism`. Added a
structured `Blocked on:` field to both OQ-09 and OQ-10 in their per-OQ files,
each pointing at its new external-trigger tracker task
(`architecture/oq-09-wasm-server-use-case`,
`architecture/oq-10-git-adapter-spec`). Kept the legacy `deferred` status
rather than reframing to `deferred(scope)` — the distinction is no longer
load-bearing now that both have explicit blocking conditions and tracker
tasks. The `open-questions.md` Deferred/Blocked section now surfaces all six
deferred OQs with concrete conditions inline — no placeholders remain.

## Verification

The "Deferred / Blocked" section of `docs/architecture/open-questions.md`
shows a concrete blocking condition for OQ-09 and OQ-10 instead of the "no
explicit blocking condition recorded" placeholder.