---
id: call/operation-context-forwarded-for
name: Add forwarded_for field to OperationContext and wire from call.requested (ADR-032)
status: completed
depends_on: [call/retire-remote-safe]
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Add the `forwarded_for: Option<Identity>` field to `OperationContext` and wire
it from the `call.requested` payload. Per ADR-032, `forwarded_for` is metadata
only — `AccessControl::check` never reads it; the ACL always authorizes the
direct caller's `identity`. This is the wire-format one-way door folded into
the `OperationContext` edit window (ADR-032 says this is the cheapest point
since `OperationContext` is under edit for the ADR-029 migration).

### OperationContext change

```rust
pub struct OperationContext {
    // ... existing fields ...
    pub forwarded_for: Option<Identity>,  // NEW (ADR-032)
}
```

- `forwarded_for`: The original caller when this call was forwarded by a
  `from_call` handler (ADR-032). **Metadata only** — `AccessControl::check`
  never reads it; the ACL always authorizes `identity` (the direct caller).
  Handlers may read it for logging, auditing, per-user rate limiting, or
  application context. The forwarder's claim, not a verified identity — a
  malicious hub can lie (same property as HTTP `X-Forwarded-For`).

### Wire format (call.requested payload)

The `call.requested` payload already carries `operationId`, `input`, and
optional `auth_token`. Add optional `forwarded_for`:

```json
{
  "operationId": "/docker/start",
  "input": { ... },
  "auth_token": "alk_...",
  "forwarded_for": {           // optional (ADR-032)
    "id": "alice",
    "scopes": ["fs:read", "docker:start"],
    "resources": {}
  }
}
```

The payload is a `serde_json::Value` (not a typed struct), so `forwarded_for`
is read from `payload.get("forwarded_for")` and deserialized into `Identity`.

### build_root_context wiring

`Dispatcher::build_root_context` reads `forwarded_for` from the payload and
populates the field:

```rust
fn build_root_context(
    &self,
    request_id: String,
    operation_name: &str,
    identity: Option<Identity>,
    forwarded_for: Option<Identity>,  // NEW parameter
    connection: &CallConnection,
) -> OperationContext {
    // ...
    OperationContext {
        // ...
        forwarded_for,  // from call.requested.forwarded_for
        // ...
    }
}
```

`dispatch_requested` extracts `forwarded_for` from the payload:

```rust
let forwarded_for = payload.get("forwarded_for")
    .and_then(|v| serde_json::from_value::<Identity>(v.clone()).ok());
```

### OperationEnv::invoke sets None for composed children

`forwarded_for` is **wire-ingress only** (ADR-032 Assumption 3). Composed
children (calls via `OperationEnv::invoke`) do NOT inherit `forwarded_for` —
they get `None`. The field is populated from `call.requested.forwarded_for` by
the dispatch path, and the `from_call` forwarding handler sets it when
constructing the forwarded payload (that's `call/from-call-forwarded-for`).

In `LocalOperationEnv::invoke_with_policy` (and `PeerCompositeEnv` when built):

```rust
let context = OperationContext {
    // ...
    forwarded_for: None,  // composed children do not inherit (ADR-032)
    // ...
};
```

### AccessControl::check never reads forwarded_for

The security property is structural: `AccessControl::check` takes
`Option<&Identity>` (the direct caller's `identity`). The `forwarded_for` field
is `Option<Identity>` on `OperationContext`, but the check signature doesn't
accept it. Verify this invariant holds — no code path passes `forwarded_for`
to `AccessControl::check`.

## Acceptance Criteria

- [ ] `OperationContext.forwarded_for: Option<Identity>` field added
- [ ] `build_root_context` accepts `forwarded_for` parameter and populates the field
- [ ] `dispatch_requested` extracts `forwarded_for` from `payload.get("forwarded_for")`
- [ ] `forwarded_for` deserialized from JSON `{ id, scopes, resources }` into `Identity`
- [ ] `OperationEnv::invoke_with_policy` sets `forwarded_for: None` for composed children
- [ ] `AccessControl::check` never reads `forwarded_for` (verify no code path passes it)
- [ ] Missing `forwarded_for` in payload → `None` (no error)
- [ ] Unit test: `forwarded_for` populated from payload
- [ ] Unit test: missing `forwarded_for` → None
- [ ] Unit test: composed children get `forwarded_for: None`
- [ ] Unit test: `AccessControl::check` still uses `identity` (not `forwarded_for`)
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call` succeeds with no warnings

## References

- docs/architecture/crates/call/operation-registry.md — OperationContext.forwarded_for
- docs/architecture/crates/call/call-protocol.md — call.requested payload, build_root_context
- docs/architecture/decisions/032-forwarded-for-identity.md — ADR-032

## Notes

> `forwarded_for` is a wire-format one-way door (ADR-032) — folded into the
> OperationContext edit window because ADR-029 is already rewriting the
> composition env. The field is metadata only: `AccessControl::check` never
> reads it; the ACL always authorizes the direct caller's `identity`. The
> `from_call` handler populates it when forwarding (that's
> `call/from-call-forwarded-for`). Composed children get `None` (wire-ingress
> only, not composition-ingress).

## Summary

Added forwarded_for: Option<Identity> field to OperationContext (with ADR-032 doc comment) and wired it through the dispatch path: Dispatcher::build_root_context accepts a forwarded_for parameter, dispatch_requested extracts it from payload.get("forwarded_for") via serde_json::from_value::<Identity> (missing or malformed → None, no error), and LocalOperationEnv/OverlayOperationEnv invoke_with_policy set forwarded_for: None for composed children (wire-ingress only). Added Serialize/Deserialize derives to alknet_core::auth::Identity so it can be deserialized from the JSON payload. Updated all OperationContext literal sites (production + tests + integration test). AccessControl::check signature unchanged — still takes Option<&Identity> (the direct caller); no code path passes forwarded_for to it (verified structurally). 8 unit tests covering: build_root_context populates/omits forwarded_for, dispatch_requested populates from payload / None when missing / None when malformed / does not satisfy ACL when only forwarded_for has the scope / present alongside satisfied ACL, and composed children get None. Coordinator fixed 5 cross-task test sites (adapter.rs + discovery.rs) where OperationContext literals from peer-composite-env and services-list tasks needed forwarded_for: None. 213 unit + 2 integration tests pass, clippy clean, fmt clean.