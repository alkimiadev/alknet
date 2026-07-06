---
id: architecture/oq-10-git-adapter-spec
name: External trigger — speccing alknet-git (resolve OQ-10 when that crate is specified, not deferred past it)
status: pending
depends_on: []
scope: single
risk: trivial
impact: component
level: research
tags: [external-trigger, deferred-oq]
---

## Description

External-trigger tracker for [OQ-10](../docs/architecture/questions/010-git-adapter-scope-smart-protocol-only-or-full-server.md)
(Git Adapter Scope — Smart Protocol Only or Full Server?). This is **not
actionable work** — it tracks when the alknet-git crate is being specified,
at which point OQ-10 must be resolved (not deferred past it).

## Trigger condition

The alknet-git crate is being specified. The OQ's Resolution text already
states: "Resolve this when speccing alknet-git, not deferred past it." The
two sub-questions:

1. **Git adapter scope** — start with git smart protocol over QUIC streams;
   ERC721 integration and full server capabilities are additive.
2. **Composability fork** — whether git operations are registered in the
   `OperationRegistry` and callable via `env.invoke()`, or only available as
   raw smart protocol on `alknet/git`. The path of least resistance (raw
   smart protocol only) forecloses agent composition of git operations; to
   make git composable, a call-protocol projection (a set of
   `HandlerRegistration` bundles wrapping git operations behind the
   registry) must be built alongside or instead of the raw handler.

## What unblocking looks like

When alknet-git is specced:

1. Mark this task `status: completed`.
2. Move [OQ-10](../docs/architecture/questions/010-git-adapter-scope-smart-protocol-only-or-full-server.md)
   from `deferred` to `open`, then resolve it as part of the alknet-git spec
   pass (the Resolution text is explicit: do not defer past the spec).

## Why this is a task, not just an OQ field

OQ-10 predates the formalized `deferred(scope)` + blocking-condition pattern
and lacked a structured `Blocked on:` field (it used the legacy `deferred`
status with the deferral reason in the Resolution prose). This task
formalizes the blocking condition and gives the OQ a machine-readable
presence in the task graph.

## Verification

This task is "completed" when alknet-git is being specced and OQ-10 has been
moved to `open` (then resolved as part of that spec pass).