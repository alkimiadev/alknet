---
status: draft
last_updated: 2026-06-27
---

# Authentication

AuthContext, Identity, IdentityProvider, AuthToken, and the resolution flow.

See [ADR-004](../../decisions/004-auth-as-shared-core.md) and [ADR-011](../../decisions/011-authcontext-structure.md) for rationale.

## AuthContext

Created by the endpoint for each incoming connection. Passed to `ProtocolHandler::handle()` as an immutable reference.

```rust
#[derive(Clone)]
pub struct AuthContext {
    /// The peer's authenticated identity, if resolved by the endpoint.
    /// None means the endpoint has no identity information for this connection.
    pub identity: Option<Identity>,

    /// The negotiated ALPN for this connection. Always present.
    pub alpn: Vec<u8>,

    /// The peer's remote address, if available. Informational (NAT/proxy).
    pub remote_addr: Option<SocketAddr>,

    /// SHA-256 fingerprint of the TLS client certificate, if presented.
    /// Set by the endpoint during TLS handshake. Handlers may use this for
    /// fingerprint-based auth even when IdentityProvider returns None.
    pub tls_client_fingerprint: Option<String>,
}
```

### Construction by the endpoint

The endpoint constructs `AuthContext` from the QUIC connection:

1. `alpn`: From `connection.alpn()` — always present after TLS handshake.
2. `remote_addr`: From `connection.remote_addr()` — may be `None` for iroh connections.
3. `tls_client_fingerprint`: Extracted from the TLS session's client certificate, if one was presented.
4. `identity`: If a TLS client fingerprint is available, the endpoint calls `IdentityProvider::resolve_from_fingerprint()`. If it resolves, `identity = Some(resolved)`. If not, `identity = None`.

### Handler-level resolution

Handlers that require authentication extract protocol-specific credentials and call `IdentityProvider` inside `handle()`. When identity is resolved, the handler stores it on the `Connection` for observability:

```rust
// Example: CallAdapter extracting an AuthToken from the first frame
async fn handle(&self, connection: Connection, auth: &AuthContext) -> Result<(), HandlerError> {
    let identity = match &auth.identity {
        Some(id) => id.clone(),  // Endpoint already resolved identity
        None => {
            let stream = connection.accept_bi().await?;
            let token = extract_auth_token(stream).await?;
            self.identity_provider
                .resolve_from_token(&token)
                .ok_or(HandlerError::AuthRequired)?
        }
    };
    connection.set_identity(identity);  // Store for observability (OQ-11)
    // ... proceed with authenticated identity
}
```

Handlers that don't require authentication (e.g., DNS resolver, health check) can ignore `auth.identity` entirely and don't call `set_identity`.

### Two Identity Scopes

There are two distinct identity scopes that must not be conflated:

| Scope | Where it's set | Where it's stored | What it represents | Used for |
|-------|---------------|-------------------|-------------------|----------|
| Connection-level | Handler in `handle()` | `Connection` (via `set_identity`) | Who opened this QUIC connection | Observability, logging, audit |
| Per-request | `CallAdapter` per `call.requested` | `OperationContext.identity` | Who is making this specific call | ACL (ADR-015) |

The connection-level identity is stable — set once when the handler resolves it. The per-request identity is dynamic — resolved per `call.requested`, potentially different across requests on the same connection (if different auth tokens are used). The per-request identity takes precedence for ACL on `OperationContext`; the connection-level identity is for observability only, not for ACL.

`Connection` exposes `set_identity` via interior mutability — the handler sets it once when resolved, the endpoint and observability layers read it. The identity is write-once-read-many.

### AuthContext is Clone and immutable

- `derive(Clone)` allows handlers to clone `AuthContext` for per-stream or per-channel contexts.
- `handle()` receives `&AuthContext` — immutable. Handlers that resolve identity create local variables, they don't mutate the shared context. This prevents cross-contamination between streams on the same connection.

## Identity

