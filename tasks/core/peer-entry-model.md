---
id: core/peer-entry-model
name: Add PeerEntry struct and replace AuthPolicy.authorized_fingerprints with peers (ADR-030)
status: pending
depends_on: []
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Replace `AuthPolicy.authorized_fingerprints: HashSet<String>` with
`AuthPolicy.peers: Vec<PeerEntry>`, per ADR-030. This is the foundational data
change for the entire ADR-029/030 sync — every downstream task (core resolution
logic, IdentityStore, call peer-keyed routing, fingerprint normalization) depends
on this struct and the `AuthPolicy.peers` field.

This task adds the `PeerEntry` struct and the `AuthPolicy.peers` field, and
migrates the `AuthPolicy` resolution methods to the new model. The
`ConfigIdentityProvider` rewrite (the resolution-logic half) is a separate task
(`core/config-identity-provider-peerentry`) so this task stays focused on the
data model + `AuthPolicy` resolution methods.

### PeerEntry struct

```rust
pub struct PeerEntry {
    /// Stable logical peer id ("worker-a", "alice"). Does NOT change on
    /// key rotation. This becomes Identity.id on resolution, regardless of
    /// which credential path resolved the identity.
    pub peer_id: String,

    /// TLS fingerprints for this peer — one or more. A peer may have
    /// multiple keys (e.g., an Ed25519 raw key for P2P and an X.509 cert
    /// for domain-facing). Resolution matches against any entry.
    /// Format: "ed25519:<hex of 32-byte pub key>" for RFC 7250 raw keys
    /// (normalized across quinn and iroh — ADR-030 §6), "SHA256:<hex>" for
    /// X.509 certs (DER hash). Changes on key rotation.
    pub fingerprints: Vec<String>,

    /// Optional: bearer-token authentication for this peer. A peer that
    /// also authenticates via auth_token (e.g., HTTP clients that can't
    /// do TLS client-auth) stores the SHA-256 hash of the token here.
    /// Resolution via resolve_from_token matches this field and returns
    /// the same Identity { id: peer_id, ... } as the fingerprint path.
    pub auth_token_hash: Option<String>,

    /// Authorization scopes granted to this peer. Resolved into
    /// Identity.scopes.
    pub scopes: Vec<String>,

    /// Named resource lists granted to this peer. Resolved into
    /// Identity.resources.
    pub resources: HashMap<String, Vec<String>>,

    /// Human-readable display name for logs / UIs. Optional.
    pub display_name: Option<String>,

    /// Whether this peer is authorized at all. false = recognized but
    /// disabled (revoked). Resolution returns None.
    pub enabled: bool,
}
```

### AuthPolicy change

```rust
pub struct AuthPolicy {
    /// Replaces authorized_fingerprints: HashSet<String>. Each entry maps
    /// a stable logical peer_id to its credential paths (fingerprints,
    /// optional auth_token_hash) + scopes + resources. The list is keyed
    /// by peer_id; resolution looks up by fingerprint OR auth_token.
    pub peers: Vec<PeerEntry>,

    /// API keys for bearer-token auth where the token IS the identity
    /// (rotation = new identity). Unchanged by ADR-030.
    pub api_keys: Vec<ApiKeyEntry>,
}
```

### AuthPolicy resolution methods (new model)

`AuthPolicy::resolve_identity_from_fingerprint` and a new
`resolve_identity_from_token` method resolve via `PeerEntry`:

```rust
impl AuthPolicy {
    pub fn resolve_identity_from_fingerprint(&self, fingerprint: &str) -> Option<Identity> {
        self.peers.iter()
            .find(|p| p.enabled && p.fingerprints.iter().any(|f| f == fingerprint))
            .map(|p| Identity {
                id: p.peer_id.clone(),
                scopes: p.scopes.clone(),
                resources: p.resources.clone(),
            })
    }

    pub fn resolve_identity_from_token(&self, token: &str) -> Option<Identity> {
        let token_hash = sha256(token);
        self.peers.iter()
            .find(|p| p.enabled && p.auth_token_hash.as_deref() == Some(&token_hash))
            .map(|p| Identity {
                id: p.peer_id.clone(),
                scopes: p.scopes.clone(),
                resources: p.resources.clone(),
            })
            .or_else(|| self.resolve_api_key(token))  // fall through to ApiKeyEntry
    }
}
```

