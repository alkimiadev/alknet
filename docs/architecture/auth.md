---
status: draft
last_updated: 2026-06-07
---

# Authentication & Identity

## What

A unified authentication and identity layer that works across all transports —
SSH-over-any-transport and WebTransport (non-SSH HTTP-level transports). The
same key material (Ed25519 authorized keys and certificate authorities) is
shared across both auth paths. Identity resolution produces a transport-agnostic
`Identity` that carries scopes and resources for downstream authorization.

## Why

Alknet currently authenticates connections exclusively through SSH public key
auth. Non-SSH transports (WebTransport) cannot perform SSH key exchange — they
need a different auth presentation that shares the same key material. The
unified auth layer ensures one key set, one identity, one rotation mechanism
across all transports. See ADR-023 for the decision context.

## Architecture

### Auth Presentation Per Transport

| Transport | Auth presentation | Verification |
|-----------|-------------------|-------------|
| SSH (TCP, TLS, iroh) | SSH public key auth in the SSH handshake | `ServerAuthConfig::authenticate_publickey()` — key lookup in authorized set |
| WebTransport (HTTP/3) | Signed timestamp token in CONNECT request | Token auth — same authorized set verifies the Ed25519 signature |
| Future (WebSocket, etc.) | Signed timestamp token in headers/query | Same token verification |

The **key material is shared**. The **presentation differs per transport**. The
**verification result is the same**: an authenticated identity with scopes.

### Token Authentication

For non-SSH transports, the client constructs an authentication token:

```
AuthToken = base64url(key_id || timestamp || signature)

  key_id    = SHA-256 fingerprint of the Ed25519 public key (32 bytes)
  timestamp = Unix seconds, big-endian u64 (8 bytes)
  signature = Ed25519 sign(key_id || timestamp_bytes, private_key)
```

Wire format when passed in a WebTransport CONNECT request:
```
CONNECT https://server:443/alknet?token=<AuthToken>
```

Server verification:

1. Base64url-decode the token
2. Extract `key_id` (first 32 bytes)
3. Look up `key_id` in the same `authorized_keys` set that SSH auth uses
4. Verify the Ed25519 `signature` against `(key_id || timestamp_bytes)` using
   the matching public key
5. Check `timestamp` is within the acceptable window (configurable, default
   ±300 seconds)
6. Resolve to the same `Identity` that SSH pubkey auth would produce

The key fingerprint in the token serves double duty: it identifies which key
to verify against, and it ties the signature to a specific key (swapping
`key_id` invalidates the signature).

### Replay Protection

V1 uses timestamp-only (±300s window, no server state). The replay trade-offs
and future zero-replay options (nonce challenge-response) are documented in
ADR-023.

### IdentityProvider Trait

The `IdentityProvider` trait decouples alknet-core from any specific identity
storage. It resolves a key fingerprint or auth token to an `Identity` with
scopes and resources.

```rust
pub trait IdentityProvider: Send + Sync + 'static {
    /// Resolve an SSH public key fingerprint to an identity.
    fn resolve_from_fingerprint(&self, fingerprint: &str) -> Option<Identity>;

    /// Resolve an auth token to an identity.
    /// Returns None if the token is invalid, expired, or the key is not authorized.
    fn resolve_from_token(&self, token: &AuthToken) -> Option<Identity>;
}

pub struct Identity {
    pub id: String,                              // Unique identifier — fingerprint (config) or account UUID (database)
    pub scopes: Vec<String>,                     // e.g., ["relay:connect", "service:gitea:read"]
    pub resources: HashMap<String, Vec<String>>,  // e.g., {"service": ["gitea", "registry"]}
}
```

> **Note on identity models**: Earlier research used `{node_id, fingerprint, scopes}`.
> The unified model uses `{id, scopes, resources}` where `id` serves as both
> fingerprint (for key-based auth from config) and account UUID (for
> database-backed auth). The `resources` field provides resource-level
> authorization beyond what scopes offer. This is the canonical definition
> that all components should use.
```

**Default implementation**: `ConfigIdentityProvider` loads from
`DynamicConfig.auth` (the `authorized_keys` set). Every authorized key gets a
default scope set. No database required.

**Head implementation**: Backed by `@alkdev/storage`'s `peer_credentials` and
`accounts` tables plus the ACL graph. Resolves fingerprint → account →
organization membership → effective scopes. Uses `ArcSwap` for hot reload.

The trait is the contract. The backing store is pluggable. Alknet-core never
depends on Honker, SQLite, or any specific database.

### AuthPolicy Structure

`AuthPolicy` in `DynamicConfig` holds both auth paths, sharing key material:

```rust
pub struct AuthPolicy {
    pub ssh: SshAuthConfig,
    pub token: TokenAuthConfig,
}

pub struct SshAuthConfig {
    pub authorized_keys: HashSet<PublicKey>,
    pub cert_authorities: Vec<CertAuthorityEntry>,
    // Existing fields from current ServerAuthConfig
}

pub struct TokenAuthConfig {
    pub enabled: bool,
    pub max_token_age: Duration,  // Timestamp window (default: 300s)
    pub key_source: TokenKeySource,
}

