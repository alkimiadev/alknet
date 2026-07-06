---
id: architecture/oq-44-tty-modes-use-case
name: External trigger — a concrete TTY mode-control use case
status: pending
depends_on: []
scope: single
risk: trivial
impact: component
level: research
tags: [external-trigger, deferred-oq]
---

## Description

External-trigger tracker for [OQ-44](../docs/architecture/questions/044-terminal-modes-tty-modes.md)
(Terminal Modes / TTY modes). This is **not actionable work** — it tracks
whether a concrete deployment has emerged that needs to set TTY modes (echo,
raw, canonical, etc.) on a PTY beyond the backend's defaults.

## Trigger condition

A concrete deployment that needs to control TTY modes beyond the defaults the
backends already provide (`portable_pty`, docker `tty: true`, russh
`pty_request` all have defaults that work for the common terminal case). The
`modes` field in `TerminalParams` is `serde_json::Value` (reserved as `{}` in
v1) for when this arrives.

## What unblocking looks like

When a mode-control use case arrives:

1. Mark this task `status: completed`.
2. Move [OQ-44](../docs/architecture/questions/044-terminal-modes-tty-modes.md)
   from `deferred(scope)` to `open`.
3. Specify the `modes` JSON shape (SSH's `pty_request` carries TTY modes as a
   packed bitmask; the Rust analogue extends the `modes` field). Adding mode
   control is additive (extend the `modes` JSON shape) and does not break
   downstream — the architectural commitment is two-way-door.

## Why this is a task, not just an OQ field

The OQ's `Blocked on:` field in `open-questions.md` is the human-readable
visibility surface. This task is the machine-readable half: it lives in the
task graph so `taskgraph` tools can reason about it, and so downstream work
that depends on TTY mode control can declare `depends_on:
[architecture/oq-44-tty-modes-use-case]`.

## Verification

This task is "completed" when a concrete mode-control use case is documented
and OQ-44 has been moved to `open`.