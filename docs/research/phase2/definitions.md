# Definitions: Terminology Disambiguation and Concept Mapping

> Status: Research / Draft
> Last updated: 2026-06-08
> Part of: Phase 2 planning

## Purpose

Multiple terms are overloaded across alknet's architecture, OpenStack's identity model, and the distributed systems/git space. This document disambiguates each term, maps equivalent concepts across domains, and identifies open questions that need resolution before updating architecture specs.

The architecture docs (interface.md, auth.md, services.md) reflect a pre-Phase-0/1 state. This document exists to untangle conceptual knots before editing those specs.

---

## Term Definitions

### Interface (alknet Layer 2)

**Definition**: An interface consumes a byte stream from a Transport (Layer 1) and produces call protocol sessions or handles discrete requests. It is a _protocol parser_, not a network service.

**Subtypes**:

| Subtype | Trait | Lifecycle | Transport ownership | Examples |
|---|---|---|---|---|
| `StreamInterface` | `StreamInterface::accept(stream) -> Session` | Long-lived session | Provided by caller | SshInterface, RawFramingInterface |
| `MessageInterface` | `MessageInterface::handle_request(req) -> Response` | Stateless per-request | Self-managed | HttpInterface, DnsInterface, WebSocketInterface |

**Not to be confused with**: A "service interface" (API surface of a service), a Rust trait (also called an interface generically), or an "interface" in the OpenStack sense (a network endpoint).

**Source**: [interface-model.md](interface-model.md)

---

### Transport (alknet Layer 1)

**Definition**: A transport produces a byte stream (`AsyncRead + AsyncWrite + Unpin + Send`) or a datagram channel. It is a _wire mechanism_, not a protocol. Transports are listed in `TransportKind`: TCP, TLS, iroh (QUIC), WebTransport.

**Not to be confused with**: The HTTP transport (which is a transport+interface combined in a `MessageInterface`), or the DNS "transport" (which was removed from `TransportKind` because DNS is a `MessageInterface`).

**Key constraint**: A connection is always a (Transport, StreamInterface) pair for stream-based connections. `MessageInterface` implementations manage their own transport internally.

**Source**: [tls-transport.md](tls-transport.md), [interface-model.md](interface-model.md)

---

### Service (irpc service)

**Definition**: An in-cluster Rust-to-Rust service defined by an irpc protocol enum. Services are dispatched by enum variant and use postcard serialization. They run within a node or cluster and are synchronous request-response.

**Examples**: `AuthProtocol`, `SecretProtocol`, `ConfigProtocol`, `StorageProtocol`.

**Not to be confused with**: A call protocol operation (path-based, JSON, cross-node), an external service (a third-party endpoint reachable via HTTP/call protocol), or an application service (DockerService, GitService — an operation-registered handler).

**Architecture position**: irpc services are _one dispatch backend_ for OperationEnv, not a replacement for it.

**Source**: [integration-plan.md](../integration-plan.md), Inconsistencies section item 3.

---

### Operation (call protocol)

**Definition**: A path-based handler registered in the `OperationRegistry`, dispatched by namespace + name (e.g., `/head/auth/verify`). Operations are cross-node, cross-language, and use JSON `EventEnvelope` frames.

**Not to be confused with**: An irpc service method (which is dispatched by enum variant, not path), or an OpenStack operation (which is a REST API verb).

**Architecture position**: Operations are the universal composition unit. All interfaces (SSH, HTTP, DNS, WebSocket, MCP) resolve to the same operation invocations through `OperationEnv`.

**Source**: [integration-plan.md](../integration-plan.md), ADR-033.

---

### External Service

**Definition**: Any endpoint reachable via the call protocol from another node or over an interface — an HTTP API (vast.ai), another alknet head node, rustfs, gitea. External services are _consumed_ by alknet, not part of it.

**Examples**: vast.ai cloud API, runpod API, any OpenAPI-described endpoint consumed by `OpenAPIServiceRegistry`.

