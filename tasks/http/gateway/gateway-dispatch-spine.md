---
id: http/gateway/gateway-dispatch-spine
name: Implement GatewayDispatch shared dispatch spine (thin concrete struct, not a trait)
status: completed
depends_on: [http/crate-init]
scope: narrow
risk: medium
impact: component
level: implementation
---

## Description

Implement the shared dispatch spine for the `to_*` gateway projections
(`to_openapi`, `to_mcp`) in `src/gateway/dispatch.rs`. This is the thin
shared core recommended by the gateway-factoring research: a **concrete
struct, not a trait**. It holds `Arc<OperationRegistry>` +
`Arc<dyn IdentityProvider>` and exposes a `resolve_bearer()` +
`invoke()` method pair returning the neutral `ResponseEnvelope`. Both
gateways call it as a library function; each gateway then maps the
`ResponseEnvelope` to its own wire shape.

This is the security-relevant shared piece: identity resolution, root
`OperationContext` construction, and the `OperationRegistry::invoke()`
call. A divergence here (one gateway resolving identity differently, or
building `OperationContext` with a different `internal` flag, or mapping
`CallError` inconsistently) would be a real security/correctness bug.
Extracting the spine now makes the two gateways *provably* identical on
the security-relevant axis (auth, authority, ACL) and lets them diverge
only on the wire-framing axis (where divergence is correct).

### The struct (research §5.1)

```rust
/// Shared dispatch spine for the `to_*` gateway projections.
/// Resolves identity, builds a root OperationContext, invokes the registry,
/// returns the neutral ResponseEnvelope. Each gateway maps the envelope to
/// its own wire shape.
pub struct GatewayDispatch {
    registry: Arc<OperationRegistry>,
    identity_provider: Arc<dyn IdentityProvider>,
}

impl GatewayDispatch {
    pub fn new(
        registry: Arc<OperationRegistry>,
        identity_provider: Arc<dyn IdentityProvider>,
    ) -> Self { ... }

    /// Resolve a bearer token to an Identity (shared by both gateways'
    /// axum auth middleware).
    pub fn resolve_bearer(&self, token: &AuthToken) -> Option<Identity> {
        self.identity_provider.resolve_from_token(token)
    }

    /// Invoke an operation as a wire-ingress caller. `internal: false`,
    /// `forwarded_for: None`, fresh request_id. Returns the neutral
    /// ResponseEnvelope; the gateway maps it to its wire shape.
    pub async fn invoke(
        &self,
        identity: Option<Identity>,
        op: &str,
        input: Value,
    ) -> ResponseEnvelope {
        // build root OperationContext, call self.registry.invoke(op, input, ctx)
        // ...
    }
}
```

### Root OperationContext construction

The `invoke()` method builds a root `OperationContext` for a wire-ingress
call (the same shape `CallAdapter::build_root_context` builds for
`alknet/call` wire requests):

- `internal: false` — ACL runs against the caller's `identity`, not a
  handler's composition authority (ADR-015).
- `forwarded_for: None` — wire-ingress only (ADR-032).
- `identity` = the resolved bearer identity (from `resolve_bearer`).
- `handler_identity` = the registration bundle's `composition_authority`.
- `capabilities` = the registration bundle's capabilities.
- `scoped_env` = the registration bundle's `scoped_env` (or empty).
- `request_id` = fresh UUID v4 (`generate_request_id()`).
- `deadline` = `now + default_timeout` (30s default).
- `env` = a `LocalOperationEnv` over the registry (the gateway dispatch
  path does not compose peer/session overlays — it is a flat invoke).

Coordinate with the existing `Dispatcher::build_root_context` in
alknet-call (`protocol/dispatch.rs`): if that logic can be extracted as a
shared free function (it should be — it takes `identity`, `capabilities`,
`env`, `deadline` and returns an `OperationContext`), call it from both
`Dispatcher` and `GatewayDispatch`. If it is tangled with
`CallAdapter`-specific state, duplicate the construction logic here (the
invariants — `internal: false`, `forwarded_for: None` — are the
load-bearing part; the construction itself is mechanical). See research
§6 open question #1.

### What this task does NOT do

- **No `GatewayDispatch` trait.** A concrete struct, not a polymorphic
  trait. The research (§5.2) rules this out: a trait would need an
  associated output type (HTTP `Response` vs `CallToolResult`), at which
  point it has no shared method bodies.
- **No `into_wire()` method.** The `ResponseEnvelope` → wire mapping is
  per-gateway; do not parameterize the core over it.
- **No streaming abstraction.** `/subscribe` SSE is `to_openapi`-only; do
  not build a `GatewayStream` trait for one implementation.
- **No discovery abstraction.** `services/list` is the shared backend
  (already in `OperationRegistry`); the discovery *framing* (OpenAPI
  `/search` vs MCP `tools/list` + `search` tool) is per-gateway.
- **No versioning.** `info.version` is `to_openapi`-only.
- **No `batch` method.** `batch` is a loop over `invoke()` in each
  gateway (research §6 open question #3 — confirm `batch` is genuinely
  just a loop, no shared batch-specific state).

