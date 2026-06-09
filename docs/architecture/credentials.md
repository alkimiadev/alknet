---
status: draft
last_updated: 2026-06-09
---

# Credentials (Outbound Auth)

## What

The `CredentialProvider` trait and `CredentialSet` enum handle **outbound**
authentication: how alknet authenticates _to_ external and self-hosted services.
This is the complement to `IdentityProvider`, which handles **inbound**
authentication (who is calling alknet).

## Why

Without `CredentialProvider`, each service wrapper would independently solve
credential retrieval, caching, and lifecycle management. Cloud API integrations
(vast.ai, runpod) need API keys. Self-hosted services (rustfs, gitea) need
S3 access keys or OIDC tokens. The secret service can store these at rest, but
the wiring between "decrypt a credential from storage" and "use it in an HTTP
request" doesn't exist yet.

`CredentialProvider` provides a unified abstraction — just as `IdentityProvider`
unifies inbound auth, `CredentialProvider` unifies outbound auth. Handlers
access credentials through `OperationEnv`, not by reaching into storage directly.

## Architecture

### Direction: Inbound vs Outbound

| | IdentityProvider | CredentialProvider |
|---|---|---|
| **Direction** | Inbound (who is calling alknet) | Outbound (how alknet calls others) |
| **Resolves** | Fingerprint/token → `Identity` | Service name → `CredentialSet` |
| **Storage** | `peer_credentials`, `api_keys` | Encrypted nodes in metagraph |
| **Lifecycle** | Stateless lookup | May need refresh (OIDC tokens, S3 sessions) |
| **Location** | `alknet_core::auth` | `alknet_core::credentials` |

Both live at the same architectural layer. A handler receives an
`OperationContext` with `identity` (who called us) and can access credentials
through `context.env` (how we call out).

### CredentialProvider Trait

```rust
pub trait CredentialProvider: Send + Sync + 'static {
    fn get_credentials(&self, service: &str) -> Option<CredentialSet>;
    fn refresh_credentials(&self, service: &str) -> Option<CredentialSet>;
}
```

The trait is intentionally narrow. It returns credentials for a named service.
It does not abstract the auth mechanism — that stays with the service wrapper
that knows the protocol (S3 signing, OAuth2 refresh, etc.).

### CredentialSet

```rust
pub enum CredentialSet {
    ApiKey {
        header_name: String,
        token: String,
    },
    Basic {
        username: String,
        password: String,
    },
    Bearer {
        token: String,
    },
    S3AccessKey {
        access_key: String,
        secret_key: String,
        session_token: Option<String>,
    },
    OidcToken {
        access_token: String,
        refresh_token: Option<String>,
        expires_at: Option<u64>,
    },
    Custom {
        scheme: String,
        params: HashMap<String, String>,
    },
}
```

Each variant carries the data needed for a specific auth mechanism. The service
wrapper that requested the credentials knows what variant it expects and how to
use it.

### CredentialProvider vs IdentityProvider

These are opposite-direction abstractions that compose through `OperationEnv`:

```
  Incoming Request
      │
      ▼
  IdentityProvider (credential → Identity)
      │
      ├── SSH fingerprint → Identity.id, .scopes, .resources
      ├── Bearer AuthToken  → Identity.id, .scopes, .resources
      └── API key         → Identity.id, .scopes, .resources
      │
      ▼
  OperationContext { identity, env, ... }
      │
      ├── context.env.invoke("git", "push", input)
      │      └── GitService handler
      │           └── CredentialProvider (outbound)
      │                └── get_credentials("rustfs")
      │                     └── S3AccessKey { access_key, secret_key }
      │
      └── context.env.invoke("secrets", "derive", input)
             └── local dispatch to SecretProtocol

  Two directions: Inbound (who is calling us)
                  Outbound (how we call others)
```

### SecretStoreCredentialProvider (Phase 1 Default)

The default `CredentialProvider` implementation. Decrypts credentials via
`SecretProtocol::Decrypt` and holds them in RAM:

```rust
pub struct SecretStoreCredentialProvider {
    credentials: ArcSwap<HashMap<String, CredentialSet>>,
}
```

At startup, the CLI or NAPI assembly loads credentials from the secret service
and populates the `ArcSwap`. The `refresh_credentials()` method re-decrypts
after a `Lock`/`Unlock` cycle on the secret service.

### ManagedCredentialProvider (Phase C Future)

For self-hosted services that need active lifecycle management (S3 session
token rotation, OIDC token refresh). Wraps `SecretStoreCredentialProvider`
with per-service `CredentialManager` instances:

```rust
pub struct ManagedCredentialProvider {
    base: SecretStoreCredentialProvider,
    managers: HashMap<String, Arc<dyn CredentialManager>>,
}

pub trait CredentialManager: Send + Sync + 'static {
    fn refresh(&self, current: &CredentialSet) -> Option<CredentialSet>;
    fn is_expired(&self, current: &CredentialSet) -> bool;
    fn provision(&self, identity: &Identity) -> Option<CredentialSet>;
}
```

