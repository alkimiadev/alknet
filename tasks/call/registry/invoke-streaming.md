---
id: call/registry/invoke-streaming
name: Implement OperationRegistry::invoke_streaming() returning ResponseStream
status: pending
depends_on: [call/registry/streaming-handler-handlerkind]
scope: narrow
risk: medium
impact: component
level: implementation
---

## Description

Add `OperationRegistry::invoke_streaming()` — the streaming dispatch path that
`Subscription` operations use. This is the counterpart to `invoke()`: same
visibility + ACL checks, then dispatches to the `StreamingHandler` and returns
the `ResponseStream`. Pre-handler errors (not-found, forbidden,
`INVALID_OPERATION_TYPE` for a non-Subscription op) yield a single error
`ResponseEnvelope` and end the stream.

This task depends on `call/registry/streaming-handler-handlerkind` (which
introduces `HandlerKind::Stream` and the `ResponseStream` alias). It adds only
the `invoke_streaming()` method — no other changes.

### invoke_streaming()

```rust
use futures::stream::{self, StreamExt};

impl OperationRegistry {
    /// Dispatch a Subscription operation. Returns a stream of
    /// ResponseEnvelopes. Pre-handler errors (not-found, forbidden,
    /// INVALID_OPERATION_TYPE for a non-Subscription op) yield a single
    /// error ResponseEnvelope and end the stream.
    pub fn invoke_streaming(
        &self,
        name: &str,
        input: Value,
        context: OperationContext,
    ) -> ResponseStream {
        let request_id = context.request_id.clone();

        // 1. Look up registration
        let registration = match self.operations.get(name) {
            Some(r) => r,
            None => {
                return Box::pin(stream::once(async move {
                    ResponseEnvelope::not_found(request_id, name)
                }));
            }
        };

        // 2. Visibility check (same as invoke)
        if registration.spec.visibility == Visibility::Internal && !context.internal {
            return Box::pin(stream::once(async move {
                ResponseEnvelope::not_found(request_id, name)
            }));
        }

        // 3. ACL check (same as invoke)
        let acl = &registration.spec.access_control;
        let identity = if context.internal {
            context.handler_identity.as_ref().and_then(|ca| ca.as_identity())
        } else {
            context.identity.clone()
        };
        if let AccessResult::Forbidden(message) = acl.check(identity.as_ref()) {
            return Box::pin(stream::once(async move {
                ResponseEnvelope::forbidden(request_id, message)
            }));
        }

        // 4. HandlerKind check — must be Stream for invoke_streaming
        let streaming_handler = match &registration.handler {
            HandlerKind::Stream(h) => Arc::clone(h),
            HandlerKind::Once(_) => {
                return Box::pin(stream::once(async move {
                    ResponseEnvelope::error(
                        request_id,
                        CallError::invalid_operation_type(
                            "invoke_streaming() called on a Query/Mutation op; use invoke()"
                        ),
                    )
                }));
            }
        };

        // 5. Dispatch — the handler returns the stream
        streaming_handler(input, context)
    }
}
```

The visibility + ACL checks are **identical** to `invoke()` — extract them into
a private helper if it reduces duplication, but the spec requires the security
axis to be provably identical between `invoke()` and `invoke_streaming()`. The
two methods diverge only on the return shape (single envelope vs stream) and
the handler-kind guard (Once vs Stream).

### Pre-handler errors as single-item streams

A pre-handler error (not-found, forbidden, wrong kind) produces a
`ResponseStream` that yields exactly one error `ResponseEnvelope` and then ends.
This matches the single-response path's behavior, just on a stream — the caller
(`Dispatcher::handle_stream` streaming branch, `GatewayDispatch::invoke_streaming`)
drains the stream and writes frames; a one-item error stream produces one
`call.error` frame and closes.

Use `futures::stream::once(async move { ... })` to build these single-item
streams. The error envelope carries the `request_id` from the context.

### What this task does NOT do

- **No `OperationEnv::invoke_streaming()`.** Composition stays
  request/response-only (ADR-049). `OperationEnv::invoke()` errors on
  `Subscription` (handled in `streaming-handler-handlerkind` via the
  `HandlerKind::Stream` match in `LocalOperationEnv` → `registry.invoke()` and
  `OverlayOperationEnv` direct match). No streaming variant is added to the
  trait.
- **No dispatch-loop wiring.** `Dispatcher::handle_stream` streaming branch is
  `call/protocol/dispatch-streaming-branch`.
- **No gateway wiring.** `GatewayDispatch::invoke_streaming` is
  `http/gateway/invoke-streaming`.

## Acceptance Criteria

- [ ] `OperationRegistry::invoke_streaming()` method exists
- [ ] Returns `ResponseStream` (`Pin<Box<dyn Stream<Item = ResponseEnvelope> + Send>>`)
- [ ] Not-found op → single-item stream with `NOT_FOUND` error envelope, then ends
- [ ] Internal op from external call → single-item stream with `NOT_FOUND`, then ends
- [ ] ACL denied → single-item stream with `FORBIDDEN`, then ends
- [ ] `HandlerKind::Once` op (Query/Mutation) → single-item stream with
      `INVALID_OPERATION_TYPE`, then ends
- [ ] `HandlerKind::Stream` op (Subscription) → dispatches the `StreamingHandler`,
      returns its stream
- [ ] Visibility + ACL checks identical to `invoke()` (same authority switch:
      internal → handler_identity, external → identity)
- [ ] Unit test: `invoke_streaming()` on a registered `Subscription` op yields
      the handler's stream items
- [ ] Unit test: `invoke_streaming()` on unknown op yields one `NOT_FOUND` then ends
- [ ] Unit test: `invoke_streaming()` on a `Query` op yields one
      `INVALID_OPERATION_TYPE` then ends
- [ ] Unit test: `invoke_streaming()` on Internal op from external context yields
      one `NOT_FOUND` then ends
- [ ] Unit test: `invoke_streaming()` ACL denied yields one `FORBIDDEN` then ends
- [ ] Unit test: `invoke_streaming()` internal call uses handler_identity for ACL
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call --all-targets` succeeds with no warnings
- [ ] `cargo fmt --check -p alknet-call` passes

## References

- docs/architecture/decisions/049-streaming-handler-for-subscriptions.md — ADR-049 §3 (invoke_streaming), §4 (invoke errors on Subscription), §5 (OperationEnv request/response-only)
- docs/architecture/crates/call/operation-registry.md — §OperationRegistry (invoke_streaming signature, pre-handler errors as single-item streams)

## Notes

> The visibility + ACL checks MUST be identical to `invoke()` — the spec calls
> this out explicitly: "invoke_streaming() performs the same visibility + ACL
> checks as invoke()". Extract a shared helper if it helps, but the security
> axis must be provably identical. Pre-handler errors become single-item streams
> (one error envelope, then end) — this matches the single-response path's
> behavior, just on a stream. Do NOT add `OperationEnv::invoke_streaming()` —
> composition is request/response-only by design (ADR-049 §5); stream
> composition is a handler-level concern. The `futures` crate's `stream::once`
> and `StreamExt` are the tools for building single-item streams.

## Summary

> To be filled on completion