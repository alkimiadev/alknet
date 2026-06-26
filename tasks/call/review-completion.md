---
id: call/review-completion
name: Review alknet-call client/adapter completion for spec conformance (ADR-017, ADR-028) and no-env-vars invariant
status: pending
depends_on: [call/client/from-call, call/client/operation-adapter-trait, call/client/from-jsonschema]
scope: broad
risk: low
impact: phase
level: review
---

## Description

Review the alknet-call client/adapter completion (the gap ADR-017 left to
implementation, now specced in `client-and-adapters.md`) for spec conformance,
security-constraint conformance, and pattern consistency. This is the quality
checkpoint at the end of the call-completion batch — the work that unblocks
every downstream consumer (runner, container service, bilateral exchange,
NAPI, agent cross-node dispatch).

### Review Checklist

1. **CallClient conformance** (client-and-adapters.md §CallClient):
   - `CallClient` struct with registry, identity_provider, trusted_peer
   - `new()` constructs default-deny (trusted_peer: false)
   - `trusted_peer()` constructs trusted-peer mode (explicit opt-in)
   - `connect()` opens QUIC on ALPN `alknet/call`, returns CallConnection
   - Dispatch loop is **shared** with CallAdapter (refactored, not duplicated — ADR-017 §1)
   - Connection symmetry (ADR-017 §2): both sides call each other after establishment
   - Credentials from Capabilities, not env vars (ADR-014, no-env-vars invariant)

2. **Peer-scoped filtering conformance** (ADR-028):
   - Default-deny: op with `remote_safe == false` returns NOT_FOUND to remote peer
   - Default-deny: `OperationContext.capabilities` populated only for remote-safe ops
   - Trusted-peer mode: all External ops dispatch regardless of remote_safe
   - services/list hides non-remote-safe ops in default mode (ADR-028 Assumption 2)
   - services/list lists all External ops in trusted-peer mode
   - Dispatch-time read over single Layer-0 registry (not a copy — ADR-028 §5)
   - `remote_safe` defaults false across all provenance (ADR-028 §4)

3. **from_call conformance** (client-and-adapters.md §from_call):
   - Calls services/list then services/schema for each op
   - Constructs HandlerRegistration with `provenance: FromCall`
   - Forwarding handler sends call.requested via CallConnection
   - Subscription forwarding yields until completed/aborted
   - `composition_authority: None`, `scoped_env: None` (leaf — ADR-022)
   - `remote_safe: false` on FromCall leaves
   - Namespace collision = error (DC-3/OQ-28), not silent overwrite
   - Re-import on connection establishment (DC-2/OQ-27, v1 default)
   - Cross-node abort via parent_request_id (ADR-016 §6)

4. **OperationAdapter trait conformance** (client-and-adapters.md §OperationAdapter):
   - `async fn import(&self) -> Result<Vec<HandlerRegistration>, AdapterError>`
   - Trait is `#[async_trait]` (async — ADR-017 §5, locked)
   - `AdapterError` is `#[non_exhaustive]` + `thiserror::Error`
   - Variants: DiscoveryFailed, SchemaParse, Transport, Unauthorized, Conflict
   - Trait lives in alknet-call (where the types live), not alknet-http

5. **from_jsonschema conformance** (client-and-adapters.md §from_jsonschema):
   - `provenance: FromJsonSchema`, no real handler (placeholder errors if invoked)
   - `composition_authority: None`, `scoped_env: None`, empty capabilities
   - `remote_safe: false` (provenance default, ADR-028 §4)
   - Implements OperationAdapter, no .await in import (pure parse)
   - Malformed schema → `AdapterError::SchemaParse`

6. **Adapter location map conformance** (client-and-adapters.md §Adapter Location Map):
   - OperationAdapter trait + from_call + from_jsonschema + CallClient in alknet-call
   - No HTTP client / HTTP server deps in alknet-call (stays lean)
   - from_openapi/from_mcp/to_openapi/to_mcp NOT in alknet-call (deferred to alknet-http)
   - MCP stdio not built (security position, not a feature gap)

7. **No-env-vars invariant** (client-and-adapters.md §No-Env-Vars Invariant):
   - Credential path: vault → assembly → Capabilities → HandlerRegistration.capabilities → OperationContext.capabilities → handler
   - No handler reads outbound credentials from any source other than OperationContext.capabilities
   - No `std::env::var` reads in the credential path
   - The invariant is enforced by the dispatch path (build_root_context), not runtime convention

8. **ADR conformance (completion-specific)**:
   - ADR-017 §1: shared dispatch loop, CallClient own registry (now peer-scoped per ADR-028)
   - ADR-017 §2: connection direction independent of call direction
   - ADR-017 §3: from_call flow (services/list + services/schema), FromCallConfig prefix/filter
   - ADR-017 §5: async trait, bundles not (spec,handler) pairs, to_* are projections not impls
   - ADR-017 §6: cross-node abort cascade through from_call handler
   - ADR-017 §7: credentials from capabilities (TLS identity, auth token, remote identity)
   - ADR-028: default-deny, remote_safe bool, trusted-peer opt-in, dispatch-time read, services/list hide

