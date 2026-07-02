---
id: call/registry/streaming-handler-handlerkind
name: Introduce StreamingHandler, HandlerKind, ResponseStream types and migrate HandlerRegistration to HandlerKind
status: completed
depends_on: []
scope: broad
risk: medium
impact: component
level: implementation
---

## Description

ADR-049 restores the streaming handler path that the Rust port dropped when it
collapsed the TS `OperationHandler` / `SubscriptionHandler` union into a single
`Handler`. This task introduces the new types (`StreamingHandler`, `HandlerKind`,
`ResponseStream`, `make_streaming_handler`), adds the `INVALID_OPERATION_TYPE`
protocol error code, changes `HandlerRegistration.handler` from `Handler` to
`HandlerKind`, updates the builder to absorb the wrapping, adds registration-time
validation, updates `invoke()` to error on `Stream`, updates the overlay env to
match on `HandlerKind`, and migrates **every existing construction site** to wrap
in `HandlerKind::Once`.

This is the foundational breaking change — all downstream streaming tasks depend
on it. It is broad in surface area (touches `registration.rs`, `wire.rs`,
`connection.rs`, and every test/adapter that constructs a `HandlerRegistration`)
but each individual change is mechanical. The goal: after this task, the codebase
compiles with two handler kinds, `Query`/`Mutation` ops work exactly as before
(wrapped in `HandlerKind::Once`), and `Subscription` ops are rejected by `invoke()`
with `INVALID_OPERATION_TYPE` (the streaming dispatch path `invoke_streaming()`
is added in `call/registry/invoke-streaming`).

### New types (registration.rs)

```rust
use futures::stream::Stream;

/// Streaming handler — Subscription operations. Returns a stream of
/// ResponseEnvelopes: each Ok(value) → call.responded, an Err → call.error
/// (terminal — stream ends), natural stream end → call.completed.
pub type StreamingHandler = Arc<
    dyn Fn(Value, OperationContext)
        -> Pin<Box<dyn Stream<Item = ResponseEnvelope> + Send>>
        + Send + Sync,
>;

/// Type alias for the boxed stream shape used by `invoke_streaming()` and
/// `StreamingHandler` return values. `futures::stream::BoxStream<'static, T>`
/// = `Pin<Box<dyn Stream<Item = T> + Send>>` — the concrete library is a
/// two-way-door implementation detail (ADR-049); the alias exists so the two
/// spellings refer to the same type.
pub type ResponseStream = Pin<Box<dyn Stream<Item = ResponseEnvelope> + Send>>;

