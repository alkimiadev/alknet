---
status: draft
last_updated: 2026-06-09
---

# Authentication

## What

A unified authentication layer that works across all transports — SSH-over-any-
transport and WebTransport (non-SSH HTTP-level transports). The same key
material (Ed25519 authorized keys and certificate authorities) is shared across
both auth paths. Identity resolution produces a transport-agnostic `Identity`
that carries scopes and resources for downstream authorization.

## Why

Alknet currently authenticates connections exclusively through SSH public key
auth. Non-SSH transports (WebTransport) cannot perform SSH key exchange — they
need a different auth presentation that shares the same key material. The
unified auth layer ensures one key set, one identity, one rotation mechanism
across all transports. See ADR-023 for the decision context.

The canonical definitions of `Identity` and `IdentityProvider` are in
[identity.md](identity.md). This document covers auth-specific behavior:
auth presentation per transport, `AuthPolicy` structure, and the auth service
relationship.

## Architecture

### Identity and IdentityProvider

See [identity.md](identity.md) for the canonical definitions of:
- `Identity` struct (`{ id, scopes, resources }`)
- `IdentityProvider` trait (`resolve_from_fingerprint()`, `resolve_from_token()`)
- `ConfigIdentityProvider` (default, ArcSwap-backed)
- `StorageIdentityProvider` (production, SQLite-backed, in alknet-storage)
- `AuthProtocol` irpc service (behind `irpc` feature flag)

The key relationship: `IdentityProvider` is the contract. `ConfigIdentityProvider`
is the default implementation (reads from `DynamicConfig.auth`). `AuthProtocol`
irpc service is one way to satisfy the trait, behind a feature flag. Both paths
produce the same `Identity` result. See ADR-028 and ADR-029.

### Credential Presentation Per Interface

Each (Transport, Interface) pair presents credentials differently, but all
resolve to the same `Identity` through `IdentityProvider`. See
[definitions.md](definitions.md) for the full terminology rules.

| (Transport, Interface) | Credential presentation | Resolves via |
|------------------------|------------------------|-------------|
| (TLS, SshInterface) | SSH public key handshake | `resolve_from_fingerprint()` |
| (TCP, SshInterface) | SSH public key handshake | `resolve_from_fingerprint()` |
| (iroh, SshInterface) | SSH public key handshake | `resolve_from_fingerprint()` |
| (TLS, RawFramingInterface) | AuthToken in frame header | `resolve_from_token()` |
| (TCP, RawFramingInterface) | AuthToken in frame header | `resolve_from_token()` |
| (WebTransport, RawFramingInterface) | AuthToken in CONNECT request | `resolve_from_token()` |
| (—, HttpInterface) | `Authorization: Bearer` header | `resolve_from_token()` |
| (—, DnsInterface) | AuthToken in query labels | `resolve_from_token()` |

The **key material is shared**. The **credential presentation** differs per
(Transport, Interface) pair. The **verification result is the same**: an
authenticated `Identity` with scopes.

`resolve_from_token()` handles both AuthTokens (Ed25519-signed) and API keys
(hash-verified bearer tokens). The implementation discriminates by prefix or
format — see ADR-037.

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

### IdentityProvider and Auth Service Relationship

The `IdentityProvider` trait (defined in [identity.md](identity.md)) decouples
alknet-core from any specific identity storage. Two implementations exist:

- **ConfigIdentityProvider** (in alknet-core) — reads from
  `ArcSwap<DynamicConfig.auth>`. Every authorized key gets a default scope set.
  No database required. This is the default for minimal deployments.

- **StorageIdentityProvider** (in alknet-storage) — backed by SQLite
  `peer_credentials` and `api_keys` tables plus the ACL graph. Resolves
  fingerprint → account → organization membership → effective scopes.

The `AuthProtocol` irpc service (behind the `irpc` feature flag, per ADR-028)
provides an async boundary for auth verification. It is one way to satisfy the
`IdentityProvider` trait, not a replacement for it. Both the trait path and the
irpc path produce the same `Identity` result.

The trait is the contract. The backing store is pluggable. Alknet-core never
depends on Honker, SQLite, or any specific database.

### API Keys

For service accounts, automation, and HTTP interface auth, Ed25519 AuthTokens
are inconvenient — they require client-side key generation and signing. API keys
provide a simpler bearer token format (ADR-037):

```
API key: "alk_dGhlX3NlY3JldA" (~20 chars, configurable prefix)
Storage: SHA-256 hash of the full key
Lookup: prefix match → hash verification → Identity
```

API keys are configured in `DynamicConfig.auth.api_keys`:

