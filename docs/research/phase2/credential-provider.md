# Credential Provider: Outbound Service Authentication

> Status: Research / Draft
> Last updated: 2026-06-08
> Part of: Phase 2 planning

## Overview

Alknet's `IdentityProvider` resolves **inbound** authentication: who is making a request _to_ alknet. The `CredentialProvider` resolves **outbound** authentication: how alknet authenticates _to_ external and self-hosted services. This is a distinct and currently unaddressed concern that affects nearly every application service — from cloud API integrations (vast.ai, runpod, ubicloud) to self-hosted infrastructure (rustfs, gitea, postgres).

## Problem Statement

### External API credentials

Cloud providers use simple auth patterns — API keys, bearer tokens, basic auth. The existing `SecretProtocol` (encrypt/decrypt via derived AES-256-GCM keys, defined in [secret-service.md](../../architecture/secret-service.md)) can store and retrieve these at rest. But the wiring between "decrypt a credential from storage" and "use it in an HTTP request" doesn't exist yet. Each service wrapper currently would have to independently solve credential retrieval, caching, and lifecycle.

### Self-hosted service auth

Self-hosted services use more complex auth mechanisms that go beyond static tokens:

- **rustfs** uses S3-style access key + secret key pairs with AWS Signature V4 request signing. They also support OIDC (OpenID Connect with PKCE). The access key/secret key aren't a bearer header — they're inputs to a per-request HMAC-SHA256 signature computation.
- **gitea** supports OAuth2, OIDC, and reverse proxy authentication (SSO via headers). Its internal user/token system is separate from alknet's identity model.
- Other self-hosted services (postgres, redis) may use their own auth schemes.

These services are **inside the operational domain** — their credential lifecycle (provisioning, rotation, revocation, token refresh) is part of running the stack, not a one-time configuration step.

### The gap

Currently:

```
User → alknet → IdentityProvider (resolves who the user is)  ✅ exists
alknet → external service → ??? (resolves how alknet authenticates)  ❌ missing
```

Without `CredentialProvider`, each service wrapper would:
1. Independently retrieve and decrypt credentials from the secret service
2. Independently implement auth mechanism specifics (bearer, S3 signing, OIDC refresh)
3. Have no shared infrastructure for credential lifecycle management

This leads to duplicated effort and inconsistent security practices across service wrappers.

## Design

### CredentialProvider Trait

```rust
pub trait CredentialProvider: Send + Sync + 'static {
    fn get_credentials(&self, service: &str) -> Option<CredentialSet>;
    fn refresh_credentials(&self, service: &str) -> Option<CredentialSet>;
}
```

This is intentionally narrow. It returns credentials for a named service. It does not try to abstract the auth mechanism itself — that stays with the service wrapper that knows the protocol.

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

Each variant carries the data needed for a specific auth mechanism. The service wrapper that requested the credentials knows what variant it expects and how to use it — the `OpenAPIServiceRegistry` knows it needs a `Bearer` or `ApiKey`, the rustfs S3 wrapper knows it needs `S3AccessKey` for request signing.

### CredentialProvider vs IdentityProvider

These are opposite-direction abstractions:

| | IdentityProvider | CredentialProvider |
|---|---|---|
| Direction | Inbound (who is calling alknet) | Outbound (how alknet calls others) |
| Resolves | Fingerprint/token → Identity | Service name → CredentialSet |
| Storage | `peer_credentials`, `api_keys` tables (alknet-storage) | Encrypted nodes in metagraph (via SecretProtocol) |
| Lifecycle | Stateless lookup | May need refresh (OIDC tokens, S3 sessions) |
| Location | `alknet_core::auth` | `alknet_core::credentials` |

Both live at the same architectural layer. A service handler receives an `OperationContext` with `identity` (who called us) and access to credentials through `context.env`. The handler doesn't interact with `CredentialProvider` directly in the common case — the service initialization code does, when setting up the HTTP client or SDK wrapper.

### Accounts: Storage-Layer Concern, Not Core

The `Identity` struct in core (`{ id, scopes, resources }`) does not need an explicit `account_id` field. In config-based auth (`ConfigIdentityProvider`), `id` is the SSH key fingerprint. In database-backed auth (`StorageIdentityProvider`), `id` is the account UUID. The account concept is an implementation detail of `StorageIdentityProvider` — it resolves `peer_credentials.fingerprint → account_id → Identity { id: account_uuid, ... }`. The same person authenticating via SSH key or bearer token gets the same `Identity { id: account_uuid, ... }` because both credential presentations map to the same account UUID in storage.

