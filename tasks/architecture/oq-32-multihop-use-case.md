---
id: architecture/oq-32-multihop-use-case
name: External trigger — a concrete multi-hop federation use case
status: pending
depends_on: []
scope: single
risk: trivial
impact: component
level: research
tags: [external-trigger, deferred-oq]
---

## Description

External-trigger tracker for [OQ-32](../docs/architecture/questions/032-multi-hop-federation.md)
(Multi-Hop Federation). This is **not actionable work** — it tracks whether a
concrete use case for multi-hop federation has arrived. When it does, mark this
task `completed` and the OQ moves from `deferred(scope)` to `open`.

## Trigger condition

A concrete deployment or use case that requires transitive op discovery across
more than one hop — i.e., worker A needs to reach worker B's ops *through* the
head, where the head is not explicitly re-exporting them. The one-hop model
(head→worker, runner→hub) covers all current use cases.

## What unblocking looks like

When a use case arrives:

1. Mark this task `status: completed`.
2. Move [OQ-32](../docs/architecture/questions/032-multi-hop-federation.md)
   from `deferred(scope)` to `open` (update the Status field + the
   `open-questions.md` index tables + Deferred/Blocked section).
3. Create an architecture task to write the multi-hop federation ADR — the
   peer-keyed overlay model extends to multi-hop without redesign (ADR-029 §3.7),
   but path-finding (which peer reaches which op transitively) is where the
   design work lives. A graph library (petgraph) may pay off for multi-hop; for
   one-hop, a nested `HashMap<PeerId, HashMap<String, ...>>` suffices.

## Why this is a task, not just an OQ field

The OQ's `Blocked on:` field in `open-questions.md` is the human-readable
visibility surface ("what's parked and why"). This task is the machine-readable
half: it lives in the task graph so `taskgraph` tools can reason about it, and
so downstream work that depends on multi-hop being resolved can declare
`depends_on: [architecture/oq-32-multihop-use-case]`.

## Verification

This task is "completed" when a concrete multi-hop use case is documented
(e.g., in a research finding or deployment note) and OQ-32 has been moved to
`open`.