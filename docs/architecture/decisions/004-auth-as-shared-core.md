# ADR-004: Auth as Shared Core (IdentityProvider)

## Status

Accepted

## Context

The previous architecture had authentication spread across multiple layers: `CredentialProvider` with four phases (A–D), `AuthProtocol` as an irpc service, `server_auth` and `client_auth` as separate modules, and `IdentityProvider` as a trait in alknet-core. Different interface types presented credentials differently — SSH used key fingerprints, HTTP used Bearer tokens, DNS used query labels — but the resolution was ad-hoc and tied to the three-layer model.

The ALPN dispatch model simplifies this: every handler receives the same `AuthContext`, but the credential extraction (how a handler learns who the peer is) differs per ALPN. The resolution (turning a credential into an `Identity`) should be shared across all handlers.

## Decision

> **Note**: The original text of this decision described the handler
> "enriching or replacing" the `AuthContext`. This was superseded by
> ADR-011, which made `AuthContext` immutable in `handle()` (passed as
> `&AuthContext`). Handlers resolve identity into a local variable and
> store it on `Connection` via `set_identity()`. The text below has been
> updated to reflect the ADR-011 model.

Authentication and identity resolution live in `alknet-core` as shared infrastructure. Each handler presents credentials differently, but all resolve through the same `IdentityProvider`:

```rust
pub trait IdentityProvider: Send + Sync + 'static {
    fn resolve_from_fingerprint(&self, fingerprint: &str) -> Option<Identity>;
    fn resolve_from_token(&self, token: &AuthToken) -> Option<Identity>;
}
```

Credential presentation per handler:

| Handler | Credential presentation | Resolves via |
|---------|------------------------|-------------|
| SshAdapter | SSH public key handshake | `resolve_from_fingerprint()` |
| CallAdapter | AuthToken in first frame | `resolve_from_token()` |
| HttpAdapter | `Authorization: Bearer` header | `resolve_from_token()` |
| DnsAdapter | AuthToken in query labels | `resolve_from_token()` |
| WebTransportAdapter | AuthToken in CONNECT headers | `resolve_from_token()` |
| GitAdapter | Signed push certificate | `resolve_from_fingerprint()` |

Auth resolution is **hybrid** — the endpoint resolves what it can, and handlers resolve what they must:

1. **Endpoint-level resolution** (before `handle()` is called): If the TLS handshake provides a client certificate, the endpoint resolves the fingerprint to an `Identity` and passes it in `AuthContext`. This is the case for SSH (where the key exchange happens at the protocol level, but the TLS layer may also provide information).

2. **Handler-level resolution** (inside `handle()`): For protocols that carry credentials in application frames (AuthToken in the first call frame, Bearer header in HTTP), the handler extracts the credential from the stream and calls `IdentityProvider` to resolve it. The handler then resolves the `Identity` into a local variable and stores it on the `Connection` via `set_identity()` for observability — it does **not** mutate the `AuthContext` (which is passed as `&AuthContext`, an immutable reference — see ADR-011). The per-request identity (for ACL) is resolved separately by the `CallAdapter` at `call.requested` time.

The `AuthContext` passed to `handle()` may be partial — containing only transport-level information if no TLS client certificate was provided. Handlers must not assume `AuthContext` contains a fully resolved `Identity`. Each handler knows its own credential extraction protocol and is responsible for completing authentication.

The `CredentialProvider` concept from the previous architecture is simplified: there is no phase progression (A–D). The `IdentityProvider` has two resolution paths — fingerprint and token — and a `ConfigIdentityProvider` implementation that draws from static and dynamic config.

`alknet-vault` stays standalone. It does not depend on `alknet-core` or `IdentityProvider`. The vault provides derived keys on request; identity resolution is a separate concern.

## Consequences

**Positive:**
- Unified identity model — every handler resolves identities the same way through `IdentityProvider`
- Handlers own their credential extraction — SSH reads key fingerprints, call reads AuthTokens, HTTP reads Bearer headers
- Endpoint provides what it can for free (TLS-level auth), handlers complete what they need
- Adding a new credential type is adding a method to `IdentityProvider`, not a new phase
- alknet-secret stays standalone — no coupling between key derivation and identity resolution
- `AuthContext` is a value type — easy to construct in tests, can be partial for handler-level testing

**Negative:**
- `IdentityProvider` is in alknet-core — any change to it recompiles all handlers (mitigated: the trait should be stable; implementation changes don't force recompiles)
- Two resolution paths (fingerprint, token) may not cover all future auth schemes (mitigated: the trait can be extended, or a handler can do custom resolution after the initial AuthContext)
- Handlers must handle partial AuthContext — the endpoint may not have resolved an Identity, so handlers must be prepared to do credential extraction themselves
- WebTransport and browser-based auth needs careful design — AuthToken in CONNECT headers requires the token to be available before the stream is established

## References

- Pivot proposal: `docs/research/pivot/alpn-service-architecture.md`
- ADR-002: ProtocolHandler trait
- ADR-003: Crate decomposition
- ADR-005: irpc as call protocol foundation
- The previous architecture had equivalent decisions in ADR-023 (unified auth) and ADR-029 (identity as core type), which are archived in the reference implementation at `/workspace/@alkdev/alknet-main/`.