This means identity-bound credential lookups (e.g., "Alice's rustfs access key") use `Identity.id` (which is the account UUID in database-backed deployments) as the key — not a separate field. The call pattern is:

```rust
// Service-level credential (no identity needed):
credential_provider.get_credentials("rustfs")  // shared admin key

// Identity-bound credential (uses id as account identifier):
credential_provider.get_credentials_for("rustfs", &identity.id)  // per-user key
```

The `CredentialProvider` trait at core only needs the service-level method. Identity-bound lookups are an extension in alknet-storage that uses the same `Identity.id`.

### Interaction with SecretProtocol

Credentials are stored encrypted in the metagraph via the existing `SecretProtocol`:

1. At setup time, an operator configures credentials for a service (e.g., `alknet credential add vast-ai --type bearer --token-file ./key.txt`)
2. The CLI encrypts the credential via `SecretProtocol::Encrypt` (using the derived encryption key at `m/74'/2'/0'/0'`)
3. The encrypted credential is stored as an `EncryptedData` node in the metagraph, tagged with the service name
4. At startup, `SecretStoreCredentialProvider` (the default `CredentialProvider` impl) calls `SecretProtocol::Decrypt` for each configured service
5. The decrypted credentials are held in RAM with the same lifecycle as the secret service (purged on `Lock`)

```rust
pub struct SecretStoreCredentialProvider {
    credentials: ArcSwap<HashMap<String, CredentialSet>>,
    secret_client: Client<SecretProtocol>,
}

impl CredentialProvider for SecretStoreCredentialProvider {
    fn get_credentials(&self, service: &str) -> Option<CredentialSet> {
        let cache = self.credentials.load();
        cache.get(service).cloned()
    }

    fn refresh_credentials(&self, service: &str) -> Option<CredentialSet> {
        // Re-decrypt from storage — used after Lock/Unlock cycle
        // Calls secret_client.decrypt() and updates cache
        None // simplified
    }
}
```

### Interaction with OpenAPIServiceRegistry

The TypeScript `@alkdev/operations` `from_openapi.ts` defines `HTTPServiceConfig.auth`:

```typescript
auth?: {
    type: "bearer" | "apiKey" | "basic";
    token?: string;
    headerName?: string;
    prefix?: string;
};
```

The Rust port would populate this from `CredentialProvider`:

```rust
let creds = credential_provider.get_credentials("vast-ai");
let auth = match creds {
    Some(CredentialSet::Bearer { token }) => AuthConfig::Bearer { token },
    Some(CredentialSet::ApiKey { header_name, token }) => AuthConfig::ApiKey { header_name, token },
    Some(CredentialSet::Basic { username, password }) => AuthConfig::Basic { username, password },
    _ => None,
};
let config = HttpServiceConfig {
    namespace: "vast-ai",
    base_url: "https://cloud.vast.ai/api/v1",
    auth,
    ..
};
let ops = FromOpenAPI(spec, config);
registry.register_all(ops);
```

### Self-Hosted Services: ManagedCredentialProvider

For self-hosted services, credentials may need active lifecycle management:

**rustfs (S3)**:
- Access key + secret key are created inside rustfs IAM
- The alknet rustfs service wrapper holds the `S3AccessKey` credential set
- Each S3 request is signed using AWS Signature V4 (computed from access_key + secret_key + request details)
- Session tokens from STS-style calls have a TTL and need rotation
- Provisioning: alknet could create the rustfs access key via the rustfs admin API at first setup, then store the resulting credentials

**rustfs (OIDC)**:
- rustfs supports OIDC providers — alknet's identity system _could_ act as an OIDC provider
- This would allow alknet identities to authenticate directly to rustfs without stored credentials
- Requires: alknet running an OIDC authorization server endpoint (potentially exposed via the call protocol)

**gitea (OAuth2/OIDC)**:
- Similar to rustfs OIDC — alknet could act as the OAuth2/OIDC provider
- Gitea supports reverse proxy auth (SSO via headers) — if alknet sits in front as a reverse proxy, it can inject auth headers
- Gitea also has its own API token system — simpler case, just store the token

**ManagedCredentialProvider** wraps these cases:

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

