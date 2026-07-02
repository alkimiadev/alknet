---
id: call/client/from-call-streaming-forwarding
name: Implement from_call streaming forwarding handler (Subscription → CallConnection::subscribe → StreamingHandler)
status: pending
depends_on: [call/registry/streaming-handler-handlerkind]
scope: narrow
risk: medium
impact: component
level: implementation
---

## Description

Branch `from_call`'s forwarding handler construction on `op_type` so that a
`Subscription` op discovered via `services/list` + `services/schema` registers a
`StreamingHandler` (`HandlerKind::Stream`) that calls
`CallConnection::subscribe()` and forwards the remote stream end-to-end.
`Query`/`Mutation` ops keep the existing `make_forwarding_handler()` (single
`call_with_payload()`, `HandlerKind::Once`). This closes the gap where a
`from_call`-imported `Subscription` truncated to the first value.

This task depends on `call/registry/streaming-handler-handlerkind` (which
introduces `HandlerKind::Stream` and `make_streaming_handler`). The
`CallConnection::subscribe()` client-side path already works (it returns
`impl Stream<Item = ResponseEnvelope>`); this task wires it into the forwarding
handler.

### The branch in build_bundles

`build_bundles` currently constructs one `make_forwarding_handler()` per
discovered op and wraps in `HandlerKind::Once`. Branch on
`op_summary.op_type` (parsed from `services/schema`):

- `Query` / `Mutation` → `make_forwarding_handler()` (existing), wrap in
  `HandlerKind::Once`
- `Subscription` → `make_streaming_forwarding_handler()` (new), wrap in
  `HandlerKind::Stream`