pub enum TokenKeySource {
    /// Share the same authorized_keys set with SshAuthConfig.
    /// Default and recommended for v1.
    Shared,
    /// Separate key set for non-SSH transports.
    /// For deployments that want distinct access control per transport.
    Separate(HashSet<PublicKey>),
}
```

When `TokenKeySource::Shared` (the default), adding a key to
`authorized_keys` immediately grants access via both SSH and WebTransport.
One key set, one `reloadAuth()` call, one rotation.

### Auth Flow in the Server

**SSH transport (existing, unchanged):**
```
Client connects → SSH handshake → auth_publickey() callback
  → ServerAuthConfig::authenticate_publickey() or authenticate_certificate()
  → Auth::Accept or Auth::Reject
```

**WebTransport transport (new):**
```
Browser connects → WebTransport CONNECT request
  → SessionRequest inspection: extract token from URL path or header
  → TokenAuthConfig verification: decode token → lookup key_id → verify signature → check timestamp
  → session_request.accept() or session_request.forbidden()
```

After auth, both paths produce an `Identity`. The `Identity` is attached to the
connection and used by `ForwardingPolicy` and the call protocol to make
authorization decisions.

### WebTransport SessionRequest Inspection

The wtransport library's `SessionRequest` provides:

- `path()` — URL path (e.g., `/alknet?token=...`)
- `headers()` — HTTP headers (for `Authorization: Bearer ...`)
- `origin()` — Browser origin (for CORS-like restrictions)
- `remote_address()` — Client UDP address

Token extraction from URL path is preferred for browser WebTransport because
the W3C API (`new WebTransport(url)`) naturally includes query parameters. For
native clients (Deno, CLI), the `Authorization` header is also supported.

### Browser-Side Token Construction

```javascript
// Illustrative — see client SDK for production implementation
async function createAuthToken(keyPair) {
    const publicKey = await crypto.subtle.exportKey('raw', keyPair.publicKey);
    const keyId = new Uint8Array(await crypto.subtle.digest('SHA-256', publicKey));

    const timestamp = new ArrayBuffer(8);
    new DataView(timestamp).setBigUint64(0, BigInt(Math.floor(Date.now() / 1000)));

    const message = new Uint8Array([...keyId, ...new Uint8Array(timestamp)]);
    const signature = await crypto.subtle.sign('Ed25519', keyPair.privateKey, message);

    const token = new Uint8Array([...keyId, ...new Uint8Array(timestamp), ...new Uint8Array(signature)]);
    return btoa(String.fromCharCode(...token))
        .replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}
```

Browsers support Ed25519 key generation and signing via `SubtleCrypto` (Chrome
105+, Firefox 130+, Safari 17+). Deno supports it natively. No external
dependencies needed.

## Constraints

- Auth tokens are Ed25519-signed with the same key pair used for SSH auth. No
  separate key management for non-SSH transports.
- `IdentityProvider` is the only interface between alknet-core and identity
  storage. No database dependency at the core level.
- The SSH auth path is unchanged. `auth_publickey()` continues to work exactly
  as it does today. Token auth is additive.
- Certificate authority tokens are not supported for token auth in v1. CA
  verification requires the full OpenSSH certificate structure, which doesn't
  fit in a simple signed timestamp. This can be added later if needed.
- Token auth is only available on transports that carry HTTP metadata (URL
  path, headers). SSH-over-TCP/TLS/iroh continues to use SSH native auth
  exclusively.

### Security Considerations

**Token in URL**: The auth token is passed as a URL query parameter
(`?token=...`) for browser WebTransport compatibility. This is a known web
security consideration:

- **Server logs**: The token may appear in HTTP access logs. Servers MUST
  strip or redact the `token` query parameter before logging the request URL.
- **Browser history**: The token may appear in browser history. Timestamps
  limit exposure to the token window (±300s).
- **Referrer headers**: WebTransport does not send referrer headers, so the
  token does not leak via HTTP Referer.
- **Native clients**: Deno and native clients SHOULD prefer the `Authorization:
  Bearer` header over URL parameters when the client supports custom headers.

## Open Questions

- **OQ-18**: Should `Identity.scopes` be populated from `ForwardingPolicy`
  rules, from an external `IdentityProvider`, or from both? See
  [open-questions.md](open-questions.md).

- **OQ-19**: Should the WebTransport listener require its own TLS identity
  (separate from the SSH-over-TLS listener), or can they share the same
  certificate? See [open-questions.md](open-questions.md).

## Design Decisions

| ADR | Decision | Summary |
|-----|----------|---------|
| [012](decisions/012-auth-ed25519-and-cert-authority.md) | Ed25519 + cert-authority | Key-based auth, no passwords |
| [023](decisions/023-unified-auth-shared-key-material.md) | Unified auth, shared key material | Same keys for SSH and token auth |

## References

- [server.md](server.md) — Current SSH auth handler
- [transport.md](transport.md) — Transport abstraction
- [configuration.md](../research/configuration.md) — DynamicConfig, AuthPolicy structure
- [open-questions.md](open-questions.md) — OQ-17 (resolved), OQ-18, OQ-19
- `server/handler.rs` — Current `auth_publickey()` callback
- `auth/server_auth.rs` — Current `ServerAuthConfig` struct
- `auth/keys.rs` — `KeySource` and key loading
- [wtransport](https://github.com/BiagioFesta/wtransport) — Rust WebTransport library
- [WebTransport W3C Spec](https://www.w3.org/TR/webtransport/) — Browser API
- [@alkdev/storage](/workspace/@alkdev/storage) — `peer_credentials` table, ACL graph