- `refresh`: For OIDC token refresh, S3 session token rotation
- `is_expired`: Check TTL on tokens before use
- `provision`: Create credentials on a self-hosted service for a given alknet identity (e.g., create a rustfs access key for a new user)

### Identity-Bound Credentials

For self-hosted services where alknet manages the user accounts, there's a higher-order pattern:

1. An alknet `Identity` (resolved by `IdentityProvider`) needs access to a self-hosted service
2. `ManagedCredentialProvider::provision(identity)` creates the corresponding account on the external service
3. The resulting credentials are stored and associated with the alknet identity in the metagraph
4. When the identity makes a call through the operation registry, the handler can resolve their service-specific credentials using `Identity.id` as the account key

This bridges `IdentityProvider` and `CredentialProvider`:

```
IdentityProvider: who is this user? → Identity
CredentialProvider: how do we talk to service X? → CredentialSet
Identity-bound: how does THIS user talk to service X? → CredentialSet (scoped to Identity.id)
```

The identity-bound case is important for multi-tenant self-hosted setups where different alknet users should have different access levels on rustfs or gitea. It can be deferred initially — Phase A only needs service-level credentials.

## Architectural Position

### Where CredentialProvider lives

`CredentialProvider` and `CredentialSet` are core types, analogous to `IdentityProvider` and `Identity`. They live in `alknet_core::credentials`.

Like `IdentityProvider`:
- The trait is in alknet-core
- The default impl (`SecretStoreCredentialProvider`) uses the secret service + metagraph
- Production impls (`ManagedCredentialProvider`) may live in alknet-storage or application crates
- The CLI/NAPI assembly wires the concrete impl
- Core does not depend on any storage system

### Dependencies

```
alknet-core (CredentialProvider trait, CredentialSet enum)
    ↑
alknet-secret (SecretStoreCredentialProvider reads from SecretProtocol::Decrypt)
    ↑
Application crates (rustfs wrapper, gitea wrapper, etc.)
```

`CredentialProvider` does not depend on `IdentityProvider`, but `ManagedCredentialProvider` may use `Identity.id` to resolve identity-bound credentials.

### Relationship to existing specs

| Existing concept | Relationship |
|---|---|
| `IdentityProvider` | Opposite direction. Identity is inbound auth. Credential is outbound auth. |
| `SecretProtocol` | Stores and retrieves encrypted credentials. `SecretStoreCredentialProvider` is a consumer of `SecretProtocol::Decrypt`. |
| `OperationEnv` | Service init code uses `CredentialProvider` to configure `HTTPServiceConfig.auth`. Handlers call operations through `env`. |
| `OpenAPIServiceRegistry` | Consumer of `CredentialProvider` — populates `auth` config from credential lookup. |
| `EncryptedData` | Wire format for stored credentials. Compatible with existing `EncryptedDataSchema` from `@alkdev/storage`. |
| `Identity.id` | In database-backed deployments, serves as the account UUID for identity-bound credential lookups. No separate `account_id` field needed — `id` IS the account identifier. |

### Account management is storage-layer, not core

The `AccountService` irpc protocol (CRUD for accounts and credential associations) lives in alknet-storage, not core. This follows the same pattern as `ConfigService`:
- Core has the read trait (`IdentityProvider`, `CredentialProvider`)
- Storage has the management service (`AccountProtocol`, `CredentialProtocol`)
- The CLI/NAPI assembly wires them together

The storage model for accounts:

```
accounts
  ├── id (UUID, primary key)
  ├── display_name
  ├── status (active, disabled)
  └── default_scopes (JSON)

peer_credentials  (inbound — SSH keys)
  ├── account_id → accounts.id
  ├── fingerprint (SHA-256 of public key)
  ├── public_key_data
  └── scopes_override (JSON, null = use account default)

api_keys  (inbound — bearer tokens)
  ├── account_id → accounts.id
  ├── key_prefix (first 8 chars, for lookup)
  ├── key_hash (SHA-256 of full key)
  ├── scopes (JSON)
  └── expires_at

service_credentials  (outbound — for external services)
  ├── id (UUID)
  ├── account_id → accounts.id  (NULL = shared/service-level)
  ├── service_name
  ├── credential_type
  ├── encrypted_data → EncryptedData
  ├── metadata (JSON)
  └── expires_at
```

`StorageIdentityProvider` queries `peer_credentials` → `accounts` and `api_keys` → `accounts` to resolve any inbound credential to the same `Identity { id: account_uuid, ... }`. `StorageCredentialProvider` queries `service_credentials` and decrypts via `SecretProtocol` to resolve outbound credentials.