The authenticated peer identity. Carries authorization information.

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    /// Stable logical identifier. On the fingerprint path, this is the
    /// `PeerEntry.peer_id` (stable across key rotation, ADR-030). On the
    /// API-key path, this is the key prefix (changes with the key — see
    /// "API keys vs peer entries" below). On the composition path, this
    /// is the `CompositionAuthority` label (ADR-022).
    pub id: String,

    /// Authorization scopes. e.g., ["relay:connect", "secrets:derive"]
    pub scopes: Vec<String>,

    /// Named resource lists. e.g., {"service": ["gitea", "registry"]}
    /// Populated from `PeerEntry.resources` on the fingerprint path
    /// (ADR-030), from `CompositionAuthority.resources` on the
    /// composition path (ADR-022), and empty on the API-key path.
    pub resources: HashMap<String, Vec<String>>,
}
```

This is the same structure as the reference implementation (`alknet-main/crates/alknet-core/src/auth/identity.rs`), minus the russh dependency. The `id` field is ALPN-agnostic:
- Ed25519 raw key / TLS cert auth (fingerprint path): the `PeerEntry.peer_id` (ADR-030) — a stable logical name like `"worker-a"`, **not** the fingerprint. The fingerprint is the *credential*; the `peer_id` is the *identity*. Decoupling them means key rotation changes the credential but not the identity, so ACL entries and routing references stay stable.
- Bearer token auth (auth_token path): if the token is one credential path for a `PeerEntry`, `Identity.id = peer_id` (stable). If the token IS the identity (`ApiKeyEntry`), `Identity.id = prefix` (changes with the key). See "Credential Types" below.
- Composition path: the `CompositionAuthority` label (ADR-022) — e.g., `"agent-chat"`.

### Credential Types

The alknet auth model has three credential types. A `PeerEntry` can use any combination — all resolve to the same `peer_id`:

| Credential type | `PeerEntry` field | Fingerprint format | Trust model |
|-----------------|-------------------|--------------------|----|
| Ed25519 raw key (RFC 7250) | `fingerprints[i]` | `ed25519:<hex of 32-byte pub key>` | Fingerprint IS the trust anchor (no CA) |
| X.509 cert | `fingerprints[i]` | `SHA256:<hex of DER>` | CA verification (WebPKI) |
| Bearer token (peer credential) | `auth_token_hash` | SHA-256 hash of token | Token hash match |

Ed25519 fingerprints are normalized to `ed25519:<hex>` across quinn and iroh (ADR-030 §6) — the same key has the same fingerprint regardless of transport.

Bearer tokens have two paths:
- `PeerEntry.auth_token_hash` — the token is one credential path among several for a stable logical peer. Rotation = update the hash, `peer_id` stays stable.
- `ApiKeyEntry` (separate) — the token IS the identity. Rotation = new identity (new prefix). No stable logical id.

The distinction is whether the token needs a stable logical id across rotation (`PeerEntry`) or not (`ApiKeyEntry`). See ADR-030 §"Bearer tokens."

## AuthToken

Opaque authentication token carried in protocol frames.

```rust
#[derive(Debug, Clone)]
pub struct AuthToken {
    pub raw: Vec<u8>,
}
```

Unchanged from the reference implementation. The handler that extracted it knows its encoding (UTF-8 string, binary token, etc.).

## IdentityProvider

Trait for resolving credentials to identities. Implemented by `ConfigIdentityProvider`.

```rust
pub trait IdentityProvider: Send + Sync + 'static {
    fn resolve_from_fingerprint(&self, fingerprint: &str) -> Option<Identity>;
    fn resolve_from_token(&self, token: &AuthToken) -> Option<Identity>;
}
```

- `resolve_from_fingerprint()`: Used by the endpoint (TLS client cert) and by SSH (key fingerprint).
- `resolve_from_token()`: Used by call protocol (AuthToken in first frame) and HTTP (Bearer header).

Both methods return `Option<Identity>` — `None` means the credential is not recognized.

## ConfigIdentityProvider

The default implementation. Resolves identities from `DynamicConfig`:

```rust
pub struct ConfigIdentityProvider {
    dynamic: Arc<ArcSwap<DynamicConfig>>,
}
```

The "Config" prefix indicates that identities are resolved from configuration (as opposed to a database or external service). This reads from `ArcSwap<DynamicConfig>`, which is hot-reloadable — not from `StaticConfig`. An alternative name would be `DynamicConfigIdentityProvider` to make this clearer, but `ConfigIdentityProvider` is consistent with the reference implementation and the naming is unlikely to cause confusion in practice.

How it resolves:
- **Fingerprint**: Look up in `DynamicConfig::auth.peers` for the matching `PeerEntry` (by any entry in `fingerprints`). If found and `enabled`, return `Identity { id: peer.peer_id, scopes: peer.scopes, resources: peer.resources }`. The `Identity.id` is the stable `peer_id`, **not** the fingerprint — key rotation changes the fingerprint but not the `peer_id`, so ACL entries and routing references stay stable (ADR-030).
- **Token**: Hash the token and look up in `DynamicConfig::auth.peers` for a matching `auth_token_hash`. If found, return `Identity { id: peer.peer_id, ... }` — the same `peer_id` as the fingerprint path. If no `PeerEntry` matches, fall through to `ApiKeyEntry` lookup by prefix match + SHA-256 hash. If found and not expired, return `Identity { id: prefix, scopes: entry.scopes, resources: {} }` — the token IS the identity, `Identity.id` is the key prefix.

See [ADR-030](../../decisions/030-peerentry-and-identity-id-decoupling.md) for the `PeerEntry` model, the multi-credential resolution, and the fingerprint normalization rationale.

### Resource-scoped ACLs

`Identity.resources` is populated on two paths:

| Path | Source of `resources` | Use case |
|------|----------------------|----------|
| `PeerEntry` resolution (fingerprint or auth_token) | `PeerEntry.resources` (ADR-030) | External authenticated callers with per-peer resource binding |
| Composition (`CompositionAuthority::as_identity`, ADR-015/022) | `CompositionAuthority.resources` | Internal composition calls with declared resource binding |

`ApiKeyEntry`-resolved identities have empty `resources` — API keys grant scopes only. An `OperationSpec` that declares `resource_type`/`resource_action` returns `FORBIDDEN` when the caller authenticated via `ApiKeyEntry`, but succeeds when the caller authenticated via `PeerEntry` (fingerprint or auth_token) with matching `resources`.

Changes to `DynamicConfig` via `ConfigReloadHandle` are reflected immediately — `ConfigIdentityProvider` reads from `ArcSwap` on every call.

### Fingerprint string format

`tls_client_fingerprint` and `PeerEntry.fingerprints` entries use a
prefixed-hex format. The prefix identifies the key type; the body is the
hex-encoded key material. `AuthPolicy::resolve_identity_from_fingerprint`
scans `peers` for a matching `fingerprints` entry — no normalization — so
the extractor and the operator config must use the same format.

| Transport | Source | Format |
|-----------|--------|--------|
| iroh (direct or relay) | peer `NodeId` (Ed25519 public key) | `ed25519:<lowercase hex of 32-byte pub key>` |
| quinn (RFC 7250 raw key) | SPKI cert → extract raw Ed25519 pub key | `ed25519:<lowercase hex of 32-byte pub key>` (normalized — ADR-030 §6) |
| quinn (X.509) | leaf client cert DER | `SHA256:<hex of SHA-256(cert_der)>` |

Ed25519 raw keys produce `ed25519:<hex>` regardless of transport (quinn or
iroh) — the same key has the same fingerprint. X.509 certs produce
`SHA256:<hex of DER>` — the DER hash, since X.509 doesn't have a "raw
public key" form.

When no client cert is presented (the current default — server uses
`with_no_client_auth()`), the fingerprint is `None` and identity remains
unresolved at the endpoint layer. The `CallClient` TLS client-auth wiring
(OQ-29, resolved) presents the client's Ed25519 key as a raw public key
client cert so the server can extract the fingerprint.

### Server-side client cert request

The quinn `rustls::ServerConfig` uses a custom `AcceptAnyCertVerifier`
that requests client certs but does not require them and does not verify
them against a CA. This is the "request-but-don't-require" mode: peers
that present a cert (X.509 or RFC 7250 raw key) have their fingerprint
extracted via `peer_identity()`; peers that don't present a cert connect
normally with `tls_client_fingerprint: None`.

The verifier accepts any presented cert without CA verification because
alknet's identity model is fingerprint-based, not PKI-based — the
`AuthPolicy::peers` set is the trust anchor, not a root CA store. The
cert bytes are extracted at the TLS layer and hashed to a fingerprint
string; the fingerprint is then matched against the configured `PeerEntry.fingerprint`
fields by `IdentityProvider::resolve_from_fingerprint()`.

## Resolution Flow

### Endpoint-level (before `handle()`)

```
QUIC connection arrives
  → TLS handshake (ALPN negotiation)
  → Extract TLS client certificate fingerprint (if presented)
  → If fingerprint present: IdentityProvider::resolve_from_fingerprint()
    → Some(identity): auth.identity = Some(identity)
    → None: auth.identity = None
  → Construct AuthContext { identity, alpn, remote_addr, tls_client_fingerprint }
  → Look up handler by alpn
  → tokio::spawn(handler.handle(connection, &auth))
