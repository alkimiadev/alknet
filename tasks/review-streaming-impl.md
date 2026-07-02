---
id: review-streaming-impl
name: Review ADR-049 streaming handler implementation for spec conformance and end-to-end correctness
status: pending
depends_on: [call/protocol/dispatch-streaming-branch, call/client/from-call-streaming-forwarding, http/gateway/invoke-streaming, http/server/subscribe-sse-streaming, http/adapters/from-openapi-sse-streaming]
scope: broad
risk: low
impact: phase
level: review
---

## Description

Review the ADR-049 streaming handler implementation across `alknet-call` and
`alknet-http` for spec conformance, end-to-end correctness, and pattern
consistency. This is the quality checkpoint after the streaming handler work —
the most significant cross-cutting change since the initial call/http
implementation. All five implementation tasks must be complete before this
review.

### Review Checklist

1. **Type surface conformance** (operation-registry.md §Handler):
   - `StreamingHandler` type alias matches spec (`Arc<dyn Fn(Value, OperationContext) -> Pin<Box<dyn Stream<Item = ResponseEnvelope> + Send>> + Send + Sync>`)
   - `ResponseStream` alias = `Pin<Box<dyn Stream<Item = ResponseEnvelope> + Send>>` (= `futures::stream::BoxStream<'static, ResponseEnvelope>`)
   - `HandlerKind` enum with `Once(Handler)` and `Stream(StreamingHandler)` variants
   - `make_streaming_handler()` helper (analogue of `make_handler()`)
   - `HandlerRegistration.handler` is `HandlerKind` (not `Handler`)
   - `INVALID_OPERATION_TYPE` is the sixth protocol-level code in `CallError`
     (`retryable: false`, `details: None`)

2. **Registry conformance** (operation-registry.md §OperationRegistry):
   - `register()` validates `HandlerKind` matches `spec.op_type` (Once for
     Query/Mutation, Stream for Subscription) — mismatch is a startup error
   - `invoke()` dispatches `HandlerKind::Once` (existing path); returns
     `INVALID_OPERATION_TYPE` for `HandlerKind::Stream`
   - `invoke_streaming()` dispatches `HandlerKind::Stream`; returns
     `INVALID_OPERATION_TYPE` for `HandlerKind::Once`
   - `invoke_streaming()` pre-handler errors (not-found, forbidden,
     INVALID_OPERATION_TYPE) yield a single error `ResponseEnvelope` and end
     the stream
   - `invoke_streaming()` visibility + ACL checks identical to `invoke()`
     (same authority switch: internal → handler_identity, external → identity)
   - `OperationEnv` is request/response-only — no `invoke_streaming()` on the
     trait; `invoke()` on a `Subscription` returns `INVALID_OPERATION_TYPE`
     (via `LocalOperationEnv` → `registry.invoke()` and `OverlayOperationEnv`
     direct match)

3. **Builder conformance** (operation-registry.md §OperationRegistryBuilder):
   - `with_local` / `with_leaf` / `with_leaf_provenance` wrap `Handler` in
     `HandlerKind::Once` for Query/Mutation
   - `with_local_streaming` / `with_leaf_streaming` wrap `StreamingHandler` in
     `HandlerKind::Stream` for Subscription
   - Builder validates `handler` kind matches `spec.op_type`

4. **Call-protocol dispatch conformance** (call-protocol.md §CallAdapter Stream Handling):
   - `Dispatcher::handle_stream` branches on `op_type`: `Subscription` →
     `invoke_streaming()` → pump stream; `Query`/`Mutation` → `invoke()` (existing)
   - Streaming pump: each `ResponseEnvelope` → `EventEnvelope` frame
   - Natural stream end → `call.completed` frame
   - `Err` envelope → `call.error` frame, stream ends after it (NO
     `call.completed` after an error)
   - `deadline: None` for subscriptions (unbounded)
   - Abort: `call.aborted` drops the stream (Drop releases resources; ADR-016)

