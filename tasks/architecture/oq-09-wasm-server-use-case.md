---
id: architecture/oq-09-wasm-server-use-case
name: External trigger — a concrete server-side WASM use case (or confirmation it stays a client-side constraint)
status: pending
depends_on: []
scope: single
risk: trivial
impact: component
level: research
tags: [external-trigger, deferred-oq]
---

## Description

External-trigger tracker for [OQ-09](../docs/architecture/questions/009-wasm-target-boundaries.md)
(WASM Target Boundaries). This is **not actionable work** — it tracks whether
a concrete server-side WASM use case has emerged, or whether the project
confirms that WASM compatibility remains a *client-side* design constraint
only (a browser can implement BiStream over WebTransport streams; the
server-side dispatch door is a known, accepted closure per ADR-007/009).

## Trigger condition

Either:
- **A concrete server-side WASM use case arrives** (a deployment that wants
  to run an alknet server peer compiled to WASM, which would require a
  `Connection` trait and a runtime-abstracted accept loop — currently not
  planned), or
- **A deliberate confirmation** that WASM stays a client-side design
  constraint, at which point OQ-09 transitions from `deferred` to `resolved`
  with the accepted-closure framing already in its Resolution text.

The second path is the more likely one — the OQ exists mainly so the
server-side WASM door closure is documented rather than implicit.

## What unblocking looks like

When a decision is made (either direction):

1. Mark this task `status: completed`.
2. Move [OQ-09](../docs/architecture/questions/009-wasm-target-boundaries.md)
   from `deferred` to either `resolved` (client-side-only confirmed) or `open`
   (server-side use case arrives, requiring a Connection trait + WASM runtime
   abstraction ADR).

## Why this is a task, not just an OQ field

OQ-09 predates the formalized `deferred(scope)` + blocking-condition pattern
and lacked a structured `Blocked on:` field (it used the legacy `deferred`
status with the deferral reason in the Resolution prose). This task
formalizes the blocking condition and gives the OQ a machine-readable
presence in the task graph.

## Verification

This task is "completed" when either a server-side WASM use case arrives
(move OQ-09 to `open`) or the client-side-only constraint is confirmed
(move OQ-09 to `resolved`).