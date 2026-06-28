---
id: core/config-identity-provider-peerentry
name: Rewrite ConfigIdentityProvider resolution to use PeerEntry multi-credential path (ADR-030)
status: completed
depends_on: [core/peer-entry-model]
scope: narrow
risk: medium
impact: component
level: implementation
---

## Description

Rewrite `ConfigIdentityProvider::resolve_from_fingerprint` and
`resolve_from_token` to use the new `PeerEntry`-based resolution from
`core/peer-entry-model`. This is the resolution-logic half of the ADR-030
change — the data model (PeerEntry struct, AuthPolicy.peers) lands in
`core/peer-entry-model`; this task updates the `ConfigIdentityProvider` methods
that delegate to `AuthPolicy`.

### Current state (pre-ADR-030)

```rust
impl IdentityProvider for ConfigIdentityProvider {
    fn resolve_from_fingerprint(&self, fingerprint: &str) -> Option<Identity> {
        let config = self.dynamic.load();
        config.auth.resolve_identity_from_fingerprint(fingerprint)
    }

    fn resolve_from_token(&self, token: &AuthToken) -> Option<Identity> {
        let config = self.dynamic.load();
        let token_str = String::from_utf8_lossy(&token.raw);
        config.auth.resolve_api_key(&token_str)  // ← only ApiKeyEntry path
    }
}
```

### Target state (ADR-030)

```rust
impl IdentityProvider for ConfigIdentityProvider {
    fn resolve_from_fingerprint(&self, fingerprint: &str) -> Option<Identity> {
        let config = self.dynamic.load();
        config.auth.resolve_identity_from_fingerprint(fingerprint)
        // Now resolves: fingerprint → PeerEntry → Identity { id: peer_id, ... }
    }

    fn resolve_from_token(&self, token: &AuthToken) -> Option<Identity> {
        let config = self.dynamic.load();
        let token_str = String::from_utf8_lossy(&token.raw);
        config.auth.resolve_identity_from_token(&token_str)
        // Now tries PeerEntry.auth_token_hash first, falls through to ApiKeyEntry
    }
}
```

The key change in `resolve_from_token`: it now calls
`AuthPolicy::resolve_identity_from_token` (added in `core/peer-entry-model`),
which tries the `PeerEntry.auth_token_hash` path first (token is one credential
path among several for a stable logical peer → `Identity.id = peer_id`), then
falls through to `resolve_api_key` (token IS the identity → `Identity.id =
prefix`).

### ConfigIdentityProvider stays read-only

`ConfigIdentityProvider` still reads from `ArcSwap<DynamicConfig>` on every call
(hot-reloadable). It does NOT implement `IdentityStore` (that's
`core/identity-store-trait` — config reload is its write path, not a method
call).

### Test migration

The existing auth.rs tests use `authorized_fingerprints: HashSet<String>` and
expect `Identity.id == fingerprint`. These must migrate to the `PeerEntry` model:
- `config_with_fingerprint` helper → `config_with_peer_entry` helper
- Fingerprint resolution tests expect `Identity.id == peer_id` (not the fingerprint)
- Add token-resolution-via-PeerEntry tests (auth_token_hash path)
- Config reload tests use `PeerEntry` in the new config

## Acceptance Criteria

- [ ] `ConfigIdentityProvider::resolve_from_fingerprint` delegates to `AuthPolicy::resolve_identity_from_fingerprint` (PeerEntry path)
- [ ] `ConfigIdentityProvider::resolve_from_token` delegates to `AuthPolicy::resolve_identity_from_token` (PeerEntry.auth_token_hash → fall through to ApiKeyEntry)
- [ ] `ConfigIdentityProvider` reads from ArcSwap on every call (hot-reloadable — unchanged)
- [ ] `ConfigIdentityProvider` does NOT implement `IdentityStore`
- [ ] Fingerprint resolution returns `Identity { id: peer_id, ... }` (stable, not the fingerprint)
- [ ] Token resolution: PeerEntry.auth_token_hash match → `Identity { id: peer_id }`; no match → ApiKeyEntry fall-through → `Identity { id: prefix }`
- [ ] All existing auth.rs tests migrated to PeerEntry model (no `authorized_fingerprints` references)
- [ ] Unit test: fingerprint resolution via PeerEntry (known → Some with peer_id, unknown → None)
- [ ] Unit test: token resolution via PeerEntry.auth_token_hash (matching → Some with peer_id)
- [ ] Unit test: token resolution falls through to ApiKeyEntry when no PeerEntry matches
- [ ] Unit test: config reload changes resolution results immediately (PeerEntry model)
- [ ] Unit test: disabled PeerEntry returns None
- [ ] `cargo test -p alknet-core` succeeds
- [ ] `cargo clippy -p alknet-core` succeeds with no warnings

## References

- docs/architecture/crates/core/auth.md — ConfigIdentityProvider, multi-credential resolution
- docs/architecture/crates/core/config.md — AuthPolicy.peers, PeerEntry
- docs/architecture/decisions/030-peerentry-and-identity-id-decoupling.md — ADR-030 §2 (resolution semantics)

## Notes

> This is the resolution-logic half of the ADR-030 change. The data model lands
> in `core/peer-entry-model`; this task wires `ConfigIdentityProvider` to the
> new `AuthPolicy` methods. The semantic shift: `Identity.id` changes from the
> fingerprint to the `peer_id` on the fingerprint path. The token path gains a
> new first-try (PeerEntry.auth_token_hash) before the existing ApiKeyEntry
> fall-through. ConfigIdentityProvider stays read-only and ArcSwap-backed.

## Summary

ConfigIdentityProvider already delegates to AuthPolicy::resolve_identity_from_fingerprint and resolve_identity_from_token (PeerEntry.auth_token_hash → fall through to ApiKeyEntry). Migrated auth.rs tests to PeerEntry model: renamed config_with_fingerprint → config_with_peer_entry, added tests for token resolution via PeerEntry.auth_token_hash (returns peer_id), fall-through to ApiKeyEntry (returns prefix), and disabled PeerEntry returning None on both fingerprint and token paths. ConfigIdentityProvider stays read-only (no IdentityStore impl), ArcSwap-backed. 130 tests pass, clippy clean, fmt clean.