The key change: `Identity.id` is now the stable `peer_id`, **not** the
fingerprint. Key rotation changes the fingerprint but not the `peer_id`, so ACL
entries and routing references stay stable (ADR-030 §2-3).

`resolve_api_key` stays unchanged (the `ApiKeyEntry` path where the token IS the
identity — `Identity.id = prefix`).

### Config validation

`PeerEntry.peer_id` is operator-chosen and unique within a config. Add a
validation method or assertion that duplicate `peer_id` values in
`AuthPolicy.peers` are a config error (ADR-030 Assumption 2).

### What this task does NOT do

- Does NOT rewrite `ConfigIdentityProvider` — that's
  `core/config-identity-provider-peerentry` (the `ConfigIdentityProvider` methods
  delegate to `AuthPolicy` resolution, so they keep working once `AuthPolicy`
  is updated, but the token-resolution path in `ConfigIdentityProvider` needs to
  call the new `resolve_identity_from_token` instead of only `resolve_api_key`).
- Does NOT normalize quinn fingerprints to `ed25519:<hex>` — that's
  `core/fingerprint-normalization`.
- Does NOT add `IdentityStore` or `CredentialStore` — those are separate tasks.

## Acceptance Criteria

- [ ] `PeerEntry` struct with all 7 fields (`peer_id`, `fingerprints`, `auth_token_hash`, `scopes`, `resources`, `display_name`, `enabled`)
- [ ] `AuthPolicy.authorized_fingerprints` removed; replaced with `peers: Vec<PeerEntry>`
- [ ] `AuthPolicy.api_keys` unchanged
- [ ] `AuthPolicy::resolve_identity_from_fingerprint` resolves fingerprint → PeerEntry → `Identity { id: peer_id, ... }`
- [ ] `AuthPolicy::resolve_identity_from_token` resolves token hash → PeerEntry → `Identity { id: peer_id, ... }`, falls through to `resolve_api_key`
- [ ] `Identity.id` is the `peer_id` (stable), not the fingerprint
- [ ] Disabled peers (`enabled: false`) return `None` from resolution
- [ ] Duplicate `peer_id` validation (config error)
- [ ] Unit test: fingerprint resolution via PeerEntry (known → Some with peer_id, unknown → None, disabled → None)
- [ ] Unit test: token resolution via PeerEntry.auth_token_hash (matching → Some with peer_id, non-matching → fall through to ApiKeyEntry)
- [ ] Unit test: multi-fingerprint PeerEntry (any fingerprint in the list resolves to the same peer_id)
- [ ] Unit test: resources populated from PeerEntry.resources on both paths
- [ ] Unit test: duplicate peer_id detected/rejected
- [ ] `cargo test -p alknet-core` succeeds
- [ ] `cargo clippy -p alknet-core` succeeds with no warnings

## References

- docs/architecture/crates/core/config.md — PeerEntry, AuthPolicy.peers
- docs/architecture/crates/core/auth.md — Identity.id = peer_id, multi-credential resolution
- docs/architecture/decisions/030-peerentry-and-identity-id-decoupling.md — ADR-030

## Notes

> This is the foundational data change for the ADR-029/030 sync. The key
> semantic shift: `Identity.id` changes from the fingerprint (crypto material)
> to the `peer_id` (stable logical id). Key rotation changes the fingerprint
> but not the `peer_id`, so ACL entries and `PeerRef::Specific(peer_id)`
> references stay stable. `ConfigIdentityProvider` keeps working (it delegates
> to `AuthPolicy`), but the token path needs the new
> `resolve_identity_from_token` — that's the separate
> `core/config-identity-provider-peerentry` task.

## Summary

> To be filled on completion