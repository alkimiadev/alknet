---
id: architecture/oq-46-runner-policy-use-case
name: External trigger — a concrete runner-policy use case that forces the API surface
status: pending
depends_on: []
scope: single
risk: trivial
impact: component
level: research
tags: [external-trigger, deferred-oq]
---

## Description

External-trigger tracker for [OQ-46](../docs/architecture/questions/046-runner-api-surface.md)
(Runner API Surface). This is **not actionable work** — it tracks whether a
concrete runner-policy use case has emerged that forces the API surface (job
management, log persistence, task graph integration). The runner *mechanism*
(pipe mode) is already in alknet-tty (ADR-054); the runner *policy* is a
downstream crate's job.

## Trigger condition

A concrete deployment that needs runner *policy* — job management, log
persistence, task graph integration — on top of the pipe-mode mechanism
(`TtyParams.terminal = None` → `std::process::Command` with piped stdio →
framed byte stream + exit code) that alknet-tty already provides.

## What unblocking looks like

When a runner-policy use case arrives:

1. Mark this task `status: completed`.
2. Move [OQ-46](../docs/architecture/questions/046-runner-api-surface.md)
   from `deferred(scope)` to `open`.
3. Decide whether a runner-policy crate (e.g., an `alknet-runner` crate that
   builds on the pipe mode + the wire format to provide job management) is
   needed, and what its API surface would be. The mechanism is preserved
   regardless of the policy decision.

## Why this is a task, not just an OQ field

The OQ's `Blocked on:` field in `open-questions.md` is the human-readable
visibility surface. This task is the machine-readable half: it lives in the
task graph so `taskgraph` tools can reason about it, and so downstream work
that depends on runner policy can declare `depends_on:
[architecture/oq-46-runner-policy-use-case]`.

## Verification

This task is "completed" when a concrete runner-policy use case is documented
and OQ-46 has been moved to `open`.