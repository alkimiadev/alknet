---
id: architecture/oq-41-stream-operators-use-case
name: External trigger — a handler that needs stream operators beyond existing combinators
status: pending
depends_on: []
scope: single
risk: trivial
impact: component
level: research
tags: [external-trigger, deferred-oq]
---

## Description

External-trigger tracker for [OQ-41](../docs/architecture/questions/041-stream-operators-library.md)
(Stream Operators Library). This is **not actionable work** — it tracks
whether a handler has emerged that needs stream operators (filter, map, batch,
dedupe, window, etc. on `BoxStream<T>`) and finds the existing combinators
insufficient. The operators library is a convenience, not a prerequisite for
any handler.

## Trigger condition

A handler that transforms subscription streams (`BoxStream<ResponseEnvelope>`)
and finds `Box::pin(stream::iter(...))`, `async_stream::stream!`, and
`futures::stream` combinators insufficient — i.e., the handler code is
demonstrably boilerplate-heavy for stream manipulation that the operators
library would collapse. The architectural decision (stream composition is
handler-level, not protocol-level) is already made in ADR-049; this tracks the
*implementation* of the utility library.

## What unblocking looks like

When a handler needs the operators:

1. Mark this task `status: completed`.
2. Move [OQ-41](../docs/architecture/questions/041-stream-operators-library.md)
   from `deferred(scope)` to `resolved` (the architectural decision is already
   made — ADR-049; what remains is the implementation, which is scheduling
   work). The OQ may transition directly to `resolved` rather than `open`,
   since no architecture decision remains.
3. Implement the operators library (no ADR needed — internal utility code
   that doesn't cross crate boundaries as a contract; an ADR would be
   warranted only if the operators become part of a public API surface, e.g.,
   a handler-registration DSL that references operator names).

## Why this is a task, not just an OQ field

The OQ's `Blocked on:` field in `open-questions.md` is the human-readable
visibility surface. This task is the machine-readable half: it lives in the
task graph so `taskgraph` tools can reason about it, and so a handler that
needs the operators can declare `depends_on:
[architecture/oq-41-stream-operators-use-case]`.

## Prior art

`@alkdev/pubsub/src/operators.ts` — 13 operators (`filter`, `map`, `take`,
`batch`, `dedupe`, `window`, `chain`, `join`, `reduce`, `groupBy`, `flat`,
`pipe`, `toArray`) on `AsyncIterable<T>`, forked from graphql-yoga's
subscription implementation. The Rust analogue would provide the same set on
`BoxStream<T>` / `impl Stream<Item = T>`.

## Verification

This task is "completed" when a handler is identified that needs the operators
and OQ-41 has been moved to `resolved`.