- `refresh`: OIDC token refresh, S3 session token rotation
- `is_expired`: Check TTL before use
- `provision`: Create credentials on a self-hosted service for a given identity

This is a Phase C concept. The spec defines the extension point but defers
implementation.

### Integration with OperationEnv

Handlers access credentials through `OperationEnv`:

```rust
// Handler needs outbound credentials for a service
let creds = context.env.get_credentials("rustfs");
```

This is analogous to how `context.env.invoke(namespace, op, input)` works for
operation dispatch — the handler doesn't know whether the credential comes from
config, the secret service, or a managed provider.

### Integration with SecretProtocol

Credentials are stored encrypted in the metagraph via `SecretProtocol`:

1. Operator configures credentials: `alknet credential add vast-ai --type bearer --token-file ./key.txt`
2. CLI encrypts via `SecretProtocol::Encrypt` (AES-256-GCM, key at path `m/74'/2'/0'/0'`)
3. Encrypted credential stored as `EncryptedData` node in metagraph, tagged with service name
4. At startup, `SecretStoreCredentialProvider` calls `SecretProtocol::Decrypt` for each configured service
5. Decrypted credentials held in RAM with same lifecycle as the seed (purged on `Lock`)

The `EncryptedData` wire format is shared with alknet-storage by type-level
compatibility, not a crate dependency.

### Identity-Bound Credentials (Phase B+ Future)

For multi-tenant setups where different alknet users have different access levels
on the same external service:

```rust
// Service-level credential (all users share one key):
credential_provider.get_credentials("rustfs")

// Identity-bound credential (per-user key):
credential_provider.get_credentials_for("rustfs", &identity.id)
```

The trait-level method is service-level. The identity-bound method is an
extension in alknet-storage that uses `Identity.id` (the account UUID in
database-backed deployments) as the lookup key. No separate `account_id` field
needed — `Identity.id` IS the account identifier.

## Constraints

- `CredentialProvider` and `CredentialSet` live in `alknet_core::credentials`.
  No database dependency at the core level.
- `CredentialProvider` does not depend on `IdentityProvider`. They compose
  through `OperationEnv`, not through dependency.
- `ManagedCredentialProvider` and `CredentialManager` are Phase C concepts.
  They are defined as extension points but not implemented yet.
- Identity-bound credentials use `Identity.id` as the account key. In
  config-backed deployments, this is the fingerprint or key prefix. In
  database-backed deployments, this is the account UUID.
- `SecretStoreCredentialProvider` depends on `SecretProtocol::Decrypt`, which
  requires the alknet-secret crate. A stub impl that reads from config is
  sufficient for Phase 2 when alknet-secret isn't available.
- The `CredentialSet` variants cover all identified credential types (Phases
  A–C). Phase D (alknet as OIDC provider) is additive.

## Phase Progression

| Phase | CredentialProvider Scope | Notes |
|-------|-------------------------|-------|
| Phase 2 (now) | Trait + `CredentialSet` in core. `SecretStoreCredentialProvider` stub reads from config. | Enables Phase 2 HTTP auth |
| Phase A | `SecretStoreCredentialProvider` backed by `SecretProtocol::Decrypt`. CLI command for credential management. | Full secret service integration |
| Phase B | `FromOpenAPI` integration. `CredentialProvider` populates `HttpServiceConfig.auth`. | Auto-registration of external services |
| Phase C | `ManagedCredentialProvider` + `CredentialManager`. S3 signing, OIDC refresh, identity-bound credentials. | Production self-hosted services |
| Phase D | Alknet as OIDC provider. Eliminates stored credentials for OIDC-compatible services. | Long-term goal |

## Open Questions

- **OQ-CP-01**: Should `CredentialProvider` support per-identity credentials
  (`get_credentials(service, identity)`)? See [open-questions.md](open-questions.md).

- **OQ-CP-02**: Where should OIDC provider operations live if alknet becomes
  an OIDC provider (Phase D)? See [open-questions.md](open-questions.md).

- **OQ-CP-03**: How do credential rotations propagate across a cluster? See
  [open-questions.md](open-questions.md).

- **OQ-CP-04**: Should `CredentialSet` include request-signing capability?
  See [open-questions.md](open-questions.md).

## Design Decisions

| ADR | Decision | Summary |
|-----|----------|---------|
| [036](decisions/036-credentialprovider-core-type.md) | CredentialProvider as core type | Outbound credentials in `alknet_core::credentials`, parallel to IdentityProvider |
| [029](decisions/029-identity-core-type.md) | Identity as core type | Inbound auth — the opposite direction |
| [032](decisions/032-event-boundary-discipline.md) | Event boundary | Secret service domain events stay internal |

## References

- [identity.md](identity.md) — IdentityProvider (inbound auth, opposite direction)
- [secret-service.md](secret-service.md) — SecretProtocol, EncryptedData
- [services.md](services.md) — OperationEnv, OperationContext
- [definitions.md](definitions.md) — IdentityProvider vs CredentialProvider disambiguation
- [research/phase2/credential-provider.md](../research/phase2/credential-provider.md) — Full analysis with rustfs/gitea integration