The `op_type` is already parsed in `rebuild_spec_for` (it reads `schema.op_type`
and produces `OperationType::Subscription`). The `OpSummary` needs to carry the
`op_type` (or the spec's `op_type` is read from the constructed `spec`). Read
`spec.op_type` after `rebuild_spec_for` to decide the handler kind.

### make_streaming_forwarding_handler

```rust
fn make_streaming_forwarding_handler(
    connection: Arc<CallConnection>,
    remote_name: String,
    credentials_auth_token: Option<String>,
) -> StreamingHandler {
    use crate::registry::registration::make_streaming_handler;
    make_streaming_handler(move |input, context| {
        let connection = Arc::clone(&connection);
        let remote_name = remote_name.clone();
        let auth_token = credentials_auth_token.clone();
        // The streaming forwarding handler calls subscribe() and forwards the
        // remote stream. forwarded_for is populated from context.identity
        // (ADR-032), same as the request/response forwarding handler.
        async move {
            // Build the payload (same as build_forwarded_payload, but for subscribe)
            let payload = build_forwarded_payload(&remote_name, input, &context, auth_token.as_deref());
            // CallConnection::subscribe takes (operation_id, input); for the
            // forwarded payload path, we need a subscribe_with_payload variant,
            // OR we call subscribe(remote_name, input) and let it build the
            // payload. The forwarded_for + auth_token need to be in the payload,
            // so a subscribe_with_payload variant is needed (mirrors
            // call_with_payload). Check if CallConnection::subscribe can accept
            // a full payload — if not, add subscribe_with_payload().
            let stream = connection.subscribe_with_payload(payload).await;
            // Map the impl Stream<Item=ResponseEnvelope> to BoxStream<ResponseEnvelope>
            Box::pin(stream) as ResponseStream
        }
    })
}
```

**Coordinate with `CallConnection::subscribe`**: the existing
`subscribe(operation_id, input)` builds the payload internally and does NOT
populate `forwarded_for` or `auth_token`. The forwarding handler needs those
fields (ADR-032). Two options:

1. Add `CallConnection::subscribe_with_payload(payload: Value)` (mirrors
   `call_with_payload`) that takes a caller-constructed payload. The forwarding
   handler builds the payload with `build_forwarded_payload` and calls
   `subscribe_with_payload`.
2. Extend `subscribe()` to accept optional `forwarded_for` / `auth_token`.

Option 1 mirrors the existing `call` / `call_with_payload` split and is cleaner.
Add `subscribe_with_payload()` alongside `subscribe()`.

### forwarded_for on the streaming payload

The streaming forwarding handler populates `forwarded_for` from
`context.identity` exactly as the request/response forwarding handler does
(ADR-032 §3) — reuse `build_forwarded_payload()`. The `auth_token` (hub's own
call-protocol token) is also populated identically. No new payload-construction
code; reuse the existing `build_forwarded_payload`.

### Abort cascade (ADR-016 §6)

The streaming forwarding handler's `parent_request_id` participates in the
abort cascade: if the parent is aborted, the cascade reaches this handler,
which sends `call.aborted` to the remote node; the remote node cascades to its
own descendants. Cross-node abort is transparent. The `subscribe_with_payload`
path registers the request in `PendingRequestMap` (the existing `subscribe()`
does this); abort handling is already wired. Verify the streaming forwarding
handler's stream is dropped on parent abort (the `SubscriptionStream`'s `Drop`
or the pending entry's removal handles it).

### What this task does NOT do

- **No `OperationEnv::invoke_streaming()`.** Composition is request/response-only.
- **No server-side dispatch changes.** The server-side streaming branch is
  `call/protocol/dispatch-streaming-branch`.
- **No gateway changes.** The gateway streaming path is `http/gateway/invoke-streaming`.

## Acceptance Criteria

- [ ] `build_bundles` branches on `spec.op_type`: `Subscription` → streaming
      forwarding handler (`HandlerKind::Stream`), `Query`/`Mutation` → existing
      `HandlerKind::Once`
- [ ] `make_streaming_forwarding_handler()` constructs a `StreamingHandler`
- [ ] Streaming forwarding handler calls `CallConnection::subscribe_with_payload()`
      (or `subscribe()` with the forwarded payload) and forwards the remote stream
- [ ] `CallConnection::subscribe_with_payload(payload)` exists (mirrors
      `call_with_payload`) OR `subscribe()` accepts the forwarded payload
- [ ] `forwarded_for` populated from `context.identity` (ADR-032) on the
      streaming payload (reuse `build_forwarded_payload`)
- [ ] `auth_token` populated when present (reuse `build_forwarded_payload`)
- [ ] Remote stream forwarded end-to-end: each `call.responded` → stream item,
      `call.completed` → stream end, `call.aborted` → stream dropped
- [ ] No truncation, no first-value fallback
- [ ] `composition_authority: None`, `scoped_env: None` for FromCall streaming
      leaves (same as Query/Mutation FromCall leaves)
- [ ] Unit test: `build_bundles` with a `Subscription` op produces a
      `HandlerKind::Stream` registration
- [ ] Unit test: `build_bundles` with a `Query` op produces `HandlerKind::Once`
      (unchanged)
- [ ] Unit test: `make_streaming_forwarding_handler` produces a `StreamingHandler`
      that calls `subscribe_with_payload` (verify via payload capture, mirroring
      the existing `forwarding_handler_populates_forwarded_for` test)
- [ ] Integration test: subscription forwarding streams remote events (if
      feasible with mock connection; otherwise document the integration test as
      deferred to a connection-level integration test)
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call --all-targets` succeeds with no warnings
- [ ] `cargo fmt --check -p alknet-call` passes

## References

- docs/architecture/decisions/049-streaming-handler-for-subscriptions.md — ADR-049 §8 (from_call stream forwarding)
- docs/architecture/crates/call/client-and-adapters.md — §from_call (handler branched on op_type; Subscription → StreamingHandler via subscribe())
- docs/architecture/decisions/017-call-protocol-client-and-adapter-contract.md — ADR-017 §3 (from_call flow), §6 (cross-node abort)
- docs/architecture/decisions/032-forwarded-for-identity.md — ADR-032 §3 (forwarded_for population)

## Notes

> The client-side `CallConnection::subscribe()` already works — this task wires
> it into the forwarding handler. The main new piece is
> `subscribe_with_payload()` (mirroring `call_with_payload`) so the forwarding
> handler can populate `forwarded_for` + `auth_token`. Reuse
> `build_forwarded_payload` — no new payload-construction code. The abort
> cascade is already wired via `PendingRequestMap`; verify the stream drops on
> parent abort. The `OpSummary` / `build_bundles` needs the `op_type` to
> branch — read it from the constructed `spec.op_type` (already parsed by
> `rebuild_spec_for`). Cross-node abort is transparent via
> `parent_request_id` (ADR-016 §6).

## Summary

> To be filled on completion