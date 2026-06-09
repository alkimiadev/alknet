# ADR-036: CredentialProvider as Core Type

## Status
Accepted

## Context

Alknet's `IdentityProvider` resolves **inbound** authentication: given a
credential (fingerprint or token), produce an `Identity`. But there is no
corresponding abstraction for **outbound** credentials: how does alknet
authenticate _to_ external services (vast.ai, rustfs, gitea)?

Without `CredentialProvider`, each service wrapper would independently solve
credential retrieval, caching, and lifecycle management. This leads to
duplicated effort and inconsistent security practices across service wrappers.

The pattern mirrors the existing `IdentityProvider` pattern: trait in core,
default impl using simple storage, production impl using the secret service
and database.

## Decision

Define `CredentialProvider` trait and `CredentialSet` enum in
`alknet_core::credentials`.

```rust
pub trait CredentialProvider: Send + Sync + 'static {
    fn get_credentials(&self, service: &str) -> Option<CredentialSet>;
    fn refresh_credentials(&self, service: &str) -> Option<CredentialSet>;
}

pub enum CredentialSet {
    ApiKey { header_name: String, token: String },
    Basic { username: String, password: String },
    Bearer { token: String },
    S3AccessKey { access_key: String, secret_key: String, session_token: Option<String> },
    OidcToken { access_token: String, refresh_token: Option<String>, expires_at: Option<u64> },
    Custom { scheme: String, params: HashMap<String, String> },
}
```

The trait is intentionally narrow. It returns credentials for a named service.
It does not try to abstract the auth mechanism itself — that stays with the
service wrapper that knows the protocol (S3 signing, OAuth2 refresh, etc.).

Phase 1 provides `SecretStoreCredentialProvider` (reads from
`SecretProtocol::Decrypt`, holds in RAM). Phase 2+ adds
`ManagedCredentialProvider` (with `CredentialManager` for lifecycle management:
refresh, expiration, provisioning).

`CredentialProvider` does not depend on `IdentityProvider`, though
`ManagedCredentialProvider` may use `Identity.id` for identity-bound credential
lookups.

## Consequences

**Positive**: Outbound auth has a unified abstraction, just as inbound auth
has `IdentityProvider`. Service wrappers retrieve credentials through one
interface. `OperationEnv` can expose credentials through `context.env`.

**Positive**: The `CredentialSet` enum covers all identified credential types
(API keys, bearer tokens, S3 access keys, OIDC tokens, basic auth, custom).
This is sufficient for Phases A-C. Phase D (alknet as OIDC provider) is additive.

**Positive**: The trait in core, impl in service crate pattern is consistent
with `IdentityProvider` (trait in core, `ConfigIdentityProvider` in core,
`StorageIdentityProvider` in alknet-storage).

**Negative**: Adds a new core type and a new module (`credentials`). But this
is the same pattern as `IdentityProvider` and `auth` — a small, narrow trait
with a clear contract.

**Negative**: `ManagedCredentialProvider` and `CredentialManager` are Phase C
concepts. The spec should define them as future extensions, not implement them
now.

## References

- ADR-029 (Identity as core type — same pattern)
- [credentials.md](../credentials.md) — CredentialProvider spec
- [research/phase2/credential-provider.md](../../research/phase2/credential-provider.md) — Full analysis
- [identity.md](../identity.md) — IdentityProvider (inbound, opposite direction)