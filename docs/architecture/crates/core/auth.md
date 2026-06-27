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
- SSH key / TLS cert auth (fingerprint path): the `PeerEntry.peer_id` (ADR-030) — a stable logical name like `"worker-a"`, **not** the fingerprint. The fingerprint is the *credential*; the `peer_id` is the *identity*. Decoupling them means key rotation changes the credential but not the identity, so ACL entries and routing references stay stable.
- API key auth: `"alk_test"` (key prefix) — the prefix IS the identity; rotation = new identity (see "API keys vs peer entries" below).
- Composition path: the `CompositionAuthority` label (ADR-022) — e.g., `"agent-chat"`.

### API keys vs peer entries

The fingerprint and API-key auth paths have different identity semantics, by design (ADR-030):

| Axis | Fingerprint (PeerEntry) | API key (ApiKeyEntry) |
|------|-------------------------|------------------------|
| Identity source | TLS handshake / SSH key | Bearer token in protocol frame |
| Key rotation | Same logical node, new material | New identity (revocation = new key) |
| `Identity.id` | `peer_id` (stable across rotation) | `prefix` (changes with the key) |
| `Identity.resources` | Populated from `PeerEntry.resources` | Empty (resources are composition-only) |

An API key's prefix IS the identity — rotating the key means a new prefix and a new identity, by design (revocation is the rotation mechanism for API keys). Decoupling the API key identity from the prefix would solve a problem API keys don't have: they're bearer tokens, not node identities. The fingerprint path gets the `PeerEntry` treatment because node identity must survive key rotation; the API-key path doesn't because bearer-token identity IS the token. The asymmetry is deliberate, not an oversight — see ADR-030 §"API keys".

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
- **Fingerprint**: Look up in `DynamicConfig::auth.peers` for the matching `PeerEntry` (by `fingerprint`). If found and `enabled`, return `Identity { id: peer.peer_id, scopes: peer.scopes, resources: peer.resources }`. The `Identity.id` is the stable `peer_id`, **not** the fingerprint — key rotation changes the fingerprint but not the `peer_id`, so ACL entries and routing references stay stable (ADR-030).
- **Token**: Parse as UTF-8. If it starts with `alk_`, look up in `DynamicConfig::auth.api_keys` by prefix match + SHA-256 hash. If found and not expired, return `Identity { id: prefix, scopes: entry.scopes, resources: {} }`. The `Identity.id` is the key prefix — API key rotation = new identity (see "API keys vs peer entries" above).

See [ADR-030](../../decisions/030-peerentry-and-identity-id-decoupling.md) for the `PeerEntry` model and the id-fingerprint decoupling rationale.

### Resource-scoped ACLs

`Identity.resources` is populated on three paths:

| Path | Source of `resources` | Use case |
|------|----------------------|----------|
| Fingerprint resolution (`ConfigIdentityProvider`) | `PeerEntry.resources` (ADR-030) | External fingerprint-authenticated callers with per-peer resource binding |
| API key resolution (`ConfigIdentityProvider`) | Empty (by design) | API keys grant scopes only; resource-scoped access is composition-only |
| Composition (`CompositionAuthority::as_identity`, ADR-015/022) | `CompositionAuthority.resources` | Internal composition calls with declared resource binding |

An `OperationSpec` that declares `resource_type`/`resource_action` will return `FORBIDDEN` when the caller authenticated via API key (because `Identity.resources` is empty), but succeeds when the caller authenticated via fingerprint with matching `PeerEntry.resources`, or via composition with matching `CompositionAuthority.resources`. The API-key limitation is deliberate (see "API keys vs peer entries" above); the fingerprint path's resource binding is the ADR-030 change that lifts the pre-ADR-030 limitation.

Changes to `DynamicConfig` via `ConfigReloadHandle` are reflected immediately — `ConfigIdentityProvider` reads from `ArcSwap` on every call.

### Fingerprint string format

`tls_client_fingerprint` and `PeerEntry.fingerprint` use a prefixed-hex
format. The prefix identifies the key type; the body is the hex-encoded
hash or raw key bytes. `AuthPolicy::resolve_identity_from_fingerprint`
scans `peers` for a matching `fingerprint` field — no normalization — so
the extractor and the operator config must use the same format.

| Transport | Source | Format |
|-----------|--------|--------|
| quinn (X.509) | leaf client cert DER | `SHA256:<hex of SHA-256(cert_der)>` |
| iroh (raw Ed25519) | peer `NodeId` | `ed25519:<lowercase hex of 32-byte pub key>` |

When no client cert is presented (the current default — server uses
`with_no_client_auth()`), the fingerprint is `None` and identity remains
unresolved at the endpoint layer. A follow-up task will switch the server
config to request-but-not-require client certs so fingerprints flow for
peers that present them.

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

- **OQ-35**: API key identity vs peer identity — the asymmetry between the fingerprint path (gets `PeerEntry` id-decoupling) and the API-key path (doesn't) is deliberate. See ADR-030 §"API keys" and "API keys vs peer entries" above.

## Security Constraints

These are security-critical implementation requirements, not architectural decisions (the architecture is locked by the ADRs above). They are documented here so implementation agents don't miss them.

- **Token entropy**: generated `alk_` tokens must have ≥128 bits of entropy. The prefix (first 8 chars) is for O(1) lookup and is not secret — it appears in logs by design. SHA-256 of the full token allows offline verification; this is safe only if the full token is high-entropy. The prefix alone must not be sufficient to authenticate.
- **Config reload must be authenticated**: a reload that adds an authorized fingerprint or API key grants access immediately (see [config.md](config.md)). The reload trigger must be local-only (SIGHUP, file watch) or an admin-scoped call protocol operation. A malicious reload is equivalent to root-level privilege grant.
- **Connection-level identity is for observability only**: `Connection::set_identity` stores the handler-resolved identity for logging/audit. Per-request identity (`OperationContext.identity`) takes precedence for ACL. See OQ-11.
- **Cryptographic nonces use OsRng**: AES-GCM IVs and any other cryptographic nonces must use `OsRng` (or equivalent CSPRNG), not `rand::random()`. IV reuse under the same key is catastrophic for GCM (authenticity breaks, two-time-pad on plaintext). The vault implementation (`crates/alknet-vault/src/encryption.rs`) must use `OsRng` for IV generation.
- **Derived keys are zeroized on drop**: cached derived keys (`CachedKey`) must derive `Zeroize` and `ZeroizeOnDrop`. When the cache evicts an entry (LRU) or the process exits without explicit `lock()`, derived private keys must not linger in freed heap memory. The cache must clear on drop, not just on explicit `lock()`.
- **No `unwrap()` or `expect()` outside tests**: poisoned lock recovery uses `unwrap_or_else(|e| e.into_inner())` or explicit error propagation. A panic in one vault operation must not brick the vault for all other operations.