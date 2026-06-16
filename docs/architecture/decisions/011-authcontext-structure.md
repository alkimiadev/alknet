# ADR-011: AuthContext Structure and Resolution Flow

## Status

Proposed

## Context

ADR-004 establishes the hybrid auth model: the endpoint resolves what it can (TLS client certificate fingerprint), handlers resolve what they must (AuthToken in the first frame, Bearer header, SSH key fingerprint). The `AuthContext` passed to `handle()` may be partial.

The reference implementation's `Identity` struct is:

```rust
pub struct Identity {
    pub id: String,
    pub scopes: Vec<String>,
    pub resources: HashMap<String, Vec<String>>,
}
```

And `ConfigIdentityProvider` resolves fingerprints and API keys to `Identity`. This works well and carries forward.

But the reference implementation has no `AuthContext` type — auth resolution happens inside the SSH handler before calling `IdentityProvider`. The new model needs a type that represents "what the endpoint knows about this connection's identity before the handler starts," plus a way for handlers to enrich it.

This is a one-way door: once handlers depend on `AuthContext`'s structure, changing it affects every handler. The structure must be right.

### Design considerations

1. **Handlers need identity information to make authorization decisions.** A handler that requires authentication needs to know: is the peer authenticated? Who are they? What scopes do they have?

2. **The endpoint may have zero, partial, or complete identity information.** A plain QUIC connection with no TLS client cert gives the endpoint nothing. A TLS connection with a client cert gives the endpoint a fingerprint that may resolve to an Identity. A handler that extracts an AuthToken from the first frame can complete the resolution.

3. **AuthContext must not be SSH-specific.** The reference implementation's auth types are tangled with russh (SSH key fingerprints, certificate authorities). The new model needs to be ALPN-agnostic.

4. **AuthContext is constructed by the endpoint and enriched by handlers.** The endpoint creates it from TLS-level information. The handler mutates or replaces it with protocol-level information.

5. **AuthContext must be cheap to construct.** Every incoming connection gets one, even if authentication ultimately fails.

## Decision

### AuthContext is a struct with optional fields

```rust
pub struct AuthContext {
    /// The peer's authenticated identity, if resolved.
    /// None means the endpoint has no identity information for this connection.
    /// Some(Identity) means the endpoint resolved the peer's identity.
    pub identity: Option<Identity>,

    /// The negotiated ALPN for this connection.
    /// Always present — the endpoint sets this from the TLS handshake.
    pub alpn: Vec<u8>,

    /// The peer's remote address, if available.
    pub remote_addr: Option<SocketAddr>,

    /// TLS client certificate fingerprint, if the client presented a certificate.
    /// Set by the endpoint during TLS handshake. Handlers may use this for
    /// SSH host key verification or other fingerprint-based auth.
    pub tls_client_fingerprint: Option<String>,
}
```

Key design points:

- `identity: Option<Identity>` — not `Identity` with optional fields, not a separate `PartialAuthContext`. The endpoint sets it to `None` if it has no identity information, or `Some(identity)` if it resolved one. Handlers that need to complete auth call `IdentityProvider` themselves and store the resolved identity in a local variable — they do NOT mutate AuthContext (see immutability section below).
- `alpn` is always present — every connection has a negotiated ALPN.
- `remote_addr` is informational. It's available from the QUIC connection and useful for logging and rate limiting, but it's not authoritative (clients can be behind NATs/proxies).
- `tls_client_fingerprint` captures the TLS-level credential. If present, it's the SHA-256 fingerprint of the client's TLS certificate. This is separate from `identity` because a handler might need the fingerprint even when `IdentityProvider::resolve_from_fingerprint()` returns `None` (e.g., unknown cert, but the handler wants to log it).

### AuthContext is Clone

`AuthContext` derives `Clone`. Handlers can clone it for per-stream or per-channel contexts within a connection. The `Identity` inside is also `Clone`.

### Handler-level auth enrichment pattern

Handlers that need to complete authentication do so inside `handle()`:

```rust
async fn handle(&self, connection: Connection, auth: &AuthContext) -> Result<(), HandlerError> {
    let identity = if let Some(id) = &auth.identity {
        id.clone()  // Endpoint already resolved identity
    } else {
        // Extract credentials from the protocol, resolve via IdentityProvider
        let token = self.extract_auth_token(&connection).await?;
        self.identity_provider.resolve_from_token(&token)
            .ok_or(HandlerError::AuthRequired)?
    };
    // ... proceed with authenticated identity
}
```

Handlers that don't need authentication (e.g., DNS resolver, health check) can ignore `auth.identity` entirely.

### Identity carries over from reference implementation

```rust
pub struct Identity {
    pub id: String,
    pub scopes: Vec<String>,
    pub resources: HashMap<String, Vec<String>>,
}
```

This is the same structure from the reference implementation, minus the russh dependency. It's ALPN-agnostic:
- `id`: A unique identifier string. For SSH key auth, this is the SHA-256 fingerprint. For API key auth, this is the key prefix. For certificate auth, this is the principal name.
- `scopes`: Authorization scopes. `["relay:connect", "secrets:derive"]` etc.
- `resources`: Named resource lists. `{"service": ["gitea", "registry"]}` etc.

### AuthToken carries raw bytes

```rust
pub struct AuthToken {
    pub raw: Vec<u8>,
}
```

Unchanged from the reference implementation. Opaque bytes — the handler that extracted it knows its encoding.

### IdentityProvider carries over with minor adaptation

```rust
pub trait IdentityProvider: Send + Sync + 'static {
    fn resolve_from_fingerprint(&self, fingerprint: &str) -> Option<Identity>;
    fn resolve_from_token(&self, token: &AuthToken) -> Option<Identity>;
}
```

The implementation (`ConfigIdentityProvider`) changes from the reference: it no longer depends on russh types for key storage. Instead, it stores fingerprint strings and API key entries, drawing from `DynamicConfig` via `ArcSwap`.

### AuthContext is NOT mutable inside handle()

The `handle()` signature passes `&AuthContext` (immutable reference). Handlers that resolve identity create a local variable with the resolved identity — they don't mutate the AuthContext. This prevents accidental cross-contamination between streams on the same connection.

## Consequences

**Positive:**
- `AuthContext` is a value type — cheap to construct, clone, and pass around
- Handlers that don't need auth can ignore it entirely
- The endpoint provides what it can for free (TLS client cert fingerprint), handlers complete what they need
- No russh dependency in AuthContext — it's ALPN-agnostic
- `Option<Identity>` is explicit — there's no "partially authenticated" state that handlers have to interpret
- Handlers that need to enrich auth create local variables, not mutation — clean data flow

**Negative:**
- Handlers that need auth must call `IdentityProvider` themselves — this is intentional (ADR-004 hybrid model) but means each handler has its own auth extraction logic
- `tls_client_fingerprint` is separate from `identity` — a handler might wonder "why do I have a fingerprint but no identity?" This happens when the client presents a cert that's not in the authorized keys. The handler can log the fingerprint for debugging.
- `AuthContext` doesn't carry protocol-specific auth state (e.g., SSH auth method, HTTP auth scheme). This is by design — protocol-specific details belong inside the handler, not in the shared auth context.

## References

- ADR-002: ProtocolHandler trait
- ADR-004: Auth as shared core (IdentityProvider, hybrid auth model)
- ADR-007: BiStream type definition (Connection parameter)
- ADR-010: ALPN router and endpoint (where AuthContext is created)
- Reference implementation: `alknet-main/crates/alknet-core/src/auth/identity.rs`