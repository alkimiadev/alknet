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
    /// key rotation. This becomes Identity.id on resolution, regardless of
    /// which credential path resolved the identity.
    pub peer_id: String,

    /// TLS fingerprints for this peer — one or more. A peer may have
    /// multiple keys (e.g., an Ed25519 raw key for P2P and an X.509 cert
    /// for domain-facing). Resolution matches against any entry.
    /// Format: "ed25519:<hex of 32-byte pub key>" for RFC 7250 raw keys
    /// (normalized across quinn and iroh — see §6), "SHA256:<hex>" for
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

pub struct AuthPolicy {
    /// Replaces authorized_fingerprints: HashSet<String>. Each entry maps
    /// a stable logical peer_id to its credential paths (fingerprints,
    /// optional auth_token_hash) + scopes + resources. The list is keyed
    /// by peer_id; resolution looks up by fingerprint OR auth_token.
    pub peers: Vec<PeerEntry>,

    /// API keys for bearer-token auth where the token IS the identity
    /// (rotation = new identity). Peers that need a stable logical id
    /// across credential rotation use PeerEntry.auth_token_hash instead.
    /// See "Bearer tokens" below.
    pub api_keys: Vec<ApiKeyEntry>,
}
```

### 2. `Identity.id` becomes `PeerEntry.peer_id` on resolution (any credential path)

`ConfigIdentityProvider::resolve_from_fingerprint` resolves fingerprint →
matching `PeerEntry` (by any entry in `fingerprints`) → `Identity { id:
peer_entry.peer_id, ... }`. `ConfigIdentityProvider::resolve_from_token`
resolves token → matching `PeerEntry` (by `auth_token_hash`) → the same
`Identity { id: peer_entry.peer_id, ... }`. Both paths produce the same
`Identity` — the `peer_id` is the stable logical id regardless of how the
peer authenticated.

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

If the token doesn't match any `PeerEntry.auth_token_hash`, resolution falls
through to `resolve_api_key` (the `ApiKeyEntry` path, where `Identity.id =
prefix`). This preserves the existing API-key path for bearer tokens that
ARE the identity, while adding the `PeerEntry` token path for tokens that
are one credential path among several for a stable logical peer.

This removes the pre-ADR-030 limitation in `auth.md`
§"Resource-scoped ACLs and external identities" — resolved identities now
carry `resources` from the `PeerEntry`, not just from the composition path.

### 3. Key rotation is a `PeerEntry` field update (no `peer_id` change)

Rotating a peer's TLS key:
- The vault derives the new key locally (ADR-020/021).
- The remote side's config updates the `PeerEntry.fingerprints` entry for
  that `peer_id`. The `peer_id`, `scopes`, `resources`, ACL entries, and
  any `PeerRef::Specific(peer_id)` references stay stable.
- A config reload (`ConfigReloadHandle::reload`) makes the change live.

Rotating a peer's auth token:
- Update `PeerEntry.auth_token_hash` for that `peer_id`. The `peer_id`
  and everything that references it stays stable.

No ACL update, no routing reference invalidation, no peer "disappears."
The vault's local rotation + a remote-side config edit is the full key
rotation story across nodes, for any credential path.

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

### 6. Fingerprint format normalization: `ed25519:` for raw keys

Ed25519 raw keys (RFC 7250) produce different fingerprint formats depending
on the transport:

- **iroh** (direct or relay): `ed25519:<hex of 32-byte public key>` —
  extracted from `connection.remote_node_id()`, which returns the NodeId
  (the raw Ed25519 public key). Already implemented.
- **quinn RawKey**: currently `SHA256:<hex of cert DER>` — because
  `fingerprint_from_cert_der` hashes the SPKI DER bytes. The DER encoding
  of the SPKI is not the raw 32-byte public key; it's an ASN.1 wrapper.
  So the same Ed25519 key produces `ed25519:abc...` on iroh and
  `SHA256:def...` on quinn — two different fingerprints for the same key.

This is normalized: the quinn path extracts the Ed25519 public key from the
cert DER (the `RawKeyCertResolver` already has the raw key bytes via
`Ed25519SecretKey::public()`) and formats it as `ed25519:<hex>`, matching
iroh. A peer that connects via quinn direct and via iroh has the same
fingerprint in `PeerEntry.fingerprints` — one entry, both transports.

The normalization is in `extract_quinn_client_fingerprint`: when the
presented cert is an RFC 7250 raw public key (SPKI with Ed25519 algorithm
identifier), extract the raw 32-byte public key and format as
`ed25519:<hex>`. When the cert is X.509, keep the `SHA256:<hex of DER>`
format (X.509 certs don't have a "raw public key" form — the DER hash is
the fingerprint).

This also simplifies the coming WebTransport relay work: a WebTransport
relay acts as a proxy, and the proxied connection's Ed25519 identity
should be the same `ed25519:<hex>` whether the client connected directly
or through the relay. Normalizing on the iroh pattern means the relay
doesn't need a separate fingerprint format.

## Bearer tokens

There are three credential types in the alknet auth model:

1. **Ed25519 raw key** (RFC 7250) — the most common. Same key type as SSH
   keys, native to iroh's `NodeId`. Fingerprint format: `ed25519:<hex>`.
   Used for direct quinn, iroh direct, and iroh relay connections. The
   fingerprint IS the trust anchor (no CA needed).

2. **X.509 cert** — for domain-facing endpoints (`api.alk.dev`, relays,
   ACME/Let's Encrypt). Fingerprint format: `SHA256:<hex of DER>`. Requires
   CA verification on the client side. The outgoing-only case (a client
   connects to a public X.509 endpoint) is tracked as OQ-37.

3. **Bearer token** (auth_token) — for HTTP clients that can't do TLS
   client-auth (browsers, curl), or as a secondary credential path. Carried
   in the call-protocol `auth_token` payload field.

A `PeerEntry` can have any combination of these: `fingerprints: Vec<String>`
for one or more TLS keys (Ed25519 and/or X.509), `auth_token_hash:
Option<String>` for an optional bearer-token path. All resolve to the same
`peer_id`. A peer that authenticates via Ed25519 today and via auth_token
tomorrow gets the same `PeerId` — the logical identity is stable across
credential paths.

`ApiKeyEntry` stays as a separate path for bearer tokens where the token IS
the identity (rotation = new identity, no stable logical id needed). When a
bearer token is one credential path among several for a stable peer, it
goes in `PeerEntry.auth_token_hash`. The distinction is not "peer bearer vs
auth bearer" — it's whether the token needs a stable logical id across
rotation (`PeerEntry`) or not (`ApiKeyEntry`).

| Credential type | `PeerEntry` field | `Identity.id` | Rotation |
|-----------------|-------------------|---------------|----------|
| Ed25519 raw key | `fingerprints[i]` (`ed25519:...`) | `peer_id` (stable) | Update `fingerprints` entry |
| X.509 cert | `fingerprints[i]` (`SHA256:...`) | `peer_id` (stable) | Update `fingerprints` entry |
| Bearer token (peer) | `auth_token_hash` | `peer_id` (stable) | Update `auth_token_hash` |
| Bearer token (identity) | `ApiKeyEntry` (separate) | `prefix` (changes with key) | New `ApiKeyEntry` |

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
  remote side — for any credential path (TLS key or auth token). The
  vault's local rotation story (ADR-021) is now the complete story.
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
- Resolved identities now carry `resources` (the pre-ADR-030 limitation is
  lifted) — `AccessControl::check` against `resource_type`/
  `resource_action` works for external authenticated callers when
  configured, regardless of credential path.
- A peer can authenticate via Ed25519 today and via auth_token tomorrow,
  getting the same `PeerId` — the logical identity is stable across
  credential paths.
- Fingerprint normalization (`ed25519:<hex>` for raw keys across quinn and
  iroh) means the same key has the same fingerprint regardless of transport.
  This also simplifies the coming WebTransport relay work.

**Negative:**
- `AuthPolicy.authorized_fingerprints: HashSet<String>` is replaced with
  `AuthPolicy.peers: Vec<PeerEntry>`. This is a breaking config change —
  existing config files with `authorized_fingerprints` migrate to `peers`
  entries. The migration is mechanical (each fingerprint becomes a
  `PeerEntry { peer_id: <chosen name>, fingerprints: vec![<old value>], ... }`),
  and operators must choose a `peer_id` per peer, but it is a config break.
- `Identity.id` for resolved identities changes from the fingerprint to
  the `peer_id`. Code that logs or compares `Identity.id` and assumed it
  was the fingerprint string will see the `peer_id` instead. This is the
  correct behavior (logs should show the logical name, not the rotating
  crypto material), but it's a behavior change in log output.
- The quinn fingerprint extraction changes from `SHA256:<hex of DER>` to
  `ed25519:<hex of raw key>` for raw-key certs. Existing configs with
  `SHA256:` fingerprints for Ed25519 keys migrate to `ed25519:` format.
  X.509 fingerprints stay as `SHA256:<hex of DER>`.
- ADR-029 Assumption 1 is superseded on the `PeerId` source dimension:
  the one-way door (`PeerId` is logical, not crypto) is preserved, but the
  UUID source is replaced by `Identity.id` from `PeerEntry`. The
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

3. **Bearer tokens have two paths.** `PeerEntry.auth_token_hash` is for
   tokens that are one credential path among several for a stable logical
   peer (the token rotates, the `peer_id` stays). `ApiKeyEntry` is for
   tokens that ARE the identity (rotation = new identity, no stable
   logical id needed). See "Bearer tokens" above. The distinction is not
   "peer bearer vs auth bearer" — it's whether the token needs a stable
   logical id across rotation.

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
- ~~OQ-35: API Key Identity vs Peer Identity~~ (dissolved — the
  "asymmetry" framing was wrong; `PeerEntry` supports multiple credential
  paths, and `ApiKeyEntry` is for tokens that ARE the identity)
- OQ-29: CallClient TLS Client-Auth (resolved by this ADR's §6 fingerprint
  normalization + the client-auth wiring decision recorded in OQ-29)
- OQ-37: X.509 outgoing-only case (the three auth types and how X.509
  server identity fits — see OQ-37 in open-questions.md)
- `docs/research/alknet-storage-strategy/findings.md` §4 (the `PeerEntry`
  model and resolution path)
- `docs/architecture/crates/core/auth.md` (the spec amended by this ADR)
- `docs/architecture/crates/core/config.md` (the `AuthPolicy` change)