```toml
[[auth.api_keys]]
prefix = "alk_"
hash = "sha256:abc..."
scopes = ["relay:connect"]
description = "dashboard service account"
ttl = "30d"  # optional
```

Both AuthTokens and API keys go through `IdentityProvider::resolve_from_token()`.
The implementation discriminates by prefix (default `alk_`): if the token starts
with the API key prefix, it's verified by SHA-256 hash lookup; otherwise, it's
verified as an Ed25519 AuthToken. Both paths produce the same `Identity`.

See [configuration.md](configuration.md) for the full `DynamicConfig.auth`
structure and ADR-037 for the decision context.

### AuthPolicy Structure

`AuthPolicy` in `DynamicConfig` holds all auth paths, sharing key material:

```rust
pub struct AuthPolicy {
    pub ssh: SshAuthConfig,
    pub token: TokenAuthConfig,
    pub api_keys: Vec<ApiKeyEntry>,
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

pub struct ApiKeyEntry {
    pub prefix: String,                    // e.g., "alk_"
    pub hash: String,                      // e.g., "sha256:abc..."
    pub scopes: Vec<String>,               // e.g., ["relay:connect", "secrets:derive"]
    pub description: Option<String>,        // e.g., "dashboard service account"
    pub expires_at: Option<u64>,           // Unix timestamp, optional TTL
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
- API keys are bearer tokens — anyone who obtains the key has the associated
  permissions. The hash storage and optional TTL mitigate but do not eliminate
  this risk. Ed25519 AuthTokens remain the preferred auth method for interactive
  clients. See ADR-037.
- API keys are verified by SHA-256 hash lookup in `DynamicConfig.auth.api_keys`
  (or the `api_keys` database table in production). The full key is provided to
  the client exactly once at creation time.

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

- **OQ-18**: ~~Source of Identity.scopes~~ Resolved per ADR-029 and ADR-031.
  `IdentityProvider` owns scopes, `ForwardingPolicy` uses scopes from `Identity`.
  See [open-questions.md](open-questions.md).

- **OQ-19**: Should the WebTransport listener require its own TLS identity
  (separate from the SSH-over-TLS listener), or can they share the same
  certificate? Deferred to Phase 4. See [open-questions.md](open-questions.md).

## Design Decisions

| ADR | Decision | Summary |
|-----|----------|---------|
| [012](decisions/012-auth-ed25519-and-cert-authority.md) | Ed25519 + cert-authority | Key-based auth, no passwords |
| [023](decisions/023-unified-auth-shared-key-material.md) | Unified auth, shared key material | Same keys for SSH and token auth |
| [028](decisions/028-auth-irpc-service.md) | Auth as irpc service | AuthProtocol behind feature flag; IdentityProvider is the contract |
| [029](decisions/029-identity-core-type.md) | Identity as core type | `Identity` and `IdentityProvider` in alknet-core |
| [035](decisions/035-streaminterface-messageinterface-split.md) | StreamInterface/MessageInterface | Credential presentation differs per (Transport, Interface) pair |
| [037](decisions/037-api-keys-dynamic-config.md) | API keys in DynamicConfig | Hash-verified bearer tokens for service accounts |

## Phase 2 Implementation Notes

- `ConfigIdentityProvider::resolve_from_token()` now handles API keys (`alk_` prefix) via SHA-256 hash verification with expiry checking
- `ApiKeyEntry` struct added to `AuthPolicy` with prefix, hash, scopes, description, expires_at fields
- API keys produce `Identity { id: prefix, scopes: from_entry, resources: {} }`
- Both AuthTokens (Ed25519 signed) and API keys (hash-verified bearer) go through `resolve_from_token()`, discriminated by format/prefix

## References

- [identity.md](identity.md) — Canonical Identity and IdentityProvider definitions
- [server.md](server.md) — Current SSH auth handler
- [transport.md](transport.md) — Transport abstraction
- [configuration.md](configuration.md) — DynamicConfig, AuthPolicy, ConfigReloadHandle
- [interface.md](interface.md) — Credential presentation per (Transport, Interface) pair
- [definitions.md](definitions.md) — Terminology disambiguation (IdentityProvider vs CredentialProvider, AuthToken vs API key)
- [services.md](services.md) — AuthProtocol irpc service
- [open-questions.md](open-questions.md) — OQ-17 (resolved), OQ-18 (resolved), OQ-19
- [wtransport](https://github.com/BiagioFesta/wtransport) — Rust WebTransport library
- [WebTransport W3C Spec](https://www.w3.org/TR/webtransport/) — Browser API