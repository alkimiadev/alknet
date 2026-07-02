# ADR-049: Streaming Handler for Subscription Operations

## Status

Accepted

## Context

The call protocol defines `Subscription` as a first-class operation type
(ADR-012 lists `subscribe` as one of four top-level protocol operations;
`OperationSpec.op_type` includes `Subscription`). The wire protocol supports
streaming: five event types (`call.requested`, `call.responded`,
`call.completed`, `call.aborted`, `call.error`), `PendingRequestMap::Subscribe`
with an mpsc channel, `CallConnection::subscribe()` returning
`impl Stream<Item = ResponseEnvelope>`, and a full streaming-subscribe example
in `call-protocol.md`. The **client side** works — a client can subscribe to a
remote stream and consume `call.responded` events until `call.completed`.

The **server side does not.** The `Handler` type in `alknet-call` is:

```rust
pub type Handler = Arc<
    dyn Fn(Value, OperationContext) -> Pin<Box<dyn Future<Output = ResponseEnvelope> + Send>>
        + Send + Sync,
>;
```

It returns a single `ResponseEnvelope`. `OperationRegistry::invoke()` returns
one `ResponseEnvelope` and closes. `Dispatcher::handle_stream` calls
`dispatch_requested` → `registry.invoke()` → writes one `EventEnvelope` frame →
loops. A `Subscription` operation that should produce a *stream* of
`call.responded` events followed by `call.completed` has no way to express that
through this handler signature.

This is a **spec gap that should not have shipped.** The TypeScript predecessor
(`@alkdev/operations`, from which the Rust port was derived) had two distinct
handler types:

```typescript
type OperationHandler<I, O, C> = (input: I, context: C) => Promise<O> | O;
type SubscriptionHandler<I, O, C> = (input: I, context: C) => AsyncGenerator<O, void, unknown>;
```

The TS registry (`registry.ts:21`) stored them as a union, validated at
registration that `SUBSCRIPTION` ops get an `AsyncGeneratorFunction`, and the
server-side dispatch (`call.ts:341-349`, `buildCallHandler`) branched on
`op_type`: `SUBSCRIPTION` → iterate the async generator, `respond()` for each,
then `complete()`; else → `execute()`, single `respond()`. The Rust port
collapsed the union into a single `Handler` returning one `ResponseEnvelope`,
losing the streaming path. The fix is to restore it.

The downstream consequences of the gap:

- **`/subscribe` HTTP endpoint** (`GatewayDispatch::invoke()` →
  `subscribe_handler`) wraps a single `ResponseEnvelope` in a one-event SSE
  stream. A real `Subscription` operation (e.g., `agent/chat` streaming LLM
  tokens) cannot stream through it.
- **`from_call` forwarding** for a `Subscription` op calls
  `CallConnection::call_with_payload()` (single response), not
  `CallConnection::subscribe()` (stream). A `from_call`-imported subscription
  truncates to the first value.
- **`from_openapi` forwarding** for a `text/event-stream` response returns one
  `ResponseEnvelope` instead of streaming the SSE chunks.

All three are symptoms of the same root cause: the `Handler` type cannot
produce a stream.

## Decision

### 1. `StreamingHandler` type (alongside `Handler`)

Add a streaming handler type that returns a stream of `ResponseEnvelope`s,
mirroring the TS `SubscriptionHandler` / `OperationHandler` split:

```rust
pub type StreamingHandler = Arc<
    dyn Fn(Value, OperationContext)
        -> Pin<Box<dyn Stream<Item = ResponseEnvelope> + Send>>
        + Send + Sync,
>;
```

Each `Ok(value)` in the stream becomes a `call.responded` event. An `Err`
becomes a `call.error` event (terminal — the stream ends). Natural stream end
becomes `call.completed`. The dispatch path converts each `ResponseEnvelope` to
`EventEnvelope` exactly as it does today for the single-response case — no new
wire-format concept is introduced.

