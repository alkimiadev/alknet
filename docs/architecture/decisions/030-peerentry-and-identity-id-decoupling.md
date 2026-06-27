# ADR-030: PeerEntry and Identity.id Decoupling

## Status

Accepted (supersedes the "v1 UUID" source in ADR-029 Assumption 1; resolves
the "real solution" half of OQ-33 and the storage-boundary half of OQ-34)

## Context

`Identity.id` is the string that keys authorization decisions across the
alknet crate graph. Today it is **coupled to the cryptographic material**:

```rust
// crates/alknet-core/src/config.rs — current implementation
pub struct AuthPolicy {
    pub authorized_fingerprints: HashSet<String>,  // just strings, no stable id
    pub api_keys: Vec<ApiKeyEntry>,
}

impl AuthPolicy {
    pub fn resolve_identity_from_fingerprint(&self, fingerprint: &str) -> Option<Identity> {
        if self.authorized_fingerprints.contains(fingerprint) {
            Some(Identity {
                id: fingerprint.to_string(),   // ← identity IS the crypto material
                scopes: vec!["relay:connect".to_string()],
                ...
            })
        }
    }
}
```

This coupling is a latent bug for any cross-node authorization decision:

- A TLS fingerprint or raw-key identity changes when the node rotates its key.
- When it changes, every ACL entry that references the old fingerprint stops
  matching — the peer "disappears" from the authorization system even though
  it is the same logical node.
- `PeerRef::Specific(PeerId)` (ADR-029) routes by `Identity.id`; a key
  rotation would break in-flight routing references the same way.
- The hub's `authorized_fingerprints` set has to be manually updated on every
  rotation on the *remote* side, which is exactly the operational pain the
  vault's local key rotation (ADR-021) was meant to remove.