**Not to be confused with**: An irpc service (internal), or an application service (handler within alknet).

---

### Application Service

**Definition**: A handler registered with the `OperationRegistry` that provides application-level functionality. Application services are pluggable, don't change core, and register operations like any other handler.

**Examples**: DockerService, NodeService, GitService, RustfsService.

**Not to be confused with**: An irpc service (which is a dispatch mechanism, not a handler), or an external service (outside the cluster).

---

### Identity (alknet core type)

**Definition**: A struct `{ id, scopes, resources }` that represents an authenticated principal. Produced by `IdentityProvider::resolve_from_fingerprint()` or `IdentityProvider::resolve_from_token()`. The same person connecting via SSH key or API token resolves to the same `Identity` (same `id` in database-backed deployments).

**Mapping to other domains**:

| alknet Concept | OpenStack Keystone | Distributed Git |
|---|---|---|
| `Identity.id` (fingerprint or UUID) | User ID | Radicle DID / on-chain address |
| `Identity.scopes` | Role assignments on a project/domain | Repository ACL entries |
| `Identity.resources` | Service catalog endpoints | Repositories accessible |
| `IdentityProvider` | Keystone identity service | On-chain registry + local cache |

**Not to be confused with**: A "user" (which is an account concept in storage), a "principal" (similar but not identical — an Identity can represent a service account or API key).

**Source**: [identity.md](../../architecture/identity.md), [auth.md](../../architecture/auth.md), ADR-029.

---

### IdentityProvider (alknet trait)

**Definition**: A trait in `alknet_core::auth` with two methods: `resolve_from_fingerprint()` (SSH key auth) and `resolve_from_token()` (bearer token auth). It resolves an inbound credential to an `Identity`.

**Implementations**: `ConfigIdentityProvider` (ArcSwap-backed, minimal), `StorageIdentityProvider` (SQLite-backed, production). Future possibility: `OnChainIdentityProvider` (smart contract + local cache).

**Direction**: Inbound (who is calling alknet).

**Not to be confused with**: `CredentialProvider` (outbound — how alknet authenticates TO external services), or an OpenStack Keystone "identity provider" which is a federation concept.

---

### CredentialProvider (alknet trait)

**Definition**: A trait in `alknet_core::credentials` that resolves outbound credentials. `get_credentials(service) -> Option<CredentialSet>`. It answers: "how does alknet authenticate to service X?"

**Direction**: Outbound (how alknet calls external services).

**Mapping**: Rustfs credentials (S3AccessKey), gitea tokens (Bearer), OIDC tokens (OidcToken), API keys (ApiKey).

**Not to be confused with**: `IdentityProvider` (inbound auth resolution).

**Source**: [credential-provider.md](credential-provider.md)

---

### AuthToken (alknet wire format)

**Definition**: `base64url(key_id || timestamp || signature)` — an Ed25519-signed timestamp token used for non-SSH auth (HTTP, DNS, WebTransport, WebSocket).

**Mapping to other domains**:

| alknet | OpenStack Keystone | Description |
|---|---|---|
| AuthToken | Keystone token (X-Auth-Token) | Proof of identity carried in a request |
| AuthToken (Ed25519 signed) | Keystone token (scoped, with catalog) | Keystone tokens carry more metadata (catalog, scope); alknet tokens are minimal |
| API key (`alk_...`) | Application Credential | Password-less auth with restricted scope |
| `resolve_from_token()` | Token validation endpoint | Verify token → resolve identity |

**Key difference**: Keystone tokens are server-issued and carry scope/catalog. alknet AuthTokens are self-signed (client-generated) and carry only key_id + timestamp — scope is resolved server-side by `IdentityProvider`. This is intentional: alknet doesn't need a token issuance endpoint because tokens are self-proving.

**Source**: [auth.md](../../architecture/auth.md), ADR-023.

---