## Implementation Phases

### Phase A: Core types and simple credential storage

Define the trait and enum in alknet-core. Implement `SecretStoreCredentialProvider` that decrypts stored credentials at startup. Wire into the service assembly (CLI). This enables static API key / bearer token patterns — sufficient for cloud API integrations.

Deliverables:
- `CredentialProvider` trait + `CredentialSet` enum in `alknet_core::credentials`
- `SecretStoreCredentialProvider` impl (reads from `SecretProtocol::Decrypt`)
- CLI command: `alknet credential add <service> --type bearer --token-file <path>`
- Credential storage in metagraph as encrypted nodes tagged by service name

Depends on: Phase 1 (OperationEnv, SecretProtocol) + alknet-secret crate existing

### Phase B: OpenAPI/JSON Schema auto-registration

Port `FromOpenAPI` and `OpenAPIServiceRegistry` from the TypeScript `@alkdev/operations` to Rust. Integrate with `CredentialProvider` for auth config. This enables any OpenAPI-described service to be auto-registered as a set of operations.

Deliverables:
- `alknet-openapi` or `alknet-operations-adapter` crate with `from_openapi` module
- `FromOpenAPI(spec, config) -> Vec<(OperationSpec, Handler)>`
- `HttpServiceConfig` with auth populated from `CredentialProvider`
- `OpenAPIServiceRegistry::register_all(registry)` port

Depends on: Phase A + existing `OperationRegistry`

### Phase C: Managed credentials and self-hosted auth

Add `ManagedCredentialProvider` with `CredentialManager` trait. Implement S3 signing for rustfs. Implement OIDC token refresh. Enable identity-bound credential provisioning.

Deliverables:
- `CredentialManager` trait
- `ManagedCredentialProvider` impl
- S3CredentialManager (request signing, session token rotation)
- OidcCredentialManager (token refresh, PKCE flow)
- Identity-bound credential resolution (uses `Identity.id` as account key)

Depends on: Phase A + alknet-storage + application-specific knowledge

### Phase D: Alknet as OIDC/OAuth2 provider

Alknet's identity system could expose an OIDC authorization server endpoint. Self-hosted services (rustfs, gitea) would be configured to use alknet as their OIDC provider. This eliminates stored credential management entirely for the OIDC path — users authenticate directly through alknet's existing identity.

This is the most complex but also the most elegant path for self-hosted services. It makes alknet the identity backbone of the entire self-hosted stack.

Deliverables:
- OIDC authorization server operations (authorize, token, userinfo, jwks)
- Exposed via call protocol and/or HTTP adapter
- Configuration for rustfs/gitea to use alknet as OIDC provider
- Identity mapping: alknet Identity scopes → rustfs/gitea policies

Depends on: Phase C + call protocol HTTP or web adapter + significant R&D

## Analysis of Self-Hosted Auth Mechanisms

### rustfs

**S3 access key/secret key**:
- rustfs IAM manages users, groups, policies, and service accounts
- Credentials are `access_key` + `secret_key` pairs (S3 standard)
- Auth uses AWS Signature V4: HMAC-SHA256 of request details using the secret key
- Session tokens (from STS AssumeRole-style flows) are JWTs with claims including policy
- Access keys are created via the rustfs admin API or UI

**OIDC**:
- Full OpenID Connect support with PKCE
- Uses the `openidconnect` Rust crate for standards compliance
- Supports discovery, token exchange, ID token verification
- OIDC users are mapped to rustfs policies via claims

**Integration path**:
- Minimal: Store access key + secret key as `CredentialSet::S3AccessKey`, use for request signing
- Better: alknet as OIDC provider → no stored credentials, direct identity mapping
- Best: Phase D path where rustfs trusts alknet as its identity provider

### gitea

**Auth options**:
- OAuth2 provider (gitea can act as OAuth2 provider for other apps)
- OIDC client (gitea can delegate auth to an external OIDC provider — alknet in Phase D)
- Reverse proxy auth (SSO via HTTP headers — alknet injects `X-WebAuth-User` as a reverse proxy)
- API tokens (personal access tokens, scoped, with TTL)
- SSH keys (for git operations, separate from API auth)

