---
id: call/review-call-sync
name: Review alknet-call ADR-029/030/032/034 sync for spec conformance
status: completed
depends_on: [call/operation-env-invoke-peer, call/services-list-accesscontrol-filtered, call/call-client-verifier-selection, call/from-call-forwarded-for, call/dispatch-peer-identity]
scope: broad
risk: low
impact: phase
level: review
---

## Description

Review the alknet-call implementation after the ADR-029/030/032/034 sync for
spec conformance, pattern consistency, and correctness. This is the quality
checkpoint at the end of the call phase — the most complex sync in this batch.

### Review Checklist

1. **remote_safe/trusted_peer retirement** (ADR-029 §3):
   - No `remote_safe` field on `HandlerRegistration`
   - No `trusted_peer` on `CallClient`
   - No `RemoteFilter` in dispatch
   - No `list_operations_peer_scoped` / `services_list_handler_peer_scoped`
   - No ADR-028 test remnants
   - Peer authorization via `AccessControl::check(peer_identity)` only

2. **PeerCompositeEnv conformance** (ADR-029 §1, ADR-030 §4-5):
   - `PeerCompositeEnv` with peer-keyed `connections: HashMap<PeerId, ...>`
   - `connection_order: Vec<PeerId>` (insertion order for `PeerRef::Any`)
   - `PeerId = Identity.id` (stable `peer_id`, not UUID)
   - `compose_root_env` builds `PeerCompositeEnv` per call
   - Connection with no resolved identity → not attached (no PeerId)
   - `CompositeOperationEnv` removed (all call sites migrated)
   - Singular-connection case works (degenerate single-entry map)

