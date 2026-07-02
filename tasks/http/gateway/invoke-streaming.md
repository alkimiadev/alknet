---
id: http/gateway/invoke-streaming
name: Implement GatewayDispatch::invoke_streaming() returning BoxStream<ResponseEnvelope>
status: completed
depends_on: [call/registry/invoke-streaming]
scope: narrow
risk: medium
impact: component
level: implementation
---

## Description

Add `GatewayDispatch::invoke_streaming()` — the streaming analogue of
`invoke()`, returning a `BoxStream<ResponseEnvelope>`. The security invariants
are identical to `invoke()`: `internal: false`, `forwarded_for: None`, same
capabilities, same `scoped_env`, same ACL check before dispatch. The two methods
diverge only on the return shape (stream vs single envelope). The HTTP
`/subscribe` handler calls this and pipes the stream to SSE.

This task depends on `call/registry/invoke-streaming` (which provides
`OperationRegistry::invoke_streaming()`). It adds the `GatewayDispatch` method
only — the `/subscribe` SSE wiring is `http/server/subscribe-sse-streaming`.

### invoke_streaming()

```rust
use futures::stream::BoxStream;

impl GatewayDispatch {
    /// Invoke a Subscription operation as a wire-ingress caller. The streaming
    /// analogue of `invoke()`. Security invariants identical to `invoke()`:
    /// `internal: false`, `forwarded_for: None`, same capabilities, same
    /// scoped_env, same ACL. Diverges only on return shape (stream vs single
    /// envelope). Returns a `BoxStream<ResponseEnvelope>`; the `/subscribe`
    /// handler pipes it to SSE.
    pub async fn invoke_streaming(
        &self,
        identity: Option<Identity>,
        op: &str,
        input: Value,
    ) -> BoxStream<'static, ResponseEnvelope> {
        let operation_name = strip_leading_slash(op).to_string();
        let request_id = uuid::Uuid::new_v4().to_string();
        let context = self.build_root_context(&request_id, &operation_name, identity);
        // The registry's invoke_streaming returns ResponseStream
        // (Pin<Box<dyn Stream<Item = ResponseEnvelope> + Send>>), which IS
        // BoxStream<'static, ResponseEnvelope>. Box::pin / BoxStream are the
        // same type — the alias just spells it out.
        self.registry.invoke_streaming(&operation_name, input, context)
    }
}
```

`build_root_context` is reused unchanged — it constructs the root
`OperationContext` with `internal: false`, `forwarded_for: None`, fresh
`request_id`, deadline, registration bundle's `composition_authority` /
`capabilities` / `scoped_env`. The security axis is provably identical between
`invoke()` and `invoke_streaming()` because they share `build_root_context`.

### deadline: None for streaming

The spec says `deadline: None` for subscriptions (unbounded). `build_root_context`
sets `deadline: Some(now + 30s)`. For the streaming path, set
`context.deadline = None` after `build_root_context`, OR add a streaming flag to
`build_root_context`. Coordinate with `call/protocol/dispatch-streaming-branch`
which has the same concern — extract a shared approach (e.g., a
`build_root_context_streaming` variant or a `deadline: None` override). The
gateway and the call dispatcher both need `deadline: None` for subscriptions;
don't duplicate the logic.

### What this task does NOT do

- **No `/subscribe` SSE wiring.** That's `http/server/subscribe-sse-streaming`.
- **No `OperationRegistry` changes.** `invoke_streaming()` is provided by
  `call/registry/invoke-streaming`.
- **No `to_mcp` changes.** MCP excludes `Subscription` (ADR-041); `to_mcp` never
  calls `invoke_streaming()`.

## Acceptance Criteria

- [ ] `GatewayDispatch::invoke_streaming()` method exists
- [ ] Returns `BoxStream<'static, ResponseEnvelope>` (or `ResponseStream` —
      same type)
- [ ] Builds root `OperationContext` via `build_root_context` (same as `invoke()`)
- [ ] Root context has `internal: false`, `forwarded_for: None`, fresh
      `request_id`
- [ ] `handler_identity`, `capabilities`, `scoped_env` from registration bundle
      (same as `invoke()`)
- [ ] `deadline: None` for the streaming path (unbounded subscriptions)
- [ ] Calls `OperationRegistry::invoke_streaming()`
- [ ] Security invariants identical to `invoke()` (shared `build_root_context`)
- [ ] Unit test: `invoke_streaming()` on a registered `Subscription` op returns
      the handler's stream
- [ ] Unit test: `invoke_streaming()` on unknown op returns a stream yielding
      one `NOT_FOUND`
- [ ] Unit test: `invoke_streaming()` on Internal op from external returns
      stream yielding one `NOT_FOUND` (not leaked)
- [ ] Unit test: `invoke_streaming()` with `None` identity + restricted op
      returns stream yielding one `FORBIDDEN`
- [ ] Unit test: `invoke_streaming()` on a `Query` op returns stream yielding
      one `INVALID_OPERATION_TYPE`
- [ ] Unit test: `invoke()` (existing) on a `Subscription` op returns
      `INVALID_OPERATION_TYPE` (verifies the guard from
      `streaming-handler-handlerkind` holds through the gateway)
- [ ] `cargo test -p alknet-http` succeeds
- [ ] `cargo clippy -p alknet-http --all-targets` succeeds with no warnings
- [ ] `cargo fmt --check -p alknet-http` passes

## References

- docs/architecture/decisions/049-streaming-handler-for-subscriptions.md — ADR-049 §7 (GatewayDispatch::invoke_streaming)
- docs/architecture/crates/http/http-server.md — §Streaming projection (invoke_streaming security invariants identical to invoke)
- docs/architecture/crates/call/operation-registry.md — §OperationRegistry (invoke_streaming)

## Notes

> The security axis MUST be provably identical between `invoke()` and
> `invoke_streaming()` — they share `build_root_context`. The two methods
> diverge only on the return shape. `deadline: None` for subscriptions is a
> shared concern with `call/protocol/dispatch-streaming-branch` — extract a
> shared approach (a streaming-variant of `build_root_context` or a
> `deadline: None` override) rather than duplicating. `to_mcp` never calls
> `invoke_streaming()` (MCP excludes `Subscription` — ADR-041); do not add
> streaming to the MCP gateway. The `futures` crate is already a dependency of
> `alknet-http`.

## Summary

> Added GatewayDispatch::invoke_streaming() returning BoxStream<ResponseEnvelope>. Security axis provably identical to invoke() via shared build_root_context_inner(bounded: bool); extracted build_root_context_streaming for deadline: None (unbounded subscriptions). Calls OperationRegistry::invoke_streaming(). to_mcp untouched. Added 9 unit tests (all error paths + streaming dispatch + deadline: None verification). 243 tests pass.