### Domain Event vs Integration Event

**Definition** (from event-sourcing/event_source_types.md):

| Type | Scope | Consumers | Serialization | Example |
|---|---|---|---|---|
| Domain Event | Within a single service boundary | Internal handlers only | Can be rich, domain-specific | `InventoryAdjusted`, `KeyRotated` |
| Integration Event | Across service boundaries | External services, other nodes | Simple, versioned, stripped of internals | `call.requested` (EventEnvelope), `UserCreated` (projected) |

**alknet mapping**:

| Boundary | Mechanism | Serialization | Scope |
|---|---|---|---|
| Within a service (e.g., AuthProtocol) | Honker streams (domain events) | Internal | Same service |
| Between services in a cluster | irpc protocol enum | postcard (binary) | Same cluster |
| Between nodes or over interfaces | Call protocol EventEnvelope | JSON | Cross-node |

**Hard constraint** (ADR-032): Domain events never cross service boundaries without projection. Integration events are the boundary contract.

**Not to be confused with**: A "call protocol event" (which IS an integration event), or a "service call" (which is synchronous, not event-based).

---

### Scope (alknet)

**Definition**: A permission or claim attached to an `Identity`. Used by `ForwardingPolicy` and operation-level ACL. Defined as part of the `Identity` struct.

**Mapping to other domains**:

| alknet `Scope` | OpenStack Keystone | Distributed Git |
|---|---|---|
| `scopes: ["relay:connect", "secrets:derive"]` | Role assignments on a project ("member", "admin") | Write/push access to repository X |
| `resources: [...]` | Project/domain scope targets | Which repositories are accessible |

**Open question**: Should alknet adopt a richer scope model (hierarchical, like Keystone's implied roles), or keep the flat string model? See OQ-DEF-03.

---

### OperationRegistry (alknet)

**Definition**: The central registry that maps `(namespace, operation_name)` to handlers. All interfaces resolve to the same registry. The HTTP interface maps `POST /v1/{namespace}/{op}` to `registry.invoke()`. The call protocol maps `call.requested` with `operationId` to `registry.invoke()`.

**Mapping to other domains**:

| alknet | OpenStack Keystone | Description |
|---|---|---|
| OperationRegistry | Service Catalog | Both map names to endpoints; registry is programmatic, catalog is runtime-discovered |
| `FromOpenAPI` | — | Consumes an external API spec and registers operations |
| `GET /v1/schema` (proposed) | `GET /v3/auth/catalog` | Produces a spec of available operations |

**Key difference**: Keystone's catalog is per-token (scoped to the user's project). alknet's OperationRegistry is global — scope checking happens at invocation time, not discovery time.

---

### Call Protocol (alknet Layer 3)

**Definition**: The application-level protocol that carries operations, events, and responses between nodes. Uses JSON `EventEnvelope` frames. Interface-agnostic: runs over any (Transport, StreamInterface) pair or any `MessageInterface`.

**Not to be confused with**: irpc service calls (synchronous, in-cluster, postcard), or HTTP (which is an interface that maps to call protocol operations).

---

## Concept Mapping Table

### Alknet ↔ OpenStack Keystone