3. **invoke_peer / PeerRef conformance** (ADR-029 §2):
   - `PeerRef::Specific(PeerId)` / `PeerRef::Any` enum
   - `OperationEnv::invoke_peer` with default-impl (back-compat)
   - `OperationEnv::peer_contains` with default-impl
   - `PeerCompositeEnv` overrides with real peer-keyed routing
   - `PeerRef::Specific` → no fallthrough (NOT_FOUND if peer doesn't serve op)
   - `PeerRef::Any` → insertion-order first-match
   - Reachability check (`scoped_env.allows`) gates before routing

4. **forwarded_for conformance** (ADR-032):
   - `OperationContext.forwarded_for: Option<Identity>` field
   - `build_root_context` populates from `call.requested.forwarded_for`
   - `OperationEnv::invoke` sets `None` for composed children (wire-ingress only)
   - `AccessControl::check` never reads `forwarded_for` (structural invariant)
   - `from_call` handler populates `forwarded_for` from `context.identity`

5. **services/list AccessControl-filtered** (ADR-029 §6):
   - `services/list` filters by `AccessControl::check(calling_peer_identity)`
   - Op with `AccessControl::default()` listed to any peer
   - Op with `required_scopes` hidden from unauthorized peers
   - `services/list-peers` opt-in (peer-attributed, AccessControl-filtered)
   - `services/schema` unchanged

6. **CallClient verifier selection** (OQ-29, ADR-034):
   - Client presents Ed25519 key as RFC 7250 raw public key client cert
   - `AcceptAnyServerCertVerifier` removed (security hole)
   - Verifier by `PeerEntry` presence: `Some` → fingerprint pin, `None` + X.509 → CA, `None` + Ed25519 → fail closed
   - `CallCredentials.remote_identity: None` is load-bearing (not placeholder)
   - No-env-vars invariant preserved (credentials from Capabilities)

7. **from_call peer-keyed registration** (ADR-029 §5):
   - `from_call` registers into peer's sub-overlay
   - Same-peer collision = error (`SamePeerCollision`)
   - Cross-peer collision dissolves (separate sub-overlays)
   - `namespace_prefix` defaults to `None` (optional local-naming sugar)
   - `forwarded_for` populated from hub's `context.identity`

8. **dispatch_peer_identity** (ADR-029 §3, ADR-030 §5):
   - `dispatch_requested` resolves peer `Identity`
   - `AccessControl::check(peer_identity)` is the sole authorization gate
   - `PeerId` from `connection.identity().id` (not UUID)
   - Connection with no identity → no PeerId, not in PeerCompositeEnv

9. **ADR conformance**:
   - ADR-029: peer-keyed overlays, PeerRef routing, AccessControl-based peer auth, retire remote_safe
   - ADR-030: PeerId = Identity.id = PeerEntry.peer_id (stable)
   - ADR-032: forwarded_for (metadata only, wire-ingress only, never in ACL)
   - ADR-034: three remote roles, verifier selection by PeerEntry presence

10. **Security constraints**:
    - Capabilities non-serializable, zeroized, immutable (unchanged)
    - No secret material on wire (unchanged)
    - `forwarded_for` is metadata, not authority (AccessControl::check never reads it)
    - Internal ops → NOT_FOUND from wire (unchanged)
    - Reachability check bounds composition (unchanged)
    - No-env-vars invariant (credentials from Capabilities, not env vars)
    - `AcceptAnyServerCertVerifier` removed (no security hole for X.509)

11. **Pattern consistency**:
    - `OperationEnv` is a trait (not concrete) — preserved
    - `PeerCompositeEnv` uses `contains()` probe before dispatch — preserved
    - Authority switch on `internal: true` — preserved
    - Deadline inheritance — preserved
    - Metadata fresh on composition (`HashMap::new()`) — preserved

12. **Test coverage**:
    - PeerCompositeEnv routing (session, peers in order, base fallthrough)
    - PeerRef::Specific (routes, NOT_FOUND on missing peer)
    - PeerRef::Any (insertion-order first-match)
    - forwarded_for (populated, None for composed children, never in ACL)
    - services/list AccessControl-filtered
    - AccessControl::check peer authorization (allowed, FORBIDDEN, Internal NOT_FOUND)
    - CallClient verifier selection (fingerprint pin, CA, fail closed)
    - from_call forwarded_for + peer-keyed registration + collision rules

## Acceptance Criteria

- [ ] No `remote_safe`/`trusted_peer`/`RemoteFilter` references remain
- [ ] `PeerCompositeEnv` matches ADR-029 §1 (peer-keyed, insertion order)
- [ ] `PeerId = Identity.id` (stable, not UUID)
- [ ] `invoke_peer`/`peer_contains`/`PeerRef` match ADR-029 §2
- [ ] `forwarded_for` matches ADR-032 (metadata only, wire-ingress only, never in ACL)
- [ ] `services/list` AccessControl-filtered; `services/list-peers` opt-in
- [ ] CallClient verifier selection matches ADR-034 §3
- [ ] `from_call` peer-keyed registration + forwarded_for + collision rules
- [ ] `AccessControl::check(peer_identity)` is the sole authorization gate
- [ ] `AcceptAnyServerCertVerifier` removed
- [ ] No-env-vars invariant preserved
- [ ] `OperationEnv` is a trait (not concrete)
- [ ] Test coverage adequate for all new functionality
- [ ] `cargo fmt --check -p alknet-call` passes
- [ ] `cargo clippy -p alknet-call` passes with no warnings
- [ ] All tests pass

## References

- docs/architecture/crates/call/README.md
- docs/architecture/crates/call/call-protocol.md
- docs/architecture/crates/call/operation-registry.md
- docs/architecture/crates/call/client-and-adapters.md
- docs/architecture/decisions/029-peer-graph-routing-model.md
- docs/architecture/decisions/030-peerentry-and-identity-id-decoupling.md
- docs/architecture/decisions/032-forwarded-for-identity.md
- docs/architecture/decisions/034-outgoing-only-x509-and-three-peer-roles.md

## Notes

> This is the most complex sync in this batch. The review should verify that
> the peer-keyed routing model (ADR-029), the stable PeerId (ADR-030), the
> forwarded_for metadata field (ADR-032), and the verifier selection (ADR-034)
> all work correctly together. The `OperationEnv` trait-object design is
> load-bearing — verify it's still a trait, not concrete. The
> `AccessControl::check`-based peer authorization is the security property —
> verify no parallel gate remains. If deviations are found, document and fix
> before considering the call sync complete.

## Summary

All 12 checklist sections PASS. Negative criteria verified: no remote_safe/trusted_peer/RemoteFilter/list_operations_peer_scoped/AcceptAnyServerCertVerifier/CompositeOperationEnv in code. PeerCompositeEnv matches ADR-029 §1. PeerId = Identity.id (stable). invoke_peer/PeerRef match ADR-029 §2. forwarded_for matches ADR-032. services/list filters by AccessControl::check. CallClient verifier selection matches ADR-034 §3. from_call peer-keyed registration with SamePeerCollision. dispatch_peer_identity: AccessControl::check is sole gate. FIXES APPLIED: ran cargo fmt to fix 3 drifts in adapter.rs (2 spots) and env.rs (1 spot) — pure formatting, no semantic change. No large deviations — implementation is conformant with ADR-029/030/032/034. 389 tests pass, clippy clean.