ADR-029 §1 set `PeerId = Identity.id` and made `PeerId` a logical identifier
"NOT `Identity.id` (the fingerprint)" — but left the *source* of that logical
identifier as a connection-assigned UUID (OQ-33's v1 workaround). The UUID
is ephemeral: it survives only for the connection's lifetime, changes on
reconnect, and cannot persist across restarts or key rotations. It is a
no-storage workaround, not a real identity.

The research at `docs/research/alknet-storage-strategy/findings.md` §4
established the real fix: introduce a `PeerEntry` config model that maps a
**stable logical peer id** to its current cryptographic material and
authorization scopes, and have `ConfigIdentityProvider` resolve
fingerprint → `PeerEntry` → `Identity { id: peer_entry.peer_id, scopes:
peer_entry.scopes, ... }`. The `Identity.id` becomes the stable `peer_id`,
decoupled from the fingerprint. Key rotation is a single field update in the
peer entry; the `peer_id` and every ACL / routing reference to it stay
stable.

This is the storage-boundary question OQ-34 tracks. With ADR-033 (the
repo/adapter pattern) establishing that core defines repo traits and the
default in-memory adapter lives alongside the trait, the answer is: core
gets the `PeerEntry` config model and the
`ConfigIdentityProvider::resolve_from_fingerprint → Identity { id: peer_id
}` resolution path now, with no SQLite dependency in core. A future
`alknet-peer-store-sqlite` adapter that persists `PeerEntry` records is
additive — it implements the same `IdentityProvider` trait against a `peers`
table instead of config. The trait is the one-way door; the adapter is the
two-way door.

## Decision

### 1. Add `PeerEntry` to `AuthPolicy`, replacing `authorized_fingerprints`

```rust
pub struct PeerEntry {
    /// Stable logical peer id ("worker-a", "alice"). Does NOT change on
    /// key rotation. This becomes Identity.id on resolution.
    pub peer_id: String,

    /// Current cryptographic material — the fingerprint the endpoint
    /// extracts from the TLS handshake (SHA256:... for X.509, ed25519:...
    /// for RFC 7250 raw keys). Changes on key rotation.
    pub fingerprint: String,

    /// Authorization scopes granted to this peer. Resolved into
    /// Identity.scopes.
    pub scopes: Vec<String>,

    /// Named resource lists granted to this peer. Resolved into
    /// Identity.resources. Populated from config (not just composition, as
    /// the pre-ADR-030 limitation in auth.md §"Resource-scoped ACLs and
    /// external identities" required).
    pub resources: HashMap<String, Vec<String>>,

    /// Human-readable display name for logs / UIs. Optional.
    pub display_name: Option<String>,

    /// Whether this peer is authorized at all. false = the fingerprint
    /// is recognized but the peer is disabled (token-revoked-equivalent
    /// for fingerprints). Resolution returns None.
    pub enabled: bool,
}

pub struct AuthPolicy {
    /// Replaces authorized_fingerprints: HashSet<String>. Each entry maps
    /// a stable logical peer_id to its current fingerprint + scopes +
    /// resources. The list is keyed by peer_id; resolution looks up by
    /// fingerprint.
    pub peers: Vec<PeerEntry>,

    /// API keys — unchanged by this ADR (see "API keys" below).
    pub api_keys: Vec<ApiKeyEntry>,
}
```

### 2. `Identity.id` becomes `PeerEntry.peer_id` on fingerprint resolution

`ConfigIdentityProvider::resolve_from_fingerprint` resolves fingerprint →
matching `PeerEntry` → `Identity { id: peer_entry.peer_id, scopes:
peer_entry.scopes, resources: peer_entry.resources }`. The `Identity.id` is
the stable `peer_id`, not the fingerprint.

```rust
impl AuthPolicy {
    pub fn resolve_identity_from_fingerprint(&self, fingerprint: &str) -> Option<Identity> {
        self.peers.iter()
            .find(|p| p.enabled && p.fingerprint == fingerprint)
            .map(|p| Identity {
                id: p.peer_id.clone(),
                scopes: p.scopes.clone(),
                resources: p.resources.clone(),
            })
    }
}
```

This removes the pre-ADR-030 limitation in `auth.md`
§"Resource-scoped ACLs and external identities" — fingerprint-resolved
identities now carry `resources` from the `PeerEntry`, not just from the
composition path. The composition path (`CompositionAuthority::as_identity`,
ADR-015/022) still produces its own `Identity` for internal calls; the
external-auth path now also carries resources when configured.

### 3. Key rotation is a single `PeerEntry.fingerprint` update

Rotating a peer's TLS key:
- The vault derives the new key locally (ADR-020/021).
- The remote side's config updates the `PeerEntry.fingerprint` field for
  that `peer_id`. The `peer_id`, `scopes`, `resources`, ACL entries, and
  any `PeerRef::Specific(peer_id)` references stay stable.
- A config reload (`ConfigReloadHandle::reload`) makes the change live.

No ACL update, no routing reference invalidation, no peer "disappears."
The vault's local rotation + a remote-side config edit is the full key
rotation story across nodes.

### 4. `PeerId` source changes from UUID to `Identity.id` from `PeerEntry`

ADR-029 Assumption 1 said `PeerId` is a connection-assigned UUID (v4). With
`Identity.id` now stable (`peer_id`), the UUID workaround is no longer
needed: `PeerId = Identity.id` from `IdentityProvider` resolution. This is
the one-way-door tightening — `PeerId` was always specified as logical-not-
crypto (ADR-029), the UUID was the *source*; the source now becomes the
auth system.

```rust
// ADR-029 §1, updated by this ADR:
pub type PeerId = String;  // = Identity.id from IdentityProvider resolution
                           // = PeerEntry.peer_id (stable, not crypto material)
```

ADR-029 §2's `invoke_peer` / `PeerRef::Specific(PeerId)` signatures are
unchanged. The `PeerId` payload is now stable across reconnects and key
rotations, instead of ephemeral. An in-flight `PeerRef::Specific` that
survives a reconnect now keeps resolving (the `peer_id` is unchanged), which
is the property the UUID workaround could not provide.

### 5. The `PeerId` for a connection comes from `IdentityProvider` resolution

The dispatch path that builds a `CallConnection` and assigns a `PeerId` to
the peer-keyed overlay (`PeerCompositeEnv::attach_peer`) reads
`connection.identity().id` — the resolved `Identity.id` from the
`IdentityProvider`. If identity resolution returns `None` (no client cert,
unrecognized fingerprint), the peer has no `PeerId` and the connection
cannot be added to the peer-keyed overlay. The handler either rejects the
connection or falls back to a connection-without-peer-identity path (the
caller-id-is-the-connection case, e.g., anonymous dial-in).

The UUID fallback is removed. A connection with no resolved identity has no
`PeerId`, not a random one.

## API keys

API keys (`ApiKeyEntry`) are **not** given the `PeerEntry` treatment. The
two identity sources have different semantics:

| Axis | Fingerprint (PeerEntry) | API key (ApiKeyEntry) |
|------|-------------------------|------------------------|
| Identity source | TLS handshake / SSH key | Bearer token in protocol frame |
| Key rotation | Same logical node, new material | New identity (revocation = new key) |
| `Identity.id` | `peer_id` (stable across rotation) | `prefix` (changes with the key) |
| Resource binding | `PeerEntry.resources` (per-peer) | Empty (Option B, auth.md) — resources are composition-only |

An API key's prefix IS the identity — rotating the key means a new prefix
and a new identity, by design (revocation is the rotation mechanism for
API keys). Decoupling the API key identity from the prefix would be solving
a different problem (persistent logical identity across key rotation) that
API keys don't have: they're bearer tokens, not node identities.

`ApiKeyEntry` stays as-is. The asymmetry is documented here and in
`auth.md` so the difference between the two auth paths is explicit, not an
oversight.

## What this does NOT change

- **`Identity` struct shape** — `id: String`, `scopes: Vec<String>`,
  `resources: HashMap<String, Vec<String>>` are unchanged. Only the
  *meaning* of `id` on the fingerprint path changes (fingerprint →
  peer_id).
- **`IdentityProvider` trait** — unchanged. The adapter's resolution
  semantics change, not the trait.
- **`AccessControl::check`** — unchanged. Still a flat scope/resource match
  against `Identity`. The `Identity` it checks now has a stable `id` on the
  fingerprint path, but `check` doesn't key on `id` (it checks scopes and
  resources).