5. **from_call streaming forwarding** (client-and-adapters.md §from_call):
   - `build_bundles` branches on `op_type`: `Subscription` →
     `make_streaming_forwarding_handler` (`HandlerKind::Stream`),
     `Query`/`Mutation` → `make_forwarding_handler` (`HandlerKind::Once`)
   - Streaming forwarding handler calls `CallConnection::subscribe_with_payload()`
     (or `subscribe()` with forwarded payload)
   - `forwarded_for` populated from `context.identity` (ADR-032)
   - Remote stream forwarded end-to-end (no truncation, no first-value fallback)
   - `composition_authority: None`, `scoped_env: None` for FromCall streaming leaves

6. **Gateway dispatch conformance** (http-server.md §Streaming projection):
   - `GatewayDispatch::invoke_streaming()` exists, returns
     `BoxStream<ResponseEnvelope>`
   - Security invariants identical to `invoke()` (shared `build_root_context`):
     `internal: false`, `forwarded_for: None`, same capabilities, same scoped_env,
     same ACL
   - `deadline: None` for the streaming path
   - `to_mcp` does NOT call `invoke_streaming()` (MCP excludes Subscription)

7. **/subscribe SSE conformance** (http-server.md §Streaming projection):
   - `subscribe_handler` calls `GatewayDispatch::invoke_streaming()` (not `invoke()`)
   - Each `Ok(value)` → SSE `data:` frame
   - `Err` → SSE error event (`event: error`), stream ends after (terminal)
   - Natural stream end → SSE stream closes
   - Internal op → single `NOT_FOUND` error event (no leak)
   - Client disconnect → stream dropped (Drop; abort cascade)
   - Placeholder helpers removed (`subscribe_stream_from_envelope`,
     `envelope_to_sse_stream`)

8. **from_openapi SSE conformance** (http-adapters.md §Forwarding handler):
   - `build_registration` branches on `op_type`: `Subscription` →
     `HandlerKind::Stream` (streaming forward), `Query`/`Mutation` →
     `HandlerKind::Once` (existing)
   - `forward_stream()` streams SSE chunks as `ResponseEnvelope::ok()` items
   - HTTP error → single `ResponseEnvelope::error()`, stream ends
   - SSE stream end → `ResponseStream` ends
   - `parse_sse_frames` reused (multi-event, partial, comments, BOM)
   - `stream_subscription()` collect-all placeholder removed
   - `from_mcp` unchanged (always `HandlerKind::Once`)

9. **ADR conformance**:
   - ADR-049: all 9 decisions implemented (StreamingHandler, HandlerKind,
     invoke_streaming, invoke errors on Subscription, OperationEnv
     request/response-only, server-side dispatch branch,
     GatewayDispatch::invoke_streaming, from_call stream forwarding,
     from_openapi SSE forwarding)
   - ADR-023 amended: six protocol codes (INVALID_OPERATION_TYPE added)
   - ADR-016: abort cascade on streaming (stream drop)
   - ADR-032: forwarded_for on streaming forwarding handlers

10. **End-to-end correctness**:
    - A `Subscription` op registered with a `StreamingHandler` streams
      `call.responded` events through: server dispatch → wire → HTTP `/subscribe`
      SSE (or `from_call` forwarding → remote stream, or `from_openapi` SSE
      forwarding)
    - `invoke()` on a `Subscription` returns `INVALID_OPERATION_TYPE` (not silent
      truncation)
    - `invoke_streaming()` on a `Query`/`Mutation` returns
      `INVALID_OPERATION_TYPE`
    - `OperationEnv::invoke()` on a `Subscription` returns
      `INVALID_OPERATION_TYPE` (composition is request/response-only)

11. **Pattern consistency**:
    - `invoke()` and `invoke_streaming()` share visibility + ACL logic (security
      axis provably identical)
    - `GatewayDispatch::invoke()` and `invoke_streaming()` share
      `build_root_context` (security axis provably identical)
    - `HandlerKind` makes the "one or the other, matching op_type" invariant
      type-level (not two `Option`s validated at runtime)
    - Existing `Query`/`Mutation` handlers unchanged (wrapped in
      `HandlerKind::Once`, dispatch path identical)