/// Which dispatch path a handler uses — locked by ADR-049. Validated against
/// `spec.op_type` at registration: Query/Mutation → Once; Subscription → Stream.
/// Mismatch is a startup error.
pub enum HandlerKind {
    Once(Handler),
    Stream(StreamingHandler),
}
```

`make_streaming_handler()` helper (analogue of `make_handler()`):

```rust
pub fn make_streaming_handler<S, St>(f: S) -> StreamingHandler
where
    S: Fn(Value, OperationContext) -> St + Send + Sync + 'static,
    St: Stream<Item = ResponseEnvelope> + Send + 'static,
{
    Arc::new(move |input, context| Box::pin(f(input, context)))
}
```

### INVALID_OPERATION_TYPE error code (wire.rs)

Add the sixth protocol-level error code to `CallError`:

```rust
pub fn invalid_operation_type(message: impl Into<String>) -> Self {
    Self::new("INVALID_OPERATION_TYPE", message, false)
}
```

`retryable: false`, `details: None`. This is a permanent client-side programming
error (wrong dispatch path for the operation's type), not a transient failure.
Clients should treat unknown codes as `INTERNAL` with `retryable: false` (the
existing rule); `INVALID_OPERATION_TYPE` is distinct from `INVALID_INPUT` (schema
mismatch) and `INTERNAL` (handler failure).

### HandlerRegistration.handler → HandlerKind

```rust
pub struct HandlerRegistration {
    pub spec: OperationSpec,
    pub handler: HandlerKind,  // was: Handler
    pub provenance: OperationProvenance,
    pub composition_authority: Option<CompositionAuthority>,
    pub scoped_env: Option<ScopedPeerEnv>,
    pub capabilities: Capabilities,
}
```

`HandlerRegistration::new()` takes `handler: HandlerKind` (callers wrap in
`HandlerKind::Once(...)` or `HandlerKind::Stream(...)`).

### Builder absorbs HandlerKind wrapping

The builder inspects `spec.op_type` and wraps automatically — `.with_local()`
and `.with_leaf()` / `.with_leaf_provenance()` take the raw `Handler` (for
Query/Mutation) and wrap it in `HandlerKind::Once`. For Subscription ops, add a
parallel method pair (`.with_local_streaming()` / `.with_leaf_streaming()`) that
takes a `StreamingHandler` and wraps in `HandlerKind::Stream`. The builder
validates `handler` kind matches `spec.op_type` and reports mismatch as a
startup error.

The two-method-pair approach is preferred over a typed enum input because it
keeps the common case (Query/Mutation, `Handler`) on the existing signatures
and makes the streaming case explicit at the call site. Document this choice.

### register() validation

`OperationRegistry::register()` validates that `handler` is the right
`HandlerKind` for `spec.op_type`:

- `Query` / `Mutation` → `HandlerKind::Once`
- `Subscription` → `HandlerKind::Stream`

Mismatch is a startup error. Change `register()` to return `Result<(), String>`
(preferred — startup errors should be explicit, not panics) with a clear message
(`"handler kind mismatch: {op_type} requires HandlerKind::{Once|Stream}"`).
Update all `register()` call sites to handle the Result (the builder's `store()`
and tests). Alternatively panic with a clear message — but `Result` is cleaner
for a startup error and matches the `AdapterError` pattern used elsewhere.

### invoke() errors on Stream

`OperationRegistry::invoke()` matches on `registration.handler`:

- `HandlerKind::Once(handler)` → existing dispatch path (unchanged)
- `HandlerKind::Stream(_)` → return `ResponseEnvelope::error(request_id,
  CallError::invalid_operation_type("invoke() called on a Subscription op;
  use invoke_streaming()"))`

This is the guard that prevents a streaming op from being silently truncated
through the request/response path. The `invoke_streaming()` method itself is
added in `call/registry/invoke-streaming`.

### OverlayOperationEnv (connection.rs)

`OverlayOperationEnv::invoke_with_policy` dispatches directly (it does NOT call
`registry.invoke()` — it reads the handler from the overlay and calls it). After
the type change, `registration.handler` is `HandlerKind`, so the env must match:

- `HandlerKind::Once(handler)` → `handler(input, context).await` (existing path)
- `HandlerKind::Stream(_)` → return `ResponseEnvelope::error(parent.request_id,
  CallError::invalid_operation_type("OperationEnv::invoke() called on a
  Subscription op; composition is request/response-only"))`

`LocalOperationEnv` calls `self.registry.invoke()` which already errors on
`Stream` — no change needed there. `PeerCompositeEnv` delegates to
session/connection/base envs — no change needed there either.

### Migration of existing construction sites

Every site that constructs `HandlerRegistration::new(spec, handler, ...)` must
wrap `handler` in `HandlerKind::Once(handler)`. This is mechanical. Sites
include (non-exhaustive — find them all with a grep for `HandlerRegistration::new`):

- `crates/alknet-call/src/registry/registration.rs` (tests)
- `crates/alknet-call/src/registry/env.rs` (tests)
- `crates/alknet-call/src/registry/discovery.rs` (`services_list_handler`,
  `services_schema_handler` construction — these are `Query` ops)
- `crates/alknet-call/src/protocol/dispatch.rs` (tests)
- `crates/alknet-call/src/protocol/connection.rs` (tests,
  `imported_registration` helper)
- `crates/alknet-call/src/client/from_call.rs` (`build_bundles`,
  `make_forwarding_handler`, tests)
- `crates/alknet-http/src/gateway/dispatch.rs` (tests)
- `crates/alknet-http/src/server/gateway_routes.rs` (tests)
- `crates/alknet-http/src/adapters/from_openapi.rs` (`build_registration`)
- `crates/alknet-http/src/adapters/from_mcp/mod.rs` (`build_registration`)

The builder sites (`with_local`, `with_leaf`, `with_leaf_provenance`, `with`)
are updated by the builder-absorbs-wrapping change above — their callers pass
raw `Handler` and the builder wraps. Direct `HandlerRegistration::new()` calls
need the explicit `HandlerKind::Once(...)` wrap.

## Acceptance Criteria

- [ ] `StreamingHandler` type alias in `registration.rs`
- [ ] `ResponseStream` type alias (`Pin<Box<dyn Stream<Item = ResponseEnvelope> + Send>>`)
- [ ] `HandlerKind` enum with `Once(Handler)` and `Stream(StreamingHandler)` variants
- [ ] `make_streaming_handler()` helper compiles and works
- [ ] `CallError::invalid_operation_type()` constructor in `wire.rs`
- [ ] `HandlerRegistration.handler` field is `HandlerKind` (not `Handler`)
- [ ] `HandlerRegistration::new()` takes `HandlerKind`
- [ ] Builder `with_local` / `with_leaf` / `with_leaf_provenance` wrap `Handler` in
      `HandlerKind::Once` for Query/Mutation
- [ ] Builder `with_local_streaming` / `with_leaf_streaming` wrap `StreamingHandler`
      in `HandlerKind::Stream` for Subscription
- [ ] Builder validates `handler` kind matches `spec.op_type` — mismatch is a
      startup error
- [ ] `OperationRegistry::register()` validates `HandlerKind` matches `op_type`
      (returns `Result<(), String>` or panics with clear message)
- [ ] `OperationRegistry::invoke()` dispatches `HandlerKind::Once` (existing path)
- [ ] `OperationRegistry::invoke()` returns `INVALID_OPERATION_TYPE` for
      `HandlerKind::Stream`
- [ ] `OverlayOperationEnv::invoke_with_policy` matches on `HandlerKind`:
      `Once` → dispatch, `Stream` → `INVALID_OPERATION_TYPE`
- [ ] `LocalOperationEnv` propagates `INVALID_OPERATION_TYPE` via `registry.invoke()`
      (no code change needed — verify)
- [ ] All existing `HandlerRegistration::new()` call sites wrap in
      `HandlerKind::Once(...)`
- [ ] All existing builder call sites compile (builder absorbs wrapping)
- [ ] Unit test: `invoke()` on a `Subscription` op (registered with
      `HandlerKind::Stream`) returns `INVALID_OPERATION_TYPE`
- [ ] Unit test: `invoke()` on a `Query` op (registered with `HandlerKind::Once`)
      dispatches normally
- [ ] Unit test: `register()` rejects `HandlerKind::Once` for a `Subscription` spec
- [ ] Unit test: `register()` rejects `HandlerKind::Stream` for a `Query` spec
- [ ] Unit test: `OverlayOperationEnv::invoke()` on a `Stream`-kind overlay op
      returns `INVALID_OPERATION_TYPE`
- [ ] Unit test: `make_streaming_handler` produces a working `StreamingHandler`
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo test -p alknet-http` succeeds
- [ ] `cargo clippy -p alknet-call --all-targets` succeeds with no warnings
- [ ] `cargo clippy -p alknet-http --all-targets` succeeds with no warnings
- [ ] `cargo fmt --check -p alknet-call -p alknet-http` passes

## References

- docs/architecture/decisions/049-streaming-handler-for-subscriptions.md — ADR-049 (the decision)
- docs/architecture/crates/call/operation-registry.md — §Handler (StreamingHandler, HandlerKind, ResponseStream, make_streaming_handler), §OperationRegistry (register validation, invoke errors on Stream), §HandlerRegistration
- docs/architecture/crates/call/call-protocol.md — §call.error Payload (INVALID_OPERATION_TYPE protocol code)
- docs/architecture/decisions/023-operation-error-schemas.md — ADR-023 (amended: six protocol codes)

## Notes

> This is the foundational breaking change. The `HandlerRegistration.handler`
> type flip from `Handler` to `HandlerKind` ripples to every construction site,
> but each change is mechanical (`Handler` → `HandlerKind::Once(handler)`). The
> builder absorbs the wrapping for the common case. The load-bearing parts are:
> (1) `register()` validation catches kind/op_type mismatch at startup, (2)
> `invoke()` errors on `Stream` (the guard that prevents silent truncation), (3)
> `OverlayOperationEnv` matches on `HandlerKind` (it dispatches directly, not via
> `registry.invoke()`). `LocalOperationEnv` needs no change — it delegates to
> `registry.invoke()` which handles it. Do NOT add `invoke_streaming()` in this
> task — that's `call/registry/invoke-streaming`. The `futures` crate is already
> a dependency of `alknet-call`. The two-method-pair builder API
> (`with_local`/`with_local_streaming`) is preferred over a typed enum input —
> it keeps the common case on existing signatures and makes streaming explicit.

## Summary

> Introduced StreamingHandler/ResponseStream type aliases and HandlerKind enum (Once|Stream) + make_streaming_handler() helper in registration.rs; added CallError::invalid_operation_type() (sixth protocol code, retryable: false) in wire.rs; flipped HandlerRegistration.handler to HandlerKind and changed new() signature; builder absorbs wrapping (with_local/with_leaf wrap Handler in Once for Query/Mutation, new with_local_streaming/with_leaf_streaming take StreamingHandler and wrap in Stream for Subscription) with kind/op_type mismatch validation; OperationRegistry::register() now returns Result<(), String> with clear mismatch message; invoke() errors on HandlerKind::Stream with INVALID_OPERATION_TYPE; OverlayOperationEnv::invoke_with_policy matches on HandlerKind (Stream -> INVALID_OPERATION_TYPE); migrated all ~95 HandlerRegistration::new() call sites to wrap in HandlerKind::Once(handler); updated two websocket subscription tests to expect INVALID_OPERATION_TYPE; added unit tests for invoke/register validation, make_streaming_handler, and overlay Stream-kind rejection. All verification passes (build, clippy -D warnings, test, fmt --check) for alknet-call + alknet-http.