- **`AuthToken`, `AuthContext`** — unchanged.
- **`PeerRef::Specific(PeerId)` signature** — unchanged. The payload is now
  stable.
- **`CompositeOperationEnv` → `PeerCompositeEnv` migration** — unchanged.
  This ADR provides the stable `PeerId` source; ADR-029 still owns the
  overlay-keying model.

## Consequences

**Positive:**
- Key rotation no longer breaks ACL entries or routing references on the
  remote side. The vault's local rotation story (ADR-021) is now the
  complete story — `rotate` locally, edit the peer entry's fingerprint
  remotely, reload.
- `PeerRef::Specific` survives reconnects. An in-flight routing reference
  to "worker-a" keeps resolving after worker-a's TLS key rotates and after
  worker-a reconnects.
- OQ-33's UUID workaround is removed — the stable logical id is the real
  thing, not an ephemeral stand-in.
- OQ-34's storage-boundary question is resolved: core has the config model
  (`PeerEntry`) + the in-memory adapter (`ConfigIdentityProvider`); a
  future `alknet-peer-store-sqlite` adapter that persists `PeerEntry`
  records is additive, implementing the same `IdentityProvider` trait
  against a `peers` table. See ADR-033.
- Fingerprint-resolved identities now carry `resources` (the pre-ADR-030
  limitation is lifted) — `AccessControl::check` against `resource_type`/
  `resource_action` works for external fingerprint-authenticated callers
  when configured.

**Negative:**
- `AuthPolicy.authorized_fingerprints: HashSet<String>` is replaced with
  `AuthPolicy.peers: Vec<PeerEntry>`. This is a breaking config change —
  existing config files with `authorized_fingerprints` migrate to `peers`
  entries. The migration is mechanical (each fingerprint becomes a
  `PeerEntry { peer_id: <chosen name>, fingerprint: <old value>, scopes:
  ["relay:connect"], ... }`), and operators must choose a `peer_id` per
  peer, but it is a config break.