**Integration path**:
- Minimal: Store gitea API token as `CredentialSet::Bearer`
- Intermediate: If alknet runs as a reverse proxy in front of gitea, inject auth headers
- Best: alknet as OIDC provider for gitea

### General pattern

For both rustfs and gitea, the auth integration follows the same progression:

1. **Static credentials** (Phase A): Store API keys/tokens, decrypt at startup. Simple, works for single-user or admin-only access.
2. **Dynamic credentials** (Phase C): Managed credential lifecycle — token refresh, session rotation. Needed for production.
3. **Identity federation** (Phase D): Alknet acts as the identity provider. No stored service credentials. Users authenticate through alknet and their identity (scopes, resources) maps to the external service's policy model. Most secure, most complex.

Phase D is not required to start building service wrappers. Phases A and C are sufficient for functional integrations. Phase D is a quality-of-life and security improvement that becomes important in multi-user self-hosted deployments.

## Open Questions

### OQ-CP-01: Should CredentialProvider support per-identity credentials?

That is, should the trait be `get_credentials(service, identity)` instead of `get_credentials(service)`?

Pro: Enables multi-tenant self-hosted services where different alknet users have different access.
Con: More complex, and the identity resolution can be done by the service wrapper itself by looking up identity-bound credentials from the metagraph.

Recommendation: Start with service-level credentials. Add identity-level resolution as a second method (`get_credentials_for(service, account_id)`) when the need is concrete. Since `Identity.id` already serves as the account UUID in database-backed mode, there's no need for a separate `account_id` field.

### OQ-CP-02: Where should the OIDC provider operations live?

If alknet becomes an OIDC provider (Phase D), the authorization server endpoints need to live somewhere. Options:

1. In alknet-core behind a feature flag (like auth service)
2. In a new `alknet-oidc` crate
3. As an application service registered in the operation registry

Recommendation: Application service (option 3). OIDC is an application concern, not a core concern. The call protocol and `OperationRegistry` provide the transport; OIDC is just another set of operations.

### OQ-CP-03: How do credential rotations propagate across a cluster?

If a credential is rotated (e.g., S3 session token refreshed on the head node), how do worker nodes get the updated credential? Options:

1. Workers request fresh credentials on each use (always current, more secret service calls)
2. Push notification via honker stream (efficient, but adds cross-service event coupling)
3. Workers cache with TTL (simple, may briefly use stale credentials)

Recommendation: TTL-based caching with a refresh threshold. Workers call `CredentialProvider::get_credentials()` which checks `is_expired()` and calls `refresh_credentials()` if needed. The TTL is per-credential-type (e.g., 1 hour for S3 session tokens, no TTL for static API keys).

### OQ-CP-04: Should CredentialSet include request-signing capability?

For S3 auth, the credential set contains `access_key + secret_key`, but the actual HTTP request signing (AWS Signature V4) is a separate computation. Should `CredentialSet::S3AccessKey` include a signing method?

Recommendation: Keep `CredentialSet` as pure data. Add a separate `s3_sign(credential: &S3AccessKey, request: &HttpRequest) -> SignedRequest` utility function in the service wrapper or a shared `alknet-s3` utility crate. The `OpenAPIServiceRegistry` pattern already separates credentials from HTTP client behavior; signing is client behavior.

### OQ-CP-05: How does this relate to the HTTP service / AI SDK port?

The AI SDK port provides HTTP infrastructure (streaming, retries, SSE parsing, error handling). The `CredentialProvider` provides the auth config that the HTTP client consumes. They're separate concerns that compose: the HTTP service uses `CredentialProvider` to populate `auth` headers/tokens on outgoing requests, just as `OpenAPIServiceRegistry` does. The AI SDK's provider codegen (which would be replaced with the operation pattern) currently hardcodes auth per provider; `CredentialProvider` makes it dynamic and centrally managed instead.

## References

- [identity.md](../../architecture/identity.md) — IdentityProvider trait, Identity struct
- [secret-service.md](../../architecture/secret-service.md) — SecretProtocol, EncryptedData
- [services.md](../../architecture/services.md) — OperationEnv, OperationRegistry, service composition
- [call-protocol.md](../../architecture/call-protocol.md) — OperationEnv three dispatch paths
- [integration-plan.md](../integration-plan.md) — Phase structure, OperationEnv wiring
- [@alkdev/operations/src/from_openapi.ts](../../../@alkdev/operations/src/from_openapi.ts) — OpenAPIServiceRegistry, HTTPServiceConfig.auth