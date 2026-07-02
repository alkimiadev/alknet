---
id: http/server/subscribe-sse-streaming
name: Wire /subscribe handler to GatewayDispatch::invoke_streaming() and pipe BoxStream to SSE
status: completed
depends_on: [http/gateway/invoke-streaming]
scope: narrow
risk: medium
impact: component
level: implementation
---

## Description

Replace the `/subscribe` handler's one-event placeholder
(`subscribe_stream_from_envelope`, which calls `GatewayDispatch::invoke()` and
wraps the single `ResponseEnvelope` in a one-event SSE stream) with the real
streaming path: call `GatewayDispatch::invoke_streaming()` and pipe the
`BoxStream<ResponseEnvelope>` to SSE. Each `Ok(value)` → SSE `data:` frame;
`Err` → SSE error event + close (terminal); natural stream end → close (normal
end, corresponds to `call.completed` on the wire). On `call.aborted` or HTTP
client disconnect, drop the stream (Drop releases handler resources, abort
cascade runs per ADR-016).

This task depends on `http/gateway/invoke-streaming` (which provides
`GatewayDispatch::invoke_streaming()`). It rewrites `subscribe_handler` and
removes the placeholder helpers.

### subscribe_handler rewrite

```rust
pub(crate) async fn subscribe_handler(
    State(state): State<GatewayState>,
    ResolvedIdentity(identity): ResolvedIdentity,
    Json(request): Json<CallRequest>,
) -> Sse<SubscribeStream> {
    let stream = if is_internal_op(&state.registry, &request.operation) {
        // Internal ops return NOT_FOUND (don't leak existence) — single error event
        subscribe_stream_internal_error(request.operation)
    } else {
        let dispatch = state.dispatch();
        let envelope_stream = dispatch
            .invoke_streaming(identity, &request.operation, request.input)
            .await;
        // Pipe the BoxStream<ResponseEnvelope> to SSE frames
        subscribe_stream_from_envelope_stream(envelope_stream)
    };
    Sse::new(stream)
}
```

### subscribe_stream_from_envelope_stream

Map each `ResponseEnvelope` in the `BoxStream` to an SSE `Event`:

```rust
fn subscribe_stream_from_envelope_stream(
    stream: BoxStream<'static, ResponseEnvelope>,
) -> SubscribeStream {
    Box::pin(stream.map(|envelope| {
        match envelope.result {
            Ok(output) => {
                let data = serde_json::to_string(&output)
                    .unwrap_or_else(|_| "null".to_string());
                Ok(Event::default().data(data))
            }
            Err(error) => {
                let payload = serde_json::to_value(&error).unwrap_or(Value::Null);
                let data = serde_json::to_string(&payload)
                    .unwrap_or_else(|_| "null".to_string());
                Ok(Event::default().event("error").data(data))
            }
        }
    }))
}
```