- `Identity.id` for fingerprint-resolved identities changes from the
  fingerprint to the `peer_id`. Code that logs or compares `Identity.id`
  on the fingerprint path and assumed it was the fingerprint string will
  see the `peer_id` instead. This is the correct behavior (logs should
  show the logical name, not the rotating crypto material), but it's a
  behavior change in log output.
- The pre-ADR-030 `auth.md` "Resource-scoped ACLs and external identities"
  limitation note is removed — fingerprint-resolved identities now populate
  `resources`. Code that relied on fingerprint identities always having
  empty `resources` (an unintended invariant) will see populated resources
  when configured.
- ADR-029 Assumption 1 is superseded on the `PeerId` source dimension:
  the one-way door (`PeerId` is logical, not crypto) is preserved, but the
  v1 UUID source is replaced by `Identity.id` from `PeerEntry`. The
  Assumption's framing of "no-storage workaround" is no longer accurate —
  the storage boundary is now explicitly `config + in-memory adapter`
  (this ADR + ADR-033), with the SQLite adapter additive.

## Assumptions

1. **The dispatch path can require identity resolution for peer-keyed
   overlay membership.** A connection that fails `IdentityProvider`
   resolution has no `PeerId` and is not added to `PeerCompositeEnv`. The
   caller either authenticates with a recognized fingerprint (and gets a
   `peer_id`) or is rejected / falls back to a no-peer-identity path. The
   v1 UUID fallback is removed deliberately — anonymous dial-in to a
   peer-keyed composition env is a contradiction.

2. **`PeerEntry.peer_id` is operator-chosen and unique within a config.**
   Config validation enforces uniqueness; duplicate `peer_id` values in
   `AuthPolicy.peers` are a config error.

3. **API keys stay as-is.** The `ApiKeyEntry` model is correct for bearer-
   token identity where rotation = new identity. This ADR does not add a
   `PeerEntry`-equivalent for API keys. See "API keys" above.

4. **The `peers` list resolution is O(peers) per fingerprint lookup.** The
   expected peer count per node is small (10s–100s); a linear scan with a
   side index is fine. A `HashMap<fingerprint, &PeerEntry>` index is an
   implementation-detail two-way door.

5. **Adapter crates that persist `PeerEntry` records are additive and not
   specified here.** ADR-033 establishes the pattern (core trait + in-memory
   default; persistence adapters are separate crates); the concrete adapter
   shapes are deferred for exploration per the user's note. This ADR's
   commitment is to the `PeerEntry` config model + the resolution
   semantics + the `PeerId` source, not to any specific backend.

## References

- ADR-004: Auth as Shared Core (`IdentityProvider` in core)
- ADR-015: Privilege Model and Authority Context (`AccessControl::check`
  against `Identity`)
- ADR-021: Key Rotation via Version-Indexed Paths (the local rotation half
  this ADR completes across nodes)
- ADR-022: Handler Registration, Provenance, and Composition Authority
  (the registration bundle's `composition_authority` path produces its own
  `Identity`; this ADR's `PeerEntry.resources` populates the external-auth
  path's `Identity.resources`)
- ADR-029: Peer-Graph Routing Model (the `PeerId = Identity.id` model;
  Assumption 1's UUID source is superseded by this ADR's `PeerEntry.peer_id`
  source — the one-way door is preserved)
- ADR-033: Storage Boundary and Repo/Adapter Pattern (the overarching pattern
  this ADR's `PeerEntry` + `ConfigIdentityProvider` follows)
- OQ-33: PeerId — Cryptographic Identity vs Stable Logical Identifier
  (resolved by this ADR — the "real solution" half, replacing the UUID
  workaround)
- OQ-34: Persistent Peer Registry (resolved by this ADR + ADR-033 — the
  storage boundary is `config + in-memory adapter` now, SQLite adapter
  additive)
- OQ-35: API Key Identity vs Peer Identity (recorded by this ADR — the
  asymmetry is deliberate, see "API keys" above)
- `docs/research/alknet-storage-strategy/findings.md` §4 (the `PeerEntry`
  model and resolution path)
- `docs/architecture/crates/core/auth.md` (the spec amended by this ADR)
- `docs/architecture/crates/core/config.md` (the `AuthPolicy` change)