```

### Handler-level (inside `handle()`)

```
Handler receives &AuthContext
  → If auth.identity is Some: use it (endpoint already resolved)
  → If auth.identity is None and handler requires auth:
    → Extract protocol-specific credential (AuthToken, SSH key, etc.)
    → Call IdentityProvider::resolve_from_token() or resolve_from_fingerprint()
    → If resolved: use the Identity
    → If not resolved: return HandlerError::AuthRequired
  → If handler doesn't require auth: proceed without identity
```

## IdentityProvider Injection

Handlers need access to `IdentityProvider` to resolve credentials inside `handle()`. Since `ProtocolHandler::handle()` doesn't receive an `IdentityProvider` parameter, each handler must obtain it through **constructor injection**:

```rust
// Example: SshAdapter holds an Arc<dyn IdentityProvider>
pub struct SshAdapter {
    identity_provider: Arc<dyn IdentityProvider>,
    // ... other handler-specific state
}

#[async_trait]
impl ProtocolHandler for SshAdapter {
    fn alpn(&self) -> &'static [u8] { b"alknet/ssh" }

    async fn handle(&self, connection: Connection, auth: &AuthContext) -> Result<(), HandlerError> {
        let identity = match &auth.identity {
            Some(id) => id.clone(),
            None => {
                // Extract SSH key fingerprint, resolve via identity_provider
                let fingerprint = extract_ssh_fingerprint(&connection).await?;
                self.identity_provider
                    .resolve_from_fingerprint(&fingerprint)
                    .ok_or(HandlerError::AuthRequired)?
            }
        };
        // ...
    }
}
```

The CLI binary constructs each handler with `Arc::clone(&identity_provider)` and passes it when building the `HandlerRegistry`. This is the **assembly pattern**: the CLI (the only crate that depends on all handlers) wires dependencies together.

The endpoint's `AlknetEndpoint` also holds `Arc<dyn IdentityProvider>` for endpoint-level auth resolution (TLS client certificate fingerprints), but handlers don't receive it from the endpoint — they receive it at construction time from the CLI.

| Handler | Credential source | Resolution method |
|---------|------------------|-----------------|
| SshAdapter | SSH public key handshake | `resolve_from_fingerprint()` |
| CallAdapter | AuthToken in first frame | `resolve_from_token()` |
| HttpAdapter | `Authorization: Bearer` header | `resolve_from_token()` |
| DnsAdapter | AuthToken in query labels | `resolve_from_token()` |
| GitAdapter | Signed push certificate | `resolve_from_fingerprint()` |
| SftpAdapter | SSH key (shares with SshAdapter) | `resolve_from_fingerprint()` |

## Key Differences from Reference Implementation

| Aspect | Reference | New Model |
|--------|-----------|-----------|
| Auth resolution | Inside SSH handler, before `handle()` | Hybrid: endpoint resolves TLS-level, handler resolves protocol-level |
| AuthContext type | None (just `Arc<ArcSwap<DynamicConfig>>` + `IdentityProvider`) | Explicit struct with optional fields |
| `Identity.id` | Always a fingerprint or API key prefix | Same, but ALPN-agnostic documentation |
| `ConfigIdentityProvider` | Depends on russh for `PublicKey` types | No russh dependency; fingerprints stored as strings |
| Credential phases | A–D phases in `CredentialProvider` | Two paths: fingerprint and token. No phases. |

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| Hybrid auth model | [ADR-004](../../decisions/004-auth-as-shared-core.md) | Endpoint resolves TLS-level, handler resolves protocol-level |
| AuthContext with optional Identity | [ADR-011](../../decisions/011-authcontext-structure.md) | Explicit None, not "partially authenticated" |
| AuthContext is immutable in handle() | [ADR-011](../../decisions/011-authcontext-structure.md) | Handlers create local variables for resolved identity |
| Two resolution paths | [ADR-004](../../decisions/004-auth-as-shared-core.md) | Fingerprint and token, not phased auth |
| Handler stores resolved identity on Connection | OQ-11 (resolved) | `connection.set_identity()` — write-once-read-many for observability |
| PeerEntry and Identity.id decoupling | [ADR-030](../../decisions/030-peerentry-and-identity-id-decoupling.md) | `authorized_fingerprints` → `peers: Vec<PeerEntry>`; `Identity.id` = `peer_id` (stable), not fingerprint; key rotation changes fingerprint, not identity |
| CredentialStore repo trait | [ADR-031](../../decisions/031-credentialstore-repo-trait.md) | Second repo trait in core (alongside `IdentityProvider`); `InMemoryCredentialStore` default adapter |
| Storage boundary and repo/adapter pattern | [ADR-033](../../decisions/033-storage-boundary-and-repo-adapter-pattern.md) | Core defines traits + in-memory defaults; persistence adapters are separate crates |

## Open Questions

- **OQ-29** (resolved): `CallClient` TLS client-auth — wire quinn client-auth (present Ed25519 key as raw public key client cert); key-type-aware server cert verification (raw key = fingerprint match, X.509 = CA verification); fingerprint normalization (`ed25519:` across quinn/iroh). See OQ-29 in open-questions.md.
- **OQ-35** (dissolved): the "API key asymmetry" framing was wrong; `PeerEntry` supports multiple credential paths (fingerprints + auth_token_hash), `ApiKeyEntry` is for tokens that ARE the identity. See OQ-35 in open-questions.md.
- **OQ-37** (open): X.509 outgoing-only case — the three auth types and how X.509 server identity fits the peer model. Not blocking the ADR-029 migration. See OQ-37 in open-questions.md.

## Security Constraints

These are security-critical implementation requirements, not architectural decisions (the architecture is locked by the ADRs above). They are documented here so implementation agents don't miss them.

- **Token entropy**: generated `alk_` tokens must have ≥128 bits of entropy. The prefix (first 8 chars) is for O(1) lookup and is not secret — it appears in logs by design. SHA-256 of the full token allows offline verification; this is safe only if the full token is high-entropy. The prefix alone must not be sufficient to authenticate.
- **Config reload must be authenticated**: a reload that adds an authorized fingerprint or API key grants access immediately (see [config.md](config.md)). The reload trigger must be local-only (SIGHUP, file watch) or an admin-scoped call protocol operation. A malicious reload is equivalent to root-level privilege grant.
- **Connection-level identity is for observability only**: `Connection::set_identity` stores the handler-resolved identity for logging/audit. Per-request identity (`OperationContext.identity`) takes precedence for ACL. See OQ-11.
- **Cryptographic nonces use OsRng**: AES-GCM IVs and any other cryptographic nonces must use `OsRng` (or equivalent CSPRNG), not `rand::random()`. IV reuse under the same key is catastrophic for GCM (authenticity breaks, two-time-pad on plaintext). The vault implementation (`crates/alknet-vault/src/encryption.rs`) must use `OsRng` for IV generation.
- **Derived keys are zeroized on drop**: cached derived keys (`CachedKey`) must derive `Zeroize` and `ZeroizeOnDrop`. When the cache evicts an entry (LRU) or the process exits without explicit `lock()`, derived private keys must not linger in freed heap memory. The cache must clear on drop, not just on explicit `lock()`.
- **No `unwrap()` or `expect()` outside tests**: poisoned lock recovery uses `unwrap_or_else(|e| e.into_inner())` or explicit error propagation. A panic in one vault operation must not brick the vault for all other operations.