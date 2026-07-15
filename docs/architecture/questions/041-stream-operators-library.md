# OQ-41: Stream Operators Library

- **Origin**: [ADR-049](decisions/049-streaming-handler-for-subscriptions.md),
  [operation-registry.md](crates/call/operation-registry.md) §"OperationEnv"
- **Status**: deferred(scope)
- **Door type**: Two-way (additive utility library; no protocol or API-surface
  change)
- **Priority**: low
- **Impacts**: None — handlers produce streams today without it. Would
  reduce boilerplate in stream-transforming handlers when added.
- **Blocked on**: A handler that needs stream operators and finds the existing
  combinators (`Box::pin(stream::iter(...))`, `async_stream::stream!`,
  `futures::stream`) insufficient. The operators library is a convenience, not
  a prerequisite for any handler.
- **Resolution**: ADR-049 establishes that stream composition (filter, map,
  combine, window, dedupe) is a **handler-level concern**, not a protocol
  composition concern. `OperationEnv::invoke()` is request/response-only;
  stream manipulation happens at the handler level with stream operators on
  the `BoxStream<ResponseEnvelope>` the handler itself produces. The
  `@alkdev/pubsub` `operators.ts` is the prior art: 13 operators (`filter`,
  `map`, `take`, `batch`, `dedupe`, `window`, `chain`, `join`, `reduce`,
  `groupBy`, `flat`, `pipe`, `toArray`) that operate on `AsyncIterable<T>`,
  forked from graphql-yoga's subscription implementation.

  The Rust analogue — a stream-operators utility crate or module providing
  the same set of operators on `BoxStream<T>` / `impl Stream<Item = T>` — is
  a feature extension. Handlers can
  produce streams today without it (`Box::pin(stream::iter(...))`,
  `async_stream::stream!`, `futures::stream` combinators all work); the
  operators library is a convenience that reduces boilerplate for handlers
  that transform streams (filter, batch, dedupe, window). No ADR is needed
  for the library itself — it's internal utility code that doesn't cross
  crate boundaries as a contract. An ADR would be warranted only if the
  operators become part of a public API surface (e.g., a handler-registration
  DSL that references operator names).

  This OQ exists so the operators library is tracked and findable, not left
  as inline hedging in the spec docs. It is not a deferral of a decision —
  the architectural decision (stream composition is handler-level, not
  protocol-level) is made in ADR-049. This tracks the *implementation* of
  the utility library, which is scheduling work, not architecture work.
- **Cross-references**: ADR-049,
  [operation-registry.md](crates/call/operation-registry.md) §"OperationEnv",
  `/workspace/@alkdev/pubsub/src/operators.ts` (TS prior art)