| Alknet Concept | Keystone Equivalent | Notes |
|---|---|---|
| `Identity` | User + Role Assignment + Project scope | alknet is simpler; Keystone separates user/role/project |
| `Identity.id` | User ID | In storage-backed: UUID. In config-backed: key fingerprint |
| `Identity.scopes` | Role assignments | alknet uses flat strings; Keystone uses hierarchical roles |
| `Identity.resources` | Project scope + Service Catalog | Both limit what a token can access |
| `IdentityProvider` | Keystone identity service | Both resolve credentials → identity + scope |
| `AuthToken` | Keystone token (X-Auth-Token) | alknet tokens are self-signed (no issuance endpoint); Keystone tokens are server-issued |
| API key (`alk_...`) | Application Credential | Nearly identical pattern |
| `CredentialProvider` | — (no direct equivalent) | Keystone doesn't authenticate outbound; each service manages its own credentials |
| `OperationRegistry` | Service Catalog | Registry is programmatic; catalog is runtime-discovered per-token scope |
| `CredentialSet::S3AccessKey` | S3 credential (access key + secret) | Directly maps to rustfs IAM model |
| `CredentialSet::OidcToken` | Federated token | alknet Phase D: becomes OIDC provider |
| Domain events (Honker) | — | Internal event bus, no Keystone equivalent |
| Integration events (call protocol) | Keystone notifications | Both are cross-boundary, but call protocol is request/response, not pub/sub |
| Token scoping | Unscoped → scoped token flow | alknet resolves scope server-side; Keystone requires explicit scope request |

### Alknet ↔ Distributed Git / Smart Contracts

