---
status: draft
last_updated: 2026-06-16
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

Handlers that require authentication extract protocol-specific credentials and call `IdentityProvider` inside `handle()`:

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
    // ... proceed with authenticated identity
}
```

Handlers that don't require authentication (e.g., DNS resolver, health check) can ignore `auth.identity` entirely.

### AuthContext is Clone and immutable

- `derive(Clone)` allows handlers to clone `AuthContext` for per-stream or per-channel contexts.
- `handle()` receives `&AuthContext` — immutable. Handlers that resolve identity create local variables, they don't mutate the shared context. This prevents cross-contamination between streams on the same connection.

## Identity

The authenticated peer identity. Carries authorization information.

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    /// Unique identifier string. Fingerprint, key prefix, or principal name.
    pub id: String,

    /// Authorization scopes. e.g., ["relay:connect", "secrets:derive"]
    pub scopes: Vec<String>,

    /// Named resource lists. e.g., {"service": ["gitea", "registry"]}
    pub resources: HashMap<String, Vec<String>>,
}
```

This is the same structure as the reference implementation (`alknet-main/crates/alknet-core/src/auth/identity.rs`), minus the russh dependency. The `id` field is ALPN-agnostic:
- SSH key auth: `"SHA256:abc123..."` (key fingerprint)
- API key auth: `"alk_test"` (key prefix)
- Certificate auth: `"username"` (principal name)

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
- **Fingerprint**: Look up in `DynamicConfig::auth::authorized_keys_fingerprints`. If found, return `Identity { id: fingerprint, scopes: ["relay:connect"], resources: {} }`.
- **Token**: Parse as UTF-8. If it starts with `alk_`, look up in `DynamicConfig::auth::api_keys` by prefix match + SHA-256 hash. If found and not expired, return `Identity { id: prefix, scopes: entry.scopes, resources: entry.resources }`.

Changes to `DynamicConfig` via `ConfigReloadHandle` are reflected immediately — `ConfigIdentityProvider` reads from `ArcSwap` on every call.

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

## Open Questions

- **OQ-11**: See [open-questions.md](../../open-questions.md) — handler-level auth resolution observability.