9. **Security constraints (completion-specific)**:
   - Default-deny filtering: remote peer can't trigger capability exposure for non-remote-safe ops
   - Trusted-peer opt-in is explicit, never default
   - Capabilities non-serializable, never cross the wire (ADR-014)
   - from_call trust is transitive (remote node's code runs) — recorded in spec, not enforced beyond scoped env
   - FromCall/FromJsonSchema leaves have no composition authority (can't escalate)

10. **Test coverage**:
    - Integration test: two-node call (CallClient ↔ CallAdapter, both call each other)
    - Integration test: default-deny op → NOT_FOUND to remote peer
    - Integration test: remote_safe op dispatches to remote peer
    - Integration test: trusted-peer mode exposes all External ops
    - Integration test: services/list hides non-remote-safe in default mode
    - Integration test: from_call populates Layer 2 overlay, forwarding works
    - Integration test: subscription forwarding streams remote events
    - Integration test: namespace collision returns error
    - Integration test: cross-node abort cascades through from_call handler
    - Unit tests: AdapterError variants, OperationAdapter trait compiles

11. **Spec drift check**: verify `client-and-adapters.md` still matches the
    implementation after the completion (no spec/impl drift introduced during
    implementation). In particular: the CallClient struct sketch, the
    CallCredentials sketch, the FromCallConfig fields, the AdapterError
    variants, and the remote_safe field on HandlerRegistration.

## Acceptance Criteria

- [ ] CallClient matches client-and-adapters.md (struct, new/trusted_peer, connect, shared loop)
- [ ] Peer-scoped filtering matches ADR-028 (default-deny, trusted-peer, services/list hide)
- [ ] from_call matches client-and-adapters.md (flow, FromCallConfig, provenance, None fields)
- [ ] OperationAdapter trait + AdapterError match client-and-adapters.md (async, non_exhaustive, variants)
- [ ] from_jsonschema matches client-and-adapters.md (provenance, placeholder handler, no I/O)
- [ ] Adapter location map respected (no HTTP deps in alknet-call; from_openapi/mcp not built here)
- [ ] No-env-vars invariant holds (credentials from Capabilities, no env-var reads)
- [ ] ADRs 017 + 028 conformed to (plus 014/015/016/022/023/024 where touched)
- [ ] Default-deny security constraint enforced (no capability exposure for non-remote-safe)
- [ ] Integration tests cover two-node call, default-deny, trusted-peer, from_call, abort cascade
- [ ] No spec/impl drift in client-and-adapters.md (or drift documented + spec amended)
- [ ] `cargo fmt --check -p alknet-call` passes
- [ ] `cargo clippy -p alknet-call --all-targets` passes with no warnings
- [ ] All tests pass

## References

- docs/architecture/crates/call/client-and-adapters.md — the spec being reviewed against
- docs/architecture/crates/call/README.md — crate index (now lists client-and-adapters.md)
- docs/architecture/decisions/017-call-protocol-client-and-adapter-contract.md — ADR-017 (amended)
- docs/architecture/decisions/028-callclient-peer-scoped-registry-filtering.md — ADR-028
- docs/architecture/open-questions.md — OQ-25..28 (two-way-door remainders — verify defaults match spec)
- docs/research/alknet-call-completion/gap-analysis.md — DC-1..4, the decisions this batch resolved
- tasks/call/registry/remote-safe-marking.md
- tasks/call/client/call-client.md
- tasks/call/client/from-call.md
- tasks/call/client/operation-adapter-trait.md
- tasks/call/client/from-jsonschema.md

## Notes

> This review closes the call-completion batch. The load-bearing security
> invariant is ADR-028's default-deny: a remote peer's call must not trigger
> dispatch that populates OperationContext.capabilities from the local
> registration bundle unless the op is explicitly remote-safe. Verify this
> with a test that asserts a non-remote-safe op's call does NOT populate
> capabilities (not just that it returns NOT_FOUND — the security argument is
> about capability exposure, not just call denial). The no-env-vars invariant
> (ADR-014) is the dispatch-side corollary: no handler reads credentials from
> any source other than OperationContext.capabilities. The shared dispatch
> loop (ADR-017 §1) is the architectural commitment that keeps CallClient from
> becoming a parallel protocol implementation — verify the loop is genuinely
> shared (refactored out of CallAdapter), not copy-pasted. If deviations are
> found, document and fix before considering the call-completion batch done.
> This unblocks every downstream consumer, so spec/impl drift here propagates.