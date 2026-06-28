---
id: core/review-core-sync
name: Review alknet-core ADR-029/030/031/034/035 sync for spec conformance
status: completed
depends_on: [core/credential-store-trait, core/identity-store-trait, core/config-identity-provider-peerentry, core/fingerprint-normalization, core/three-remote-roles-docs]
scope: moderate
risk: low
impact: phase
level: review
---

## Description

Review the alknet-core implementation after the ADR-029/030/031/034/035 sync
for spec conformance, pattern consistency, and correctness. This is the quality
checkpoint at the end of the core phase — before alknet-call (which depends on
the new `Identity.id = peer_id` semantics) begins its sync.

### Review Checklist

1. **PeerEntry / AuthPolicy conformance** (config.md, auth.md, ADR-030):
   - `PeerEntry` has all 7 fields (peer_id, fingerprints, auth_token_hash, scopes, resources, display_name, enabled)
   - `AuthPolicy.authorized_fingerprints` removed; `peers: Vec<PeerEntry>` in place
   - `AuthPolicy.api_keys` unchanged
   - `resolve_identity_from_fingerprint` resolves fingerprint → PeerEntry → `Identity { id: peer_id }`
   - `resolve_identity_from_token` resolves auth_token_hash → PeerEntry → falls through to ApiKeyEntry
   - `Identity.id` is the stable `peer_id`, not the fingerprint
   - Disabled peers (`enabled: false`) return None
   - Duplicate `peer_id` validation

2. **ConfigIdentityProvider conformance** (auth.md, ADR-030):
   - `resolve_from_fingerprint` delegates to `AuthPolicy::resolve_identity_from_fingerprint`
   - `resolve_from_token` delegates to `AuthPolicy::resolve_identity_from_token` (PeerEntry first, ApiKeyEntry fall-through)
   - Reads from ArcSwap on every call (hot-reloadable — unchanged)
   - Does NOT implement `IdentityStore`

3. **CredentialStore conformance** (auth.md, ADR-031/035):
   - `CredentialStore` trait with sync `get`, async `put`/`delete`
   - `InMemoryCredentialStore` default adapter (async put/delete with no .await points)
   - `EncryptedData` core mirror (4 fields, serializable, no vault dep)
   - `StoreError` enum (`#[non_exhaustive]`, thiserror, 3 variants)
   - No `list` method
   - No vault dependency added to core

4. **IdentityStore conformance** (auth.md, ADR-035):
   - `IdentityStore: IdentityProvider` supertrait
   - `put_peer`/`update_peer`/`remove_peer` all async
   - `ConfigIdentityProvider` does NOT implement it
   - `IdentityProvider` trait unchanged (read-only, sync)

5. **Fingerprint normalization conformance** (auth.md, ADR-030 §6):
   - Ed25519 raw key (SPKI) → `ed25519:<lowercase hex of 32 bytes>`
   - X.509 cert → `SHA256:<hex of DER>` (unchanged)
   - iroh path → `ed25519:<hex>` (unchanged)
   - Same key, same fingerprint across quinn and iroh
   - No-client-cert → None (no regression)

6. **Three remote roles documentation** (ADR-034):
   - `auth.rs` comments document the three roles and verifier selection rule
   - `endpoint.rs` comments clarify server-side vs client-side verifier concerns

7. **Pattern consistency**:
   - ArcSwap used consistently for DynamicConfig (unchanged)
   - Repo/adapter pattern consistent (trait + in-memory default, no backend dep in core)
   - No russh dependency in core (unchanged)
   - Feature flags (quinn, iroh) gate transport code correctly

8. **Security constraints**:
   - `PeerEntry.enabled: false` → resolution returns None (revoked peers)
   - `StoreError` is `#[non_exhaustive]`
   - `EncryptedData` carries no plaintext (encrypted blob only)
   - No env vars in the credential path (ADR-014 invariant preserved)

9. **Test coverage**:
   - PeerEntry resolution (fingerprint, auth_token_hash, ApiKeyEntry fall-through)
   - Multi-fingerprint PeerEntry
   - Disabled peer → None
   - Duplicate peer_id validation
   - CredentialStore get/put/delete round-trip
   - EncryptedData serialization round-trip
   - Fingerprint normalization (Ed25519 → ed25519:, X.509 → SHA256:)
   - Config reload with PeerEntry model

## Acceptance Criteria

- [ ] All PeerEntry / AuthPolicy types match config.md and auth.md
- [ ] ConfigIdentityProvider resolution matches auth.md (PeerEntry multi-credential path)
- [ ] CredentialStore trait + InMemoryCredentialStore + EncryptedData + StoreError match ADR-031/035
- [ ] IdentityStore trait matches ADR-035 (read/write split, ConfigIdentityProvider posture)
- [ ] Fingerprint normalization matches ADR-030 §6 (ed25519: for raw keys, SHA256: for X.509)
- [ ] Three remote roles documented in source comments (ADR-034)
- [ ] No `authorized_fingerprints` references remain
- [ ] No `remote_safe`/`trusted_peer` references in core (those are call-side)
- [ ] ArcSwap pattern consistent
- [ ] No russh dependency, no vault dependency in core
- [ ] Test coverage adequate for all new functionality
- [ ] `cargo fmt --check -p alknet-core` passes
- [ ] `cargo clippy -p alknet-core` passes with no warnings
- [ ] All tests pass

## References

- docs/architecture/crates/core/README.md
- docs/architecture/crates/core/auth.md
- docs/architecture/crates/core/config.md
- docs/architecture/decisions/030-peerentry-and-identity-id-decoupling.md
- docs/architecture/decisions/031-credentialstore-repo-trait.md
- docs/architecture/decisions/033-storage-boundary-and-repo-adapter-pattern.md
- docs/architecture/decisions/034-outgoing-only-x509-and-three-peer-roles.md
- docs/architecture/decisions/035-concrete-persistence-adapter-shapes.md

## Notes

> This review verifies core is spec-conformant after the ADR-029/030/031/034/035
> sync before alknet-call begins its sync. alknet-call depends heavily on the new
> `Identity.id = peer_id` semantics (PeerCompositeEnv keys, PeerRef::Specific
> routing, AccessControl-based peer authorization) — any issues here propagate
> to call. If deviations are found, document and fix before proceeding to the
> call phase.

## Summary

All 9 review checklist sections passed (PeerEntry/AuthPolicy, ConfigIdentityProvider, CredentialStore, IdentityStore, fingerprint normalization, three-roles docs, pattern consistency, security constraints, test coverage). Verification: cargo build/clippy(-D warnings)/fmt --check all clean; 135 tests pass. Negative criteria confirmed: no authorized_fingerprints, no remote_safe/trusted_peer, no russh, no vault/honker/rusqlite/sqlx deps in core (honker appears only in a doc comment describing the SQLite adapter — not a dependency), no env::var, no list method, ConfigIdentityProvider does not implement IdentityStore. No fixes applied — implementation was conformant.