12. **Test coverage**:
    - Unit tests for `HandlerKind` validation at `register()` (both mismatch
      directions)
    - Unit tests for `invoke()` / `invoke_streaming()` cross-kind errors
      (`INVALID_OPERATION_TYPE` both directions)
    - Unit tests for `invoke_streaming()` pre-handler errors (not-found,
      forbidden, internal-from-external)
    - Unit tests for `invoke_streaming()` ACL authority switch (internal →
      handler_identity)
    - Unit test for `make_streaming_handler`
    - Unit/integration tests for server-side streaming dispatch (multiple
      `call.responded` + `call.completed`; `Err` → `call.error`, no
      `call.completed` after)
    - Unit/integration tests for `from_call` streaming forwarding
    - Unit/integration tests for `from_openapi` SSE streaming forwarding
    - Unit tests for `/subscribe` SSE (multiple `data:` frames; `event:error`;
      `INVALID_OPERATION_TYPE` for `Query` op via `/subscribe`)
    - Unit tests for `GatewayDispatch::invoke_streaming()` (all error paths)

## Acceptance Criteria

- [ ] All type surface matches operation-registry.md §Handler
- [ ] All registry methods match operation-registry.md §OperationRegistry
- [ ] Builder wraps HandlerKind correctly per op_type
- [ ] Call-protocol dispatch branches on op_type correctly
- [ ] from_call streaming forwarding works end-to-end
- [ ] GatewayDispatch::invoke_streaming security invariants identical to invoke
- [ ] /subscribe SSE pipes BoxStream correctly
- [ ] from_openapi SSE streaming works (no truncation)
- [ ] from_mcp unchanged (always HandlerKind::Once)
- [ ] ADR-049 all 9 decisions implemented
- [ ] ADR-023 amended (six protocol codes)
- [ ] invoke() / invoke_streaming() / OperationEnv::invoke() cross-kind errors
      all return INVALID_OPERATION_TYPE
- [ ] Existing Query/Mutation handlers unchanged
- [ ] Test coverage adequate for all streaming functionality
- [ ] `cargo fmt --check -p alknet-call -p alknet-http` passes
- [ ] `cargo clippy -p alknet-call -p alknet-http --all-targets` passes with no warnings
- [ ] All tests pass (`cargo test -p alknet-call -p alknet-http`)

## References

- docs/architecture/decisions/049-streaming-handler-for-subscriptions.md — ADR-049
- docs/architecture/crates/call/operation-registry.md — Handler, OperationRegistry, HandlerRegistration
- docs/architecture/crates/call/call-protocol.md — CallAdapter Stream Handling, call.error Payload
- docs/architecture/crates/call/client-and-adapters.md — from_call streaming forwarding
- docs/architecture/crates/http/http-server.md — Streaming projection (SSE)
- docs/architecture/crates/http/http-adapters.md — Forwarding handler (from_openapi)
- docs/architecture/crates/http/http-mcp.md — from_mcp always HandlerKind::Once
- docs/architecture/decisions/023-operation-error-schemas.md — ADR-023 (amended: six codes)
- docs/architecture/decisions/016-abort-cascade-for-nested-calls.md — ADR-016 (stream drop)
- docs/architecture/decisions/032-forwarded-for-identity.md — ADR-032 (forwarded_for)

## Notes

> This is the quality checkpoint for the ADR-049 streaming handler work — the
> most significant cross-cutting change since the initial call/http
> implementation. The review should verify the end-to-end streaming path works:
> a `Subscription` op registered with a `StreamingHandler` streams
> `call.responded` events through every projection (server dispatch → wire,
> HTTP `/subscribe` SSE, `from_call` forwarding, `from_openapi` SSE forwarding).
> The load-bearing invariants: (1) `invoke()` / `invoke_streaming()` /
> `OperationEnv::invoke()` cross-kind errors all return
> `INVALID_OPERATION_TYPE` (no silent truncation), (2) the security axis is
> provably identical between `invoke()` and `invoke_streaming()` (shared
> `build_root_context` + shared visibility/ACL logic), (3) `HandlerKind` makes
> the kind/op_type invariant type-level, (4) existing `Query`/`Mutation`
> handlers are unchanged. If deviations are found, document and fix before
> considering the streaming handler work complete.

## Summary

> To be filled on completion