A `make_streaming_handler()` helper (analogue of `make_handler()`) wraps an
async generator / stream-producing closure into a `StreamingHandler`.

### 2. `HandlerKind` enum on `HandlerRegistration`

```rust
pub enum HandlerKind {
    Once(Handler),
    Stream(StreamingHandler),
}

pub struct HandlerRegistration {
    pub spec: OperationSpec,
    pub handler: HandlerKind,  // validated against spec.op_type at registration
    pub provenance: OperationProvenance,
    pub composition_authority: Option<CompositionAuthority>,
    pub scoped_env: Option<ScopedPeerEnv>,
    pub capabilities: Capabilities,
}
```

Registration validates: `Query` / `Mutation` → `HandlerKind::Once`;
`Subscription` → `HandlerKind::Stream`. Mismatch is a startup error (same as
the TS `validateSubscriptionHandler`). The enum makes the "one or the other,
matching `op_type`" invariant type-level rather than two `Option`s validated at
runtime.

### 3. `OperationRegistry::invoke_streaming()`

```rust
impl OperationRegistry {
    /// Dispatch a Subscription operation. Returns a stream of
    /// ResponseEnvelopes. Errors (not-found, forbidden, invalid operation
    /// type) yield a single ResponseEnvelope::error and end the stream.
    pub fn invoke_streaming(
        &self,
        name: &str,
        input: Value,
        context: OperationContext,
    ) -> BoxStream<ResponseEnvelope>;
}
```

`invoke_streaming()` performs the same visibility + ACL checks as `invoke()`,
then dispatches to the `StreamingHandler`. Pre-handler errors (not-found,
forbidden) produce a single error `ResponseEnvelope` and end the stream
(matching the single-response path's behavior, just on a stream).

### 4. `OperationRegistry::invoke()` errors on `Subscription`

`invoke()` is the request/response dispatch path. Calling it on a
`Subscription` op is a type mismatch — a streaming operation dispatched through
the request/response path. It returns a `ResponseEnvelope` carrying
`CallError { code: "INVALID_OPERATION_TYPE", ... }` (a new protocol-level
code):

```
INVALID_OPERATION_TYPE
```

(`retryable: false`, `details: None`). This is the wire-format addition: a
sixth protocol-level error code. It signals "you called the wrong dispatch
method for this operation's type" — distinct from `INVALID_INPUT` (schema
mismatch) and `INTERNAL` (handler failure). Clients should treat unknown codes
as `INTERNAL` with `retryable: false` (the existing rule); `INVALID_OPERATION_
TYPE` is a permanent client-side programming error, not a transient failure.

### 5. `OperationEnv::invoke()` errors on `Subscription`

`OperationEnv::invoke()` (composition) stays request/response-only. It returns
a single `ResponseEnvelope`. Calling it on a `Subscription` op produces the
same `INVALID_OPERATION_TYPE` error — composition cannot truncate a stream to
its first value. This is a clean architectural boundary, not a deferral:

- **`OperationEnv` composition** is "call a child operation, get a result"
  (the `OperationHandler` model). It is request/response by construction.
- **Stream composition** (filter, map, combine, window, dedupe) is a
  handler-level concern. A handler that produces a stream transforms it with
  stream operators at the handler level, not through `OperationEnv`. The
  `@alkdev/pubsub` `operators.ts` is the prior art for this model: 13 operators
  (`filter`, `map`, `take`, `batch`, `dedupe`, `window`, `chain`, `join`, etc.)
  that operate on `AsyncIterable<T>`, distinct from the request/response
  composition. In Rust, the analogues operate on `BoxStream<T>`.
- No `invoke_streaming()` is added to `OperationEnv`. The protocol composition
  surface is request/response; stream manipulation is handler-internal.

### 6. Server-side dispatch branches on `op_type`

`Dispatcher::handle_stream` / `dispatch_requested` gains a branch on
`op_type`:

- `Subscription` → `registry.invoke_streaming()` → for each `ResponseEnvelope`
  in the stream, write `EventEnvelope` to the wire → write `call.completed` on
  stream end.