The `Err` case produces an SSE error event — the stream ends after it (the
`StreamingHandler`'s contract: `Err` is terminal). The natural stream end
(stream yields `None`) closes the SSE stream (axum's `Sse` wrapper handles the
close when the underlying stream ends).

### Remove the placeholder

Delete `subscribe_stream_from_envelope` (the one-event placeholder) and
`envelope_to_sse_stream` (the single-envelope-to-stream helper). The new
`subscribe_stream_from_envelope_stream` replaces them. Keep
`subscribe_stream_internal_error` (Internal ops still return a single
`NOT_FOUND` error event — they don't reach `invoke_streaming()`).

### Client disconnect / abort

axum's `Sse` response detects when the HTTP client disconnects (the response
writer closes) and drops the stream future. `Drop` releases the handler's
resources, and the abort cascade runs per ADR-016. No explicit disconnect
handling is needed — Rust's `Drop` + axum's response-drop handle it. Verify the
stream is dropped (not leaked) on disconnect.

### What this task does NOT do

- **No `GatewayDispatch` changes.** `invoke_streaming()` is provided by
  `http/gateway/invoke-streaming`.
- **No `to_mcp` changes.** MCP has no `/subscribe` equivalent (ADR-041).
- **No `from_openapi` changes.** `from_openapi` SSE forwarding is
  `http/adapters/from-openapi-sse-streaming`.

## Acceptance Criteria

- [ ] `subscribe_handler` calls `GatewayDispatch::invoke_streaming()` (not
      `invoke()`)
- [ ] `subscribe_stream_from_envelope_stream` maps `BoxStream<ResponseEnvelope>`
      to SSE `Event`s
- [ ] `Ok(value)` → SSE `data:` frame with output serialized as JSON
- [ ] `Err` → SSE error event (`event: error`) with `CallError` serialized, then
      stream ends (terminal)
- [ ] Natural stream end → SSE stream closes (normal end)
- [ ] Internal op → single `NOT_FOUND` error event (unchanged —
      `subscribe_stream_internal_error` kept)
- [ ] Client disconnect → stream dropped (Drop releases resources; abort cascade)
- [ ] Placeholder helpers (`subscribe_stream_from_envelope`,
      `envelope_to_sse_stream`) removed
- [ ] `SubscribeStream` type alias still `BoxStream<'static, Result<Event, Infallible>>`
- [ ] Unit test: `/subscribe` on a `Subscription` op streams multiple `data:`
      frames (one per `call.responded`)
- [ ] Unit test: `/subscribe` on a `Subscription` op that yields `Err` → one
      `event:error` frame, then stream closes
- [ ] Unit test: `/subscribe` on Internal op → `event:error` with `NOT_FOUND`
      (unchanged)
- [ ] Unit test: `/subscribe` on unknown op → `event:error` with `NOT_FOUND`
- [ ] Unit test: `/subscribe` on `Query` op → `event:error` with
      `INVALID_OPERATION_TYPE` (the guard holds through the gateway)
- [ ] Unit test: response `Content-Type` is `text/event-stream`
- [ ] `cargo test -p alknet-http` succeeds
- [ ] `cargo clippy -p alknet-http --all-targets` succeeds with no warnings
- [ ] `cargo fmt --check -p alknet-http` passes

## References

- docs/architecture/decisions/049-streaming-handler-for-subscriptions.md — ADR-049 §7 (HTTP /subscribe pipes BoxStream to SSE)
- docs/architecture/crates/http/http-server.md — §Streaming projection (SSE — the gateway's /subscribe)
- docs/architecture/decisions/016-abort-cascade-for-nested-calls.md — ADR-016 (stream drop on disconnect/abort)

## Notes

> This replaces the one-event placeholder with the real streaming path. The
> `Err` envelope is terminal — the SSE stream ends after the error event (no
> `data:` frame after an `event:error`). Natural stream end closes the SSE
> stream (axum handles the close when the underlying stream ends). Client
> disconnect drops the stream future via Rust's `Drop` — no explicit handling
> needed. Keep `subscribe_stream_internal_error` (Internal ops return
> `NOT_FOUND` without reaching `invoke_streaming()` — they don't leak
> existence). The `futures::StreamExt::map` combinator is the tool for mapping
> the envelope stream to SSE events.

## Summary

> Replaced /subscribe one-event placeholder with real streaming path. subscribe_handler now calls GatewayDispatch::invoke_streaming() and pipes BoxStream to SSE via subscribe_stream_from_envelope_stream (StreamExt::map). Ok → data: frame, Err → event:error (terminal, stream ends after). Removed placeholder helpers (subscribe_stream_from_envelope, envelope_to_sse_stream). Kept subscribe_stream_internal_error for Internal ops (NOT_FOUND). Added 6 unit tests. Also fixed 2 pre-existing websocket subscription tests that expected INVALID_OPERATION_TYPE but now get call.responded (dispatch_requested routes Subscription via invoke_streaming). 247 tests pass.