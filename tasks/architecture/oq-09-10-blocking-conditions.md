---
id: architecture/oq-09-10-blocking-conditions
name: Add explicit Blocked on conditions to OQ-09 (WASM) and OQ-10 (Git Adapter)
status: pending
depends_on: []
scope: small
risk: low
impact: local
level: architecture
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

## Verification

The "Deferred / Blocked" section of `docs/architecture/open-questions.md` should
show a concrete blocking condition for OQ-09 and OQ-10 instead of the "no
explicit blocking condition recorded" placeholder.