| Alknet Concept | Distributed Git Equivalent | Notes |
|---|---|---|
| `Identity.id` (Ed25519 fingerprint) | Radicle DID (Ed25519 pubkey hash) | Both use Ed25519; alknet uses SLIP-0010 derivation |
| `Identity.scopes` | Repository ACL entries | Smart contract: NFT ownership → write permission |
| `IdentityProvider` | On-chain identity registry | alknet: local/DB lookup. Distributed: on-chain verification + local cache |
| `CredentialSet` | Git push credentials | ssh-key for SSH git, token for HTTPS git |
| Call protocol (integration events) | Gossip protocol (Radicle) | Both are cross-node; call protocol is point-to-point, gossip is epidemic |
| `OperationRegistry` | Replicator registry (on-chain) | Both map names to endpoints/operations |
| Domain events (Honker) | Git ref updates (internal) | Internal to the git service boundary |
| Seed derivation (BIP39) | Ethereum private key | Both derive multiple keys from one seed; different curves (Ed25519 vs secp256k1) |
| SecretProtocol key paths | — | alknet's `m/74'/0'/0'/0'` for Ed25519 identity; `m/44'/60'/0'/0/0` for Ethereum signing |

### Alknet ↔ Rustfs Auth Integration

| Alknet Concept | Rustfs Equivalent | Integration Path |
|---|---|---|
| `IdentityProvider` (inbound) | Rustfs IAM / Keystone auth | Phase D: alknet as OIDC provider → rustfs accepts alknet tokens |
| `CredentialSet::S3AccessKey` | Rustfs access key + secret key | Phase A: static credentials; Phase C: per-identity provisioned keys |
| `CredentialProvider` (outbound) | Rustfs admin API (key provisioning) | Phase C: `ManagedCredentialProvider` provisions rustfs keys |
| `Identity.scopes` | Rustfs IAM policy | Phase D: scope → OIDC claim → policy mapping |
| HTTP MessageInterface | Rustfs S3 API (port 9000) | Rustfs sits behind alknet's HTTP router or sidecar |
| OperationRegistry | — | Git service maps `git.clone`, `git.push`, etc. to operations |

---

## Overloaded Terms: Disambiguation

### "Service" — Three Meanings

| Context | Meaning | Example | Architecture Layer |
|---|---|---|---|
| alknet irpc service | In-cluster Rust-to-Rust protocol enum | `AuthProtocol`, `SecretProtocol` | Layer 3 (internal) |
| alknet application service | Operation-registered handler | `GitService`, `RustfsService` | Layer 3 (handler) |
| External service | Third-party endpoint consumed by alknet | `vast.ai`, `rustfs` instance | Outside alknet (consumed via OperationEnv) |

**Rule**: When ambiguity is possible, use the full qualifier: "irpc service", "application service", or "external service". The bare word "service" should be avoided in architecture docs.

### "Interface" — Three Meanings

| Context | Meaning | Example |
|---|---|---|
| alknet Layer 2 | A protocol parser that consumes Transport streams or handles discrete requests | `SshInterface`, `HttpInterface`, `DnsInterface` |
| Rust/generic | A trait definition | `IdentityProvider`, `CredentialProvider` |
| OpenStack/generic | A network endpoint (URL) for a service | Keystone's public/internal/admin interfaces |

**Rule**: In alknet architecture docs, "Interface" (capitalized) refers to Layer 2. "trait" or "contract" should be used for Rust trait definitions. "endpoint" should be used for network URLs.

### "Token" — Three Meanings

| Context | Meaning | Structure |
|---|---|---|
| AuthToken (alknet) | Self-signed Ed25519 timestamp | `base64url(key_id \|\| timestamp \|\| sig)` |
| API key (alknet) | Hash-verified bearer string | `alk_...` prefix, SHA-256 hash verification |
| Keystone token | Server-issued scoped token | UUID or JWT, carries catalog and scope |

**Rule**: "AuthToken" refers to alknet's self-signed token. "API key" refers to the hash-verified bearer format. "Keystone token" when referring to OpenStack. Never use bare "token" in architecture docs.

### "Identity" — Two Meanings

| Context | Meaning |
|---|---|
| alknet `Identity` struct | `{ id, scopes, resources }` — the authenticated principal |
| OpenStack Identity (Keystone) | The entire identity management SERVICE, including users, projects, roles, tokens, catalog |

**Rule**: "Identity" (capitalized, code font) = alknet struct. "Keystone" or "identity service" = OpenStack concept.

### "Domain" — Two Meanings in Event Sourcing

| Context | Meaning |
|---|---|
| Domain Event | An event within a single service boundary (e.g., `KeyRotated` within AuthProtocol) |
| DNS Domain | A domain name in DNS queries/records |

These are unrelated. "Domain event" is from DDD. "DNS domain" is from networking. Context should always make it clear, but if there's any chance of confusion, use "bounded-context event" instead of "domain event".

---

## Architectural Patterns: Cross-Domain Comparison

### Pattern: Inbound Auth → Outbound Credentials

```
┌──────────────────────────────────────────────────────────────┐
│  Incoming Request                                            │
│     │                                                        │
│     ▼                                                        │
│  IdentityProvider                                            │
│  (credential → Identity)                                     │
│     │                                                        │
│     ├── SSH fingerprint → Identity.id, .scopes, .resources   │
│     ├── Bearer AuthToken  → Identity.id, .scopes, .resources │
│     └── API key           → Identity.id, .scopes, .resources│
│     │                                                        │
│     ▼                                                        │
│  OperationContext { identity, env, ... }                     │
│     │                                                        │
│     ├── context.env.invoke("git", "push", input)            │
│     │      └── GitService handler                            │
│     │           └── CredentialProvider                      │
│     │                └── get_credentials("rustfs")           │
│     │                     └── S3AccessKey { access_key,     │
│     │                          secret_key }                  │
│     │                                                        │
│     └── context.env.invoke("secrets", "derive", input)      │
│            └── local dispatch to SecretProtocol              │
│                                                                │
│  Two directions: Inbound (who is calling us)                  │
│                  Outbound (how we call others)                │
└──────────────────────────────────────────────────────────────┘
```

### Pattern: Scope Resolution Across Systems

| System | Scope Source | Scope Shape | Scope Check Location |
|---|---|---|---|
| alknet (current) | `IdentityProvider` | Flat strings `["relay:connect"]` | Handler invocation |
| Keystone | Role assignment on project | Hierarchical roles with implied roles | Policy engine per service |
| Rustfs IAM | Policy document attached to user | JSON policy with actions/resources | Request evaluation |
| Smart contract ACL | NFT ownership + on-chain mapping | Address → repo → permission level | On-chain verification + local cache |
| Radicle | Local config | Pubkey → repo → permission | Pre-receive hook |

**Open question**: Should alknet adopt hierarchical implied roles (Keystone pattern) or stay with flat scopes and let individual services interpret them?

### Pattern: Token Self-Proving vs Server-Issued

| Property | alknet AuthToken | Keystone Token | API Key |
|---|---|---|---|
| Issued by | Client (self-signed) | Server (Keystone) | Admin (config or DB) |
| Carries | key_id + timestamp + signature | User ID, scope, catalog, expiry | Prefix + hash |
| Verified by | Ed25519 signature check | Server lookup (database or JWT) | SHA-256 hash check |
| Revocation | Key removal from `authorized_keys` | Token revocation list or JWT `jti` | DB deletion |
| Scope resolution | Server-side (IdentityProvider) | Embedded in token | Server-side (DB lookup) |
| Replay protection | Timestamp window (±300s) | Token TTL + server validation | N/A (stateless) |

alknet's self-proving model avoids the need for a token issuance endpoint. This is a deliberate trade-off: simpler at the cost of no server-side session state. For replay protection beyond the timestamp window, future work could add nonce challenge-response (ADR-023).

---

## Service Classification

Services within alknet's ecosystem are classified by their relationship to the core:

### Core Services (irpc, always present when feature flag enabled)

| Service | Protocol | Location | Purpose |
|---|---|---|---|
| Auth | `AuthProtocol` | alknet-core (`irpc` feature) | Identity resolution, credential verification |
| Config | `ConfigProtocol` | alknet-core (`irpc` feature) | Dynamic config reload |
| Secret | `SecretProtocol` | alknet-secret | Key derivation, encryption, decryption |
| Storage | `StorageProtocol` | alknet-storage | Metagraph CRUD, ACL, accounts |

### Application Services (operation-registered, pluggable)

| Service | Interface | Core dependency | Purpose |
|---|---|---|---|
| GitService | HTTP (MessageInterface) + SSH (StreamInterface) | IdentityProvider, CredentialProvider | Git clone/push/pull over HTTPS and SSH |
| RustfsService | HTTP (MessageInterface) | CredentialProvider | S3-compatible object storage proxy |
| DockerService | HTTP (MessageInterface) | CredentialProvider | Container management |
| NodeService | HTTP (MessageInterface) | IdentityProvider | Node management |

### External Services (consumed, not hosted)

| Service | Integration | Auth |
|---|---|---|
| vast.ai | `OpenAPIServiceRegistry` + `CredentialProvider` | API key |
| runpod | `OpenAPIServiceRegistry` + `CredentialProvider` | API key |
| ubicloud | `OpenAPIServiceRegistry` + `CredentialProvider` | API key |

**Key distinction**: Rustfs and gitea are "self-hosted external services" — they run inside the same deployment boundary but are managed independently. alknet acts as a gateway (identity provider, credential provider) and reverse proxy (HTTP interface) for them, but they are NOT part of alknet-core.

---

## Open Questions

### OQ-DEF-01: Should alknet adopt a "Service Catalog" concept like Keystone?

Keystone's service catalog lets a token carry information about which services and endpoints are available to the authenticated user. alknet's `OperationRegistry` is global — every authenticated identity sees the same operations. Should there be a scope-filtered operation discovery mechanism?

**Options**:
1. Keep `OperationRegistry` global, check scope at invocation time (current design)
2. Add `GET /v1/catalog` or `GET /v1/schema?scope=<scope>` that returns only operations the identity can invoke
3. Add a "service catalog" field to `Identity.resources` that lists available namespaces

**Recommendation**: Start with option 1 (current design). Add option 2 when multi-tenant deployment requires it. The `GET /v1/schema` endpoint (from tls-transport.md) already provides operation discovery — adding scope filtering is additive.

---

### OQ-DEF-02: Should "application service" and "irpc service" be renamed to avoid "service" overloading?

The word "service" has three meanings in the architecture (irpc, application, external). Should we adopt different terms?

**Options**:
1. Keep current names, always qualify with "irpc service", "application service", "external service"
2. Rename: "irpc service" → "irpc protocol" or "backend handler"; "application service" → "adapter" or "integration"
3. Adopt the call-protocol terminology exclusively: everything that registers in `OperationRegistry` is an "operation handler", and "service" refers only to external endpoints

**Recommendation**: Option 1 for now. The qualifiers are sufficient, and renaming would require changing ADRs and multiple specs. Revisit if confusion persists in practice.

---

### OQ-DEF-03: Should `Identity.scopes` be hierarchical (like Keystone implied roles) or stay flat?

Current design: `scopes: Vec<String>` with flat strings like `"relay:connect"`, `"secrets:derive"`.
Keystone pattern: Roles can imply other roles (admin implies member). Policies are per-service, not global strings.

**Options**:
1. Keep flat scopes, let individual services interpret them (current)
2. Add implied scope resolution: `"admin"` → `["relay:connect", "secrets:derive", ...]`
3. Adopt a policy language (JSON policy documents like Rustfs IAM)

**Recommendation**: Start with option 1. Add implied scope resolution in alknet-storage when multi-tenant deployment requires it. A full policy language is Phase D territory and should follow what Rustfs already uses (MinIO-style JSON policies) rather than inventing something new.

---

### OQ-DEF-04: How should the GitService adapter work across HTTP and SSH?

gitserver provides `gitserver-core` (transport-agnostic git protocol logic) and `gitserver-http` (Axum HTTP layer). alknet's architecture supports two paths:

**Path A — HTTP MessageInterface**: Git operations over HTTPS, with alknet's HTTP interface authenticating the request and passing Identity to the GitService handler. The GitService handler uses `Identity` to determine repo access and calls `gitserver-core` directly.

**Path B — SSH StreamInterface**: Git operations over SSH, where the SSH interface already authenticates the user. Git commands are dispatched through SSH channels (similar to how `channel_open_direct_tcpip` works for port forwarding, but with a `git-upload-pack` / `git-receive-pack` channel type).

**Path C — Both**: `gitserver-core` as the protocol engine, `gitserver-http` for HTTPS, and a custom `SshGitInterface` for SSH-git channels.

**Recommendation**: Phase 1 — Path A (HTTP only). Phase 2 — Path C (both). The git-smart-HTTP-protocol is well-understood, and `gitserver-http` can be nested into alknet's Axum router. SSH git requires designing a new channel type in `SshInterface`.

---

### OQ-DEF-05: Should alknet act as an OIDC provider (Phase D of credential-provider.md)?

This is the most cross-cutting question. If alknet becomes an OIDC provider, it becomes the identity backbone for all self-hosted services (rustfs, gitea, etc.). This maps to OpenStack Keystone's role but with a different scope model.

**Benefits**:
- Eliminates stored credential management for OIDC-compatible services
- Users authenticate once via alknet (SSH key or token) and get scoped access to all services
- Maps directly to `Identity.scopes → OIDC claims → service policies`

**Complexity**:
- Requires OIDC authorization server endpoints (authorize, token, userinfo, jwks)
- Requires PKCE flow for browser-based auth
- Requires claim → policy mapping per service
- alknet is not currently designed to be an OIDC server

**Recommendation**: Phase D (long-term). Phases A-C use static credentials and managed credentials, which are sufficient for most deployments. OIDC provider is a quality-of-life improvement that becomes important in multi-user self-hosted setups.

---

### OQ-DEF-06: How does Domain Event vs Integration Event discipline apply to self-hosted services?

ADR-032 says domain events stay within the service boundary and integration events cross it. Rustfs and gitea are outside alknet's boundary but inside the deployment boundary. Where do their events fall?

**Options**:
1. Self-hosted services are external: their events are integration events, consumed via callback/webhook
2. Self-hosted services are part of alknet's boundary: use Honker streams internally, project to integration events for cross-node
3. Hybrid: alknet projects rustfs/gitea state changes into `EventEnvelope` integration events, but rustfs/gitea internal events stay in their own boundary

**Recommendation**: Option 3. Self-hosted services have their own internal event systems. alknet projects state changes (bucket created, repo pushed) into `EventEnvelope` integration events for cross-node communication. Honker streams are for events within alknet-core services only.

---

### OQ-DEF-07: How should the smart-contract / on-chain identity model relate to alknet's IdentityProvider?

The distributed git concept (NFT-based org/repo tokens) introduces a third `IdentityProvider` implementation that validates identity on-chain. How does this relate to the existing two implementations?

**Option 1 — OnChainIdentityProvider**: A new implementation of the `IdentityProvider` trait that checks on-chain ownership. Slow path: on-chain verification (0.5-5s on L2). Fast path: local ACL metagraph cache validated against on-chain state periodically.

**Option 2 — Separate verification layer**: On-chain verification is a separate step, not an IdentityProvider. After normal auth (SSH key or token), a second check verifies on-chain ownership for specific operations (e.g., write to a distributed repo).

**Option 3 — CredentialProvider extension**: On-chain verification is outbound — alknet authenticates TO the smart contract to verify repo permissions. This would be a new `CredentialSet` variant.

**Recommendation**: Option 1 for the long term. The `IdentityProvider` trait is designed to be pluggable. An `OnChainIdentityProvider` with local cache is additive. It resolves on-chain identity to an `Identity` struct just like `ConfigIdentityProvider` and `StorageIdentityProvider`. The seed derivation path `m/44'/60'/0'/0/0` (Ethereum) alongside `m/74'/0'/0'/0'` (Ed25519 identity) provides a cryptographic link between the two key types.