### `services/list` / `services/schema` dispatch

The gateway's `search`/`schema` endpoints/tools dispatch `services/list`
and `services/schema` — these are registered operations in the
`OperationRegistry`, so `GatewayDispatch::invoke()` handles them
unchanged (it calls `OperationRegistry::invoke()`, which works for
`services/list` and `services/schema`). The `AccessControl`-filtered
listing lives in the `services/list` handler (already in the registry),
not in the gateway. Confirm via a spike that the filtering sees the
*caller's* identity when invoked through `GatewayDispatch::invoke` (it
should — `services/list` is `AccessControl::check(identity)`-filtered,
and `GatewayDispatch` passes the resolved identity as the caller). See
research §6 open question #4.

## Acceptance Criteria

- [ ] `GatewayDispatch` struct defined in `src/gateway/dispatch.rs`
- [ ] Holds `Arc<OperationRegistry>` + `Arc<dyn IdentityProvider>`
- [ ] `resolve_bearer(&self, token: &AuthToken) -> Option<Identity>` delegates to `identity_provider.resolve_from_token`
- [ ] `invoke(&self, identity, op, input) -> ResponseEnvelope` builds root context and dispatches
- [ ] Root `OperationContext` has `internal: false`, `forwarded_for: None`, fresh `request_id`
- [ ] `handler_identity` from registration bundle's `composition_authority`
- [ ] `capabilities` from registration bundle
- [ ] `scoped_env` from registration bundle (or empty)
- [ ] `deadline` = `now + 30s` default
- [ ] `invoke()` calls `OperationRegistry::invoke(op, input, ctx)`
- [ ] `invoke()` works for `services/list` and `services/schema` (registered ops)
- [ ] `AccessControl`-filtering in `services/list` sees the caller's resolved identity
- [ ] No `GatewayDispatch` trait (concrete struct only)
- [ ] No `into_wire()` method (per-gateway mapping stays out of the core)
- [ ] No streaming abstraction (per-gateway)
- [ ] `GatewayDispatch` is `pub` and re-exported from `lib.rs`
- [ ] Unit test: `invoke()` dispatches a registered op and returns `ResponseEnvelope`
- [ ] Unit test: `invoke()` for `services/list` returns AccessControl-filtered list
- [ ] Unit test: `invoke()` for unregistered op returns `CallError { code: NOT_FOUND }`
- [ ] Unit test: `invoke()` for Internal op returns `CallError { code: NOT_FOUND }` (not leaked)
- [ ] Unit test: `invoke()` with `None` identity + restricted op → `FORBIDDEN`
- [ ] `cargo test -p alknet-http` succeeds
- [ ] `cargo clippy -p alknet-http --all-targets` succeeds with no warnings

## References

- docs/research/alknet-http-gateway-factoring/findings.md — research recommendation (thin shared core, not a trait)
- docs/architecture/crates/http/http-adapters.md — to_openapi dispatch (§"Shared dispatch spine with to_mcp")
- docs/architecture/crates/http/http-mcp.md — to_mcp dispatch (§"Shared dispatch spine with to_openapi")
- docs/architecture/crates/call/operation-registry.md — OperationRegistry::invoke(), OperationContext construction
- docs/architecture/decisions/015-privilege-model-and-authority-context.md — ADR-015 (internal: false for wire)
- docs/architecture/decisions/032-forwarded-for-identity.md — ADR-032 (forwarded_for: None for wire-ingress)

## Notes

> The dispatch spine is the security-relevant shared piece. A divergence
> here (identity resolution, context construction, invoke shape) would be
> a security bug; extracting the spine now makes the two gateways provably
> identical on the security axis. The research recommends a concrete
> struct, not a trait — a trait would need an associated output type
> (HTTP Response vs CallToolResult), at which point it has no shared
> method bodies. Coordinate with the existing
> `Dispatcher::build_root_context` in alknet-call: if it can be extracted
> as a shared free function, call it from both Dispatcher and
> GatewayDispatch; otherwise duplicate the construction logic (the
> invariants are the load-bearing part). The `batch` endpoint is a loop
> over `invoke()` in each gateway, not a shared method.

## Summary

> GatewayDispatch concrete struct implemented in src/gateway/dispatch.rs. Holds
> Arc<OperationRegistry> + Arc<dyn IdentityProvider>. resolve_bearer() delegates
> to identity_provider.resolve_from_token. invoke() builds root OperationContext
> (internal:false, forwarded_for:None, fresh UUID v4 request_id, deadline now+30s,
> registration bundle's composition_authority/capabilities/scoped_env) and calls
> OperationRegistry::invoke. Duplicated build_root_context construction (alknet-call
> version is pub(crate) + tangled with CallConnection overlays; invariants identical).
> 14 unit tests covering dispatch, services/list AccessControl filtering, NOT_FOUND
> for unregistered/Internal ops, FORBIDDEN for unauthorized, concrete-struct-not-trait.
> Re-exported from lib.rs. Build/clippy/test all pass.