- `Query` / `Mutation` → `registry.invoke()` → write one `EventEnvelope`
  (existing path, unchanged).

The streaming branch sets `deadline: None` for subscriptions (unbounded —
already specced in `call-protocol.md` Timeouts) and wires abort cascade
(ADR-016): if `call.aborted` arrives for a streaming request, the stream is
dropped (Rust `Drop` releases the handler's resources).

### 7. `GatewayDispatch::invoke_streaming()` (alknet-http)

The shared dispatch spine gains a streaming variant:

```rust
impl GatewayDispatch {
    pub async fn invoke_streaming(
        &self,
        identity: Option<Identity>,
        op: &str,
        input: Value,
    ) -> BoxStream<ResponseEnvelope>;
}
```

`invoke_streaming()` builds the root `OperationContext` identically to
`invoke()` (same security invariants: `internal: false`, `forwarded_for:
None`, same capabilities, same `scoped_env`), then calls
`registry.invoke_streaming()`. The two gateways (`to_openapi`, `to_mcp`)
diverge only on wire-framing; the security axis is provably identical between
`invoke()` and `invoke_streaming()`.

The HTTP `/subscribe` handler calls `invoke_streaming()` and pipes the
`BoxStream<ResponseEnvelope>` to SSE: each `Ok(value)` → SSE `data:` frame,
`Err` → SSE error event + close, stream end → close. This replaces the current
one-event `subscribe_stream_from_envelope` with the real streaming path.

### 8. `from_call` stream forwarding

The `from_call` forwarding handler construction branches on `op_type` during
discovery:

- `Query` / `Mutation` → existing `make_forwarding_handler()` (calls
  `CallConnection::call_with_payload()`, returns single `ResponseEnvelope`),
  registered as `HandlerKind::Once`.
- `Subscription` → new `make_streaming_forwarding_handler()` (calls
  `CallConnection::subscribe()`, returns `impl Stream<Item =
  ResponseEnvelope>`, maps to `BoxStream<ResponseEnvelope>`), registered as
  `HandlerKind::Stream`.

A `from_call`-imported `Subscription` op forwards the remote stream end-to-end:
the client-side `CallConnection::subscribe()` (already working) feeds a
`StreamingHandler` that produces the stream. No truncation, no first-value
fallback.

### 9. `from_openapi` SSE forwarding

The `from_openapi` forwarding handler construction branches on `op_type`
(determined by `detectOperationType` — `text/event-stream` response →
`Subscription`):

- `Query` / `Mutation` → existing forwarding handler (single HTTP request →
  single `ResponseEnvelope`), `HandlerKind::Once`.
- `Subscription` → streaming forwarding handler (HTTP request → SSE response
  stream → parse SSE chunks → `BoxStream<ResponseEnvelope>`), `HandlerKind::
  Stream`.

The SSE parsing reuses the TS `parseSSEFrames` pattern: each SSE `data:` frame
becomes a `ResponseEnvelope::ok()`, SSE stream end becomes stream end (→
`call.completed`).

## Consequences

**Positive:**

- `Subscription` operations work end-to-end: server-side handler →
  server-side dispatch → wire → HTTP `/subscribe` SSE → `from_call`
  forwarding → `from_openapi` SSE forwarding. No truncation, no broken paths.
- The `Handler` / `StreamingHandler` split mirrors the TS prior art exactly,
  making the Rust port faithful to its source.
- `HandlerKind` makes the "one or the other, matching `op_type`" invariant
  type-level (a `Once` variant for `Query`/`Mutation`, a `Stream` variant for
  `Subscription`) rather than a runtime check on two `Option`s.
- Existing handlers (echo, discovery, from_openapi Query/Mutation, from_mcp,
  from_call Query/Mutation) are unchanged — they return a single
  `ResponseEnvelope` and register as `HandlerKind::Once`. The streaming path
  is additive to the existing handler surface.
- `OperationEnv` composition stays request/response, preserving the
  composition model's simplicity. Stream composition is a handler-level
  concern, cleanly separated.
- The new `INVALID_OPERATION_TYPE` protocol code catches dispatch-path misuse
  (calling `invoke()` on a `Subscription`) at the protocol level instead of
  silently producing wrong behavior.

**Negative:**

- `HandlerRegistration.handler` changes type from `Handler` to `HandlerKind`.
  Existing code constructing `HandlerRegistration` bundles must wrap in
  `HandlerKind::Once(...)`. This is a mechanical change across handler
  construction sites (the builder's `.with_local()` / `.with_leaf()` /
  `.with()` methods absorb the wrapping internally, so most assembly-layer
  code is unaffected; direct `HandlerRegistration::new()` calls need the
  wrap).
- A new protocol-level error code (`INVALID_OPERATION_TYPE`) is a wire-format
  addition. Existing clients that treat unknown codes as `INTERNAL` with
  `retryable: false` (the existing rule) handle it correctly — they just
  don't distinguish it from `INTERNAL` until updated. The code is distinct
  from all existing codes and from operation-level domain codes (no
  `HTTP_` prefix, no collision with the five existing protocol codes).
- The `Dispatcher::handle_stream` streaming branch adds a stream-to-wire
  pump (read stream → write frames → write `call.completed`). This is new
  code in the hot dispatch path, but it is a straightforward `while let
  Some(envelope) = stream.next().await` loop, not a complex abstraction.

## Door type

**One-way.** The `Handler` / `StreamingHandler` / `HandlerKind` API surface
is what handlers are written against across crates (`alknet-call`,
`alknet-http`, downstream consumers). Changing it after handlers exist is a
rewrite. The `INVALID_OPERATION_TYPE` wire code is also one-way — once
emitted, clients may handle it, and removing it would break those handlers.

The `HandlerKind` enum shape (`Once(Handler) | Stream(StreamingHandler)`) is
the one-way commitment: two handler variants, validated against `op_type`.
The concrete `BoxStream` library choice (`futures::stream::BoxStream` vs a
custom type) is a two-way-door implementation detail within the one-way
decision.

## References

- ADR-012: Call Protocol Stream Model (defines `subscribe` as a top-level
  protocol operation; the streaming path this ADR implements)
- ADR-017: Call Protocol Client and Adapter Contract (the adapter contract
  this ADR extends with the `StreamingHandler` variant)
- ADR-022: Handler Registration, Provenance, and Composition Authority
  (`HandlerRegistration` gains `HandlerKind`)
- ADR-015: Privilege Model and Authority Context (visibility/ACL checks run
  identically in `invoke_streaming()` as in `invoke()`)
- ADR-016: Abort Cascade for Nested Calls (stream drop on abort; cascade
  through streaming handlers)
- ADR-023: Operation Error Schemas (`INVALID_OPERATION_TYPE` is a
  protocol-level code, distinct from operation-level domain codes)
- ADR-029: Peer-Graph Routing Model (`from_call` forwarding handlers gain the
  streaming variant)
- ADR-009: One-Way Door Decision Framework (the `Handler` / `StreamingHandler`
  split is a one-way door — handler API surface)
- `@alkdev/operations/src/types.ts:62-78` — TS prior art
  (`OperationHandler` / `SubscriptionHandler` split)
- `@alkdev/operations/src/registry.ts:65-75` — TS prior art
  (`validateSubscriptionHandler` — runtime validation against `op_type`)
- `@alkdev/operations/src/call.ts:341-349` — TS prior art (`buildCallHandler`
  branches on `op_type`: `SUBSCRIPTION` → iterate + complete; else → execute)
- `@alkdev/pubsub/src/operators.ts` — stream operators prior art (filter,
  map, batch, dedupe, window, chain, join — handler-level stream
  composition, distinct from `OperationEnv` request/response composition)
- Spec documents amended: `operation-registry.md`, `call-protocol.md`,
  `http-server.md`, `http-adapters.md`, `http-mcp.md`, `client-and-adapters.md`