---

### OQ-DEF-08: Should the "interface" concept in auth.md (which distinguishes auth "presentation" per transport/interface pair) be renamed to avoid confusion with Layer 2 "Interface"?

In auth.md, "auth presentation" is the mechanism by which credentials are presented on each interface:
- SSH: key handshake
- HTTP: Bearer header
- DNS: token in query labels
- WebTransport: token in CONNECT request

This is NOT the same as "Interface" (Layer 2), but uses the same word. Should we adopt a distinct term?

**Options**:
1. Keep "auth presentation" — it's already distinct from "Interface" (Layer 2)
2. Rename to "auth mechanism" or "credential presentation" to be more precise
3. Use the term from the interface-model.md table: "(Transport, Interface) → Auth mechanism"

**Recommendation**: Option 2. "Credential presentation" is precise and doesn't overload "interface". Update auth.md to use "credential presentation per (Transport, Interface) pair" consistently.

---

## References

- [interface-model.md](interface-model.md) — StreamInterface / MessageInterface trait design
- [credential-provider.md](credential-provider.md) — CredentialProvider, CredentialSet (outbound auth)
- [tls-transport.md](tls-transport.md) — Unified multi-interface architecture
- [integration-plan.md](../integration-plan.md) — Phase structure, OperationEnv, event boundary discipline
- [identity.md](../../architecture/identity.md) — Identity struct, IdentityProvider trait
- [auth.md](../../architecture/auth.md) — Unified auth, AuthToken format
- [services.md](../../architecture/services.md) — irpc services, OperationEnv
- [event-source-types.md](../event-sourcing/event_source_types.md) — Domain events vs integration events
- [ADR-032](../../architecture/decisions/032-event-boundary-discipline.md) — Event boundary rule
- [ADR-033](../../architecture/decisions/033-operationenv-irpc-call-protocol.md) — OperationEnv, three dispatch paths
- [references/rustfs/](../references/rustfs/) — Rustfs research and reference
- [references/gitserver/](../references/gitserver/) — Gitserver research and reference
- [references/openstack-keystone/](../references/openstack-keystone/) — OpenStack Keystone concepts
- [references/distributed-identity/](../references/distributed-identity/) — Distributed identity and smart contract ACL