# Alknet Services: irpc Service Architecture

> Status: Research / Draft
> Last updated: 2026-06-06

## Overview

Alknet uses an **irpc-based service layer** to decompose core responsibilities into independently testable, deployable, and replaceable components. Services communicate via irpc protocol enums that work both as in-process async boundaries (tokio channels) and cross-process/cross-network (QUIC streams via noq).

This document defines the service protocols and their relationships, following the head/worker terminology established in [core.md](core.md).

## Design Principles

### 1. Services are protocol enums

An irpc service is defined as a Rust enum annotated with `#[rpc_requests]`. The macro generates two versions:
- **Serializable** (`Request`): safe to encode with postcard, for remote communication
- **With channels** (`RequestWithChannels`): includes `oneshot::Sender` and `mpsc` channels, for local communication

Both versions use the same `Client<S>` type — the local/remote distinction is transparent at the call site.

### 2. Services are the async boundary

Instead of a giant `mpsc` message enum per the irpc documentation's description of the common anti-pattern, each service has its own focused protocol. This keeps responsibilities clear and prevents the "god enum" problem.

### 3. Local-first, remote-capable

Every service can run locally (mpsc channels, zero serialization overhead) or remotely (QUIC streams, postcard serialization). The deployment choice doesn't affect the call sites. A single-node setup runs everything locally. A distributed setup runs auth and secrets on dedicated nodes.

### 4. Event boundary discipline

Per [event_source_types.md](/workspace/research/event_sourcing/event_source_types.md):

- **Honker streams** = domain events (internal to the owning service, for state reconstruction)
- **irpc service calls** = request-response between services (synchronous boundary within a node)
- **Call protocol EventEnvelope** = integration events (cross-node asynchronous boundary)

Domain events are projected to integration events when crossing service or node boundaries. Never publish domain events directly to other services.

## Service Definitions

### AuthService

Verifies identities without holding all keys in memory.

```rust
use irpc::{rpc_requests, channel::{mpsc, oneshot}};
use serde::{Serialize, Deserialize};

#[rpc_requests(message = AuthMessage)]
#[derive(Debug, Serialize, Deserialize)]
enum AuthProtocol {
    #[rpc(tx=oneshot::Sender<AuthResult>)]
    #[wrap(VerifyPubkey)]
    VerifyPubkey {
        fingerprint: String,
        key_data: Vec<u8>,
    },

    #[rpc(tx=oneshot::Sender<AuthResult>)]
    #[wrap(VerifyToken)]
    VerifyToken {
        token_bytes: Vec<u8>,
        timestamp: u64,
    },

    #[rpc(tx=oneshot::Sender<()>)]
    #[wrap(ReloadKeys)]
    ReloadKeys,

    #[rpc(tx=oneshot::Sender<bool>)]
    #[wrap(CheckAccess)]
    CheckAccess {
        identity: Identity,
        operation: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
enum AuthResult {
    Ok(Identity),
    Denied(String),
}

#[derive(Debug, Serialize, Deserialize)]
struct Identity {
    node_id: String,
    fingerprint: String,
    scopes: Vec<String>,
}
```

**Backends:**

| Mode | Backend | When to use |
|------|---------|-------------|
| Minimal | `ArcSwap<DynamicConfig>` with all keys in memory | CLI, single-node, few users |
| SQLite | Query `peer_credentials` / `api_keys` on demand | Production, multi-user head nodes |
| Remote | Forward to dedicated auth service | Multi-head clusters, auth federation |

**Why this solves the scaling problem:** Instead of loading all keys into memory and swapping them atomically, the auth service queries SQLite per request. An LRU cache on hot fingerprints avoids repeated DB hits. Key revocations are propagated via honker stream notifications.

### SecretService

Derives keys from a master seed, encrypts/decrypts external credentials. The **only** component that holds the master seed phrase.

```rust
#[rpc_requests(message = SecretMessage)]
#[derive(Debug, Serialize, Deserialize)]
enum SecretProtocol {
    #[rpc(tx=oneshot::Sender<DerivedKey>)]
    #[wrap(DeriveEd25519)]
    DeriveEd25519 {
        path: String,  // e.g. "m/74'/0'/0'/0'"
    },

    #[rpc(tx=oneshot::Sender<DerivedKey>)]
    #[wrap(DeriveEncryptionKey)]
    DeriveEncryptionKey {
        path: String,  // e.g. "m/74'/2'/0'/0'"
    },

    #[rpc(tx=oneshot::Sender<DerivedKey>)]
    #[wrap(DeriveEthereumKey)]
    DeriveEthereumKey {
        path: String,  // e.g. "m/44'/60'/0'/0/0"
    },

    #[rpc(tx=oneshot::Sender<Vec<u8>>)]
    #[wrap(DerivePassword)]
    DerivePassword {
        path: String,
        length: usize,
    },

    #[rpc(tx=oneshot::Sender<EncryptedData>)]
    #[wrap(Encrypt)]
    Encrypt {
        plaintext: String,
        key_version: u32,
    },

    #[rpc(tx=oneshot::Sender<String>)]
    #[wrap(Decrypt)]
    Decrypt {
        encrypted: EncryptedData,
    },

    #[rpc(tx=oneshot::Sender<()>)]
    #[wrap(Lock)]
    Lock,

    #[rpc(tx=oneshot::Sender<()>)]
    #[wrap(Unlock)]
    Unlock {
        passphrase: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct DerivedKey {
    key_type: KeyType,
    private_key: Vec<u8>,
    public_key: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
enum KeyType {
    Ed25519,
    Aes256Gcm,
    Secp256k1,
}

#[derive(Debug, Serialize, Deserialize)]
struct EncryptedData {
    key_version: u32,
    salt: String,   // Base64-encoded
    iv: String,     // Base64-encoded
    data: String,   // Base64-encoded
}
```

**Security model:**

| State | What's in memory | What's on disk |
|-------|-----------------|---------------|
| Locked | Nothing | Encrypted database, derivation path metadata |
| Unlocked | Master seed in RAM | Same (seed is never persisted) |
| After use | Derived keys cached in RAM | Derivation paths only |

The seed phrase is entered once (at node startup or via `Unlock` call), held in memory, and never written to disk. Derived keys are computed on demand. The `Lock` call purges the seed and all cached derived keys from memory.

**Derived key patterns (see [storage.md](storage.md) for derivation path conventions):**

- Identity keys: SLIP-0010 `m/74'/0'/0'/0'` → Ed25519 keypair for alknet authentication
- Encryption keys: SLIP-0010 `m/74'/2'/0'/0'` → AES-256-GCM key for external credential encryption
- Ethereum keys: BIP32 `m/44'/60'/0'/0/0` → secp256k1 keypair for smart contract signing
- Site passwords: BIP32 `m/74'/1'/0'/{hash}'` → deterministic password derivation (orbit-db-wallet pattern)

### ConfigService

Dynamic configuration reload. Wraps `ArcSwap<DynamicConfig>` for minimal deployments, or delegates to SQLite-backed storage for production.

```rust
#[rpc_requests(message = ConfigMessage)]
#[derive(Debug, Serialize, Deserialize)]
enum ConfigProtocol {
    #[rpc(tx=oneshot::Sender<ForwardingPolicy>)]
    #[wrap(GetForwardingPolicy)]
    GetForwardingPolicy,

    #[rpc(tx=oneshot::Sender<RateLimitConfig>)]
    #[wrap(GetRateLimits)]
    GetRateLimits,

    #[rpc(tx=oneshot::Sender<()>)]
    #[wrap(ReloadForwarding)]
    ReloadForwarding {
        policy: ForwardingPolicy,
    },

    #[rpc(tx=oneshot::Sender<()>)]
    #[wrap(ReloadRateLimits)]
    ReloadRateLimits {
        limits: RateLimitConfig,
    },
}
```

### StorageService

Graph CRUD operations, metagraph management, and honker event bridge. Wraps the `alknet-storage` crate.

```rust
#[rpc_requests(message = StorageMessage)]
#[derive(Debug, Serialize, Deserialize)]
enum StorageProtocol {
    #[rpc(tx=oneshot::Sender<Graph>)]
    #[wrap(CreateGraph)]
    CreateGraph {
        graph_type_id: String,
        name: String,
    },

    #[rpc(tx=oneshot::Sender<Node>)]
    #[wrap(AddNode)]
    AddNode {
        graph_id: String,
        key: String,
        attributes: serde_json::Value,
    },

    #[rpc(tx=oneshot::Sender<Node>)]
    #[wrap(GetNode)]
    GetNode {
        graph_id: String,
        key: String,
    },

    #[rpc(tx=mpsc::Sender<StorageEvent>)]
    #[wrap(Subscribe)]
    Subscribe {
        stream_name: String,
    },
}
```

The `Subscribe` variant uses server-streaming irpc — the client sends one request and receives multiple `StorageEvent` messages via `mpsc::Sender`. These are honker stream events projected into integration events.

## Service Composition

### Minimal Deployment (Single Node, CLI)

All services run locally as tokio actors:

```
┌──────────────────────────────────────────────┐
│                 Single Process                │
│                                              │
│  ┌─────────┐  ┌─────────┐  ┌──────────────┐ │
│  │  Auth    │  │ Secret  │  │   Config     │ │
│  │ Service  │  │ Service │  │   Service    │ │
│  │ (mpsc)   │  │ (mpsc)  │  │   (mpsc)    │ │
│  └────┬─────┘  └────┬────┘  └──────┬───────┘ │
│       │             │               │         │
│  ┌────▼─────────────▼───────────────▼───────┐ │
│  │          alknet-core Server               │ │
│  │  (SSH auth, call protocol, forwarding)   │ │
│  └──────────────────────────────────────────┘ │
└──────────────────────────────────────────────┘
```

- Auth service uses `ArcSwap<DynamicConfig>` (all keys in memory)
- Secret service runs unlocked (seed in memory, no external access)
- Config service uses `ArcSwap<DynamicConfig>` directly

### Production Deployment (Multi-Node)

Auth and secrets run on dedicated nodes; workers access them remotely:

```
┌────────────────────┐     ┌─────────────────────┐
│   Auth Node        │     │   Secret Node        │
│                    │     │                      │
│  AuthProtocol      │     │  SecretProtocol      │
│  (SQLite-backed)   │     │  (seed in RAM)       │
│                    │     │                      │
└────────┬───────────┘     └──────────┬──────────┘
         │  QUIC (irpc)               │  QUIC (irpc)
         │                            │
┌────────▼────────────────────────────▼─────────┐
│              Head Node                        │
│                                               │
│  ┌──────────┐  ┌──────────┐  ┌─────────────┐ │
│  │ Config   │  │ Storage  │  │ alknet-core  │ │
│  │ Service  │  │ Service  │  │ Server       │ │
│  │ (local)  │  │ (local)  │  │              │ │
│  └──────────┘  └──────────┘  └──────────────┘ │
└───────────────────────────────────────────────┘
         │
         │  SSH / iroh / TLS
         │
┌────────▼──────────────────────────────────────┐
│              Worker Node                      │
│                                               │
│  ┌──────────┐  ┌──────────────┐               │
│  │ Storage  │  │ alknet-core  │               │
│  │ Client   │  │ Client       │               │
│  │ (remote) │  │              │               │
│  └──────────┘  └──────────────┘               │
└───────────────────────────────────────────────┘
```

Workers don't hold the seed or the auth database. They request derived keys and auth verification via irpc over QUIC.

## Service and Call Protocol Relationship

Services are **internal** — they run within a node or cluster. The call protocol is **external** — it's how nodes communicate with each other over SSH/QUIC/WebSocket/DNS transports.

A service can be exposed as a call protocol operation:

| Internal Service | Call Protocol Path | Direction |
|-----------------|-------------------|-----------|
| AuthProtocol::VerifyPubkey | `/head/auth/verify` | Worker → Head |
| SecretProtocol::DeriveEd25519 | `/head/secrets/derive` | Worker → Head (restricted) |
| StorageProtocol::Subscribe | `/{node}/storage/watch` | Any → Any |
| ConfigProtocol::GetForwardingPolicy | `/head/config/forwarding` | Worker → Head |

External workers call these through the call protocol, which routes to the service on the head node:

```
Worker                           Head
  │                               │
  │  call.requested               │
  │  operation: /head/auth/verify │
  │  payload: { fingerprint, key }│
  │ ─────────────────────────────►│
  │                               │ ┌─ AuthProtocol::VerifyPubkey ─┐
  │                               │ │ (irpc, local mpsc channel)     │
  │                               │ └─ Result: AuthResult ──────────┘
  │                               │
  │  call.responded               │
  │  payload: { status: "ok" }   │
  │ ◄─────────────────────────────│
```

## Service Integration Example

A head/worker deployment demonstrates service integration end-to-end:

- **Head node**: runs Auth, Secret, and Config services locally
- **Worker node**: connects to head via alknet call protocol

The worker-to-head protocol maps to call protocol operations:

| Worker Message | Call Protocol Path | Service |
|----------------|-------------------|---------|
| Auth | `/head/auth/verify` | AuthProtocol |
| Heartbeat | `/worker/heartbeat` (subscription) | ConfigProtocol |
| Task result | `/worker/task/submit` | StorageProtocol (persistence) |
| Task assignment | `/head/task/template` (subscription) | StorageProtocol |

Worker keys are derived from the seed by the secret service. The head node's API credentials are stored encrypted and decrypted on demand by the secret service.

## Derived Key Conventions

Standardized SLIP-0010/BIP32 paths (see [storage.md](storage.md) for full table):

| Path | Purpose | Curve/Algorithm |
|------|---------|----------------|
| `m/74'/0'/0'/0'` | Primary identity keypair | Ed25519 (alknet auth) |
| `m/74'/0'/0'/{n}'` | Worker/ device identity | Ed25519 |
| `m/74'/0'/1'/0'` | SSH host key | Ed25519 |
| `m/74'/1'/0'/{hash}'` | Site-specific password | Deterministic (like orbit-db-wallet) |
| `m/74'/2'/0'/0'` | Encryption key for external credentials | AES-256-GCM |
| `m/44'/60'/0'/0/0` | Ethereum signing key | secp256k1 (smart contract) |

The `74'` coin type is unallocated per SLIP-0044 and reserved for alknet.

## Application Services

Core services (auth, secret, config, storage) are infrastructure that every node needs. Application services are domain-specific and pluggable — they expose operations via the call protocol and are registered dynamically by the node operator.

### Service Tiers

```
┌─────────────────────────────────────────────────────────┐
│                    Application Layer                     │
│  DockerService · NodeService · WalletService · GitService│
│  ProxyService · ComputeService · AgentService · ...      │
├─────────────────────────────────────────────────────────┤
│                    Core Services                         │
│  AuthService · SecretService · ConfigService            │
│  StorageService                                         │
├─────────────────────────────────────────────────────────┤
│                    alknet-core                           │
│  Transport · Call Protocol · SSH · irpc                  │
└─────────────────────────────────────────────────────────┘
```

### DockerService

Container lifecycle management on a node. Wraps the Docker Engine API (via `bollard` crate, already used in dispatch) and exposes it through the call protocol.

```rust
#[rpc_requests(message = DockerMessage)]
enum DockerProtocol {
    #[rpc(tx=oneshot::Sender<ContainerInfo>)]
    #[wrap(CreateContainer)]
    CreateContainer { image: String, name: Option<String>, env: Vec<(String, String)>, ports: Vec<(u16, u16)> },

    #[rpc(tx=oneshot::Sender<ContainerInfo>)]
    #[wrap(InspectContainer)]
    InspectContainer { id: String },

    #[rpc(tx=oneshot::Sender<Vec<ContainerInfo>>)]
    #[wrap(ListContainers)]
    ListContainers { all: bool },

    #[rpc(tx=oneshot::Sender<()>)]
    #[wrap(StopContainer)]
    StopContainer { id: String, timeout: u64 },

    #[rpc(tx=oneshot::Sender<()>)]
    #[wrap(RemoveContainer)]
    RemoveContainer { id: String, force: bool },

    #[rpc(tx=mpsc::Sender<ContainerEvent>)]
    #[wrap(StreamEvents)]
    StreamEvents { filters: Vec<String> },
}
```

This makes container management a first-class alknet operation that can be called from any connected node, not just SSH. The dispatch project's `InstanceProvider` trait pattern maps directly here.

**Self-hosting use case**: An operator deploys a "server in a box" by connecting a worker node with DockerService registered. A head node (or another authorized node) can then deploy containers remotely via call protocol: `/node/docker/create`, `/node/docker/list`, etc. This replaces manual SSH + docker-compose with automated, auditable, policy-governed deployment.

### NodeService

System health, metrics, and tiered observability. Exposes system metrics and supports tiered escalation from small models to larger models to humans.

```rust
#[rpc_requests(message = NodeMessage)]
enum NodeProtocol {
    #[rpc(tx=oneshot::Sender<SystemMetrics>)]
    #[wrap(GetMetrics)]
    GetMetrics { categories: Vec<MetricCategory> },

    #[rpc(tx=oneshot::Sender<HealthStatus>)]
    #[wrap(HealthCheck)]
    HealthCheck,

    #[rpc(tx=mpsc::Sender<SystemEvent>)]
    #[wrap(SubscribeMetrics)]
    SubscribeMetrics { interval_ms: u64, categories: Vec<MetricCategory> },

    #[rpc(tx=oneshot::Sender<()>)]
    #[wrap(Escalate)]
    Escalate { severity: Severity, message: String, context: serde_json::Value },
}

#[derive(Serialize, Deserialize)]
enum MetricCategory { Cpu, Memory, Disk, Network, Docker, Uptime }

#[derive(Serialize, Deserialize)]
enum Severity { Info, Warning, Critical }
```

**Tiered escalation pattern**: A small model (fast, cheap) subscribes to `/node/metrics/stream` and evaluates simple rules (disk > 90%, memory > 95%, container crashed). When a rule triggers, it calls `/node/alert/escalate` with context. The head node decides whether to notify a larger model or a human.

### WalletService

Multichain wallet operations using a HD derivation library (e.g., wagyu). Derives keys from the same master seed via the secret service, signs transactions, and manages addresses.

```rust
#[rpc_requests(message = WalletMessage)]
enum WalletProtocol {
    #[rpc(tx=oneshot::Sender<AddressInfo>)]
    #[wrap(GetAddress)]
    GetAddress { chain: Chain, path: String },

    #[rpc(tx=oneshot::Sender<BalanceInfo>)]
    #[wrap(GetBalance)]
    GetBalance { chain: Chain, address: String },

    #[rpc(tx=oneshot::Sender<SignedTransaction>)]
    #[wrap(SignTransaction)]
    SignTransaction { chain: Chain, path: String, tx_params: serde_json::Value },

    #[rpc(tx=oneshot::Sender<String>)]
    #[wrap(VerifyAddress)]
    VerifyAddress { chain: Chain, address: String },
}

#[derive(Serialize, Deserialize)]
enum Chain { Bitcoin, Ethereum, Monero, Zcash }
```

The WalletService delegates key derivation to the SecretService via irpc. It never sees the master seed — only derived keypairs for specific paths. This means wallet operations are available to authorized nodes without exposing the full key hierarchy.

### ProxyService

Reverse proxy and TLS certificate management. Automates nginx/certbot configuration for services deployed via DockerService.

```rust
#[rpc_requests(message = ProxyMessage)]
enum ProxyProtocol {
    #[rpc(tx=oneshot::Sender<ProxyConfig>)]
    #[wrap(GetConfig)]
    GetConfig,

    #[rpc(tx=oneshot::Sender<()>)]
    #[wrap(AddRoute)]
    AddRoute { domain: String, upstream: String, tls: bool },

    #[rpc(tx=oneshot::Sender<()>)]
    #[wrap(RemoveRoute)]
    RemoveRoute { domain: String },

    #[rpc(tx=oneshot::Sender<CertificateInfo>)]
    #[wrap(ProvisionCert)]
    ProvisionCert { domain: String },

    #[rpc(tx=oneshot::Sender<Vec<CertificateInfo>>)]
    #[wrap(ListCerts)]
    ListCerts,
}
```

### ComputeService

Abstracts compute provider APIs (starting with dispatch's `InstanceProvider` pattern). Manages remote instances across providers.

```rust
#[rpc_requests(message = ComputeMessage)]
enum ComputeProtocol {
    #[rpc(tx=oneshot::Sender<InstanceInfo>)]
    #[wrap(CreateInstance)]
    CreateInstance { provider: String, spec: InstanceSpec },

    #[rpc(tx=oneshot::Sender<Vec<InstanceInfo>>)]
    #[wrap(ListInstances)]
    ListInstances { provider: Option<String> },

    #[rpc(tx=oneshot::Sender<()>)]
    #[wrap(DestroyInstance)]
    DestroyInstance { id: String },

    #[rpc(tx=oneshot::Sender<InstanceInfo>)]
    #[wrap(GetInstance)]
    GetInstance { id: String },
}
```

### Registration Pattern

Application services register with the call protocol's `OperationRegistry` at startup:

```rust
registry.register(
    OperationSpec { name: "/node/docker/create", namespace: "docker", ... },
    docker_service.create_container_handler,
);
registry.register(
    OperationSpec { name: "/node/metrics/stream", namespace: "node", ... },
    node_service.subscribe_metrics_handler,
);
```

A worker node that exposes Docker and Node services registers those operations when it connects to the head. The head can then route calls from any node to the appropriate worker via the call protocol.

### Self-Hosting Stack Example

A minimal self-hosted server with all services:

```
┌─────────────────────────────────────────────────────────┐
│                    Head Node                             │
│                                                         │
│  Core: Auth · Secret · Config · Storage                  │
│  App:  Docker · Node · Proxy · Git · Wallet · Compute    │
│                                                         │
│  Call protocol paths:                                   │
│    /head/auth/*                                          │
│    /head/docker/*                                        │
│    /head/proxy/*                                         │
│    /head/wallet/*                                        │
│    /head/compute/*                                       │
│    /head/node/metrics/*                                  │
└─────────────────────────────────────────────────────────┘
```

An operator deploys this by:
1. Running `alknet serve --config stack.toml`
2. Entering their seed phrase once (unlocks the secret service)
3. All services come online with keys derived from the seed
4. Docker containers for Gitea, Postgres, Redis, etc. are managed via DockerService
5. Reverse proxy and TLS are automated via ProxyService
6. Wallet keys are derived on demand via WalletService

No manual SSH, no hardcoded credentials, no separate secret management. The seed phrase is the single root of trust.

## Crate Structure

```
alknet-core/
├── transport/     — Transport trait, TCP, TLS, iroh, DNS
├── call/          — Call protocol, PendingRequestMap, OperationRegistry
├── auth/          — AuthService protocol, identity types
├── secrets/       — SecretService protocol, BIP39, SLIP-0010, AES-GCM
├── config/        — ConfigService protocol, StaticConfig, DynamicConfig
├── handler/       — ServerHandler, SSH authentication hooks
└── serve.rs       — Server::run(), multi-transport listeners

alknet-storage/
├── metagraph/     — GraphType, NodeType, EdgeType persistence
├── identity/      — accounts, organizations, peer_credentials, api_keys
├── acl/           — PrincipalNode, DelegatesEdge, access control graph
├── secrets/       — Encrypted node type, encrypt/decrypt, key derivation bridge
├── honker/        — honker integration: notify, stream, queue
├── graph/         — GraphInstance, Node, Edge CRUD with schema validation
└── schema/        — JSON Schema definitions (serde + jsonschema)
```

## Security Considerations

1. **Seed phrase is never persisted** — it's entered at startup or via `Unlock` call and held only in RAM
2. **Derived keys are cached in memory** — cleared on `Lock`
3. **External credentials are encrypted at rest** — the encryption key is itself derived from the seed
4. **Auth service never sees the seed** — it only sees public key fingerprints and verification results
5. **irpc remote communication is over QUIC** — encrypted in transit; irpc doesn't add its own encryption layer (assumes the transport provides it)
6. **Lock wipes all secrets** — a locked secret service returns errors for all requests until unlocked

## Open Questions

- **OQ-SVC-01**: Should the secret service support multiple seed phrases (one per tenant or identity)?

  The simplest approach is one seed per node. Multi-seed support (e.g., one per tenant in a multi-tenant system) can be added later by indexing the `Unlock` call with a tenant ID. Defer for now.

- **OQ-SVC-02**: Should service protocols use postcard (binary) or JSON for remote calls?

  irpc defaults to postcard for efficiency. However, the call protocol uses JSON `EventEnvelope` for cross-language compatibility. Service-to-service calls should use postcard (Rust-to-Rust), while node-to-node calls use JSON (call protocol). The irpc remote path naturally uses postcard.

- **OQ-SVC-03**: How does the secret service integrate with the existing `EncryptedDataSchema` from `@alkdev/storage`?

  The TypeScript `encrypt()`/`decrypt()` functions use PBKDF2 with a password. In Rust, the secret service replaces the password with a derived AES-256-GCM key. The `EncryptedData` schema (key_version, salt, iv, data) stays the same, but key derivation changes from PBKDF2(password) to SLIP-0010(seed, path). This is a superset — the old format can be migrated by re-encrypting with the new key.

- **OQ-SVC-04**: Should workers cache derived keys locally?

  Yes, with a TTL. A worker that holds a derived Ed25519 keypair for its session can re-authenticate without calling the secret service every time. The TTL should be configurable (default: 1 hour). The head can revoke by invalidating the session, not by expiring the key.

- **OQ-SVC-05**: How does the smart contract (NFT-based ACL) interact with the secret service?

  The Ethereum signing key (`m/44'/60'/0'/0/0`) is derived from the same seed. The secret service can sign transactions on behalf of the node. The smart contract is a separate concern — it's the external source of truth for identity registration. The local ACL graph (in `alknet-storage`) is a cache that's synced from the contract, not the other way around.

## References

- [core.md](core.md) — Core overview, transport, call protocol, head/worker model
- [configuration.md](configuration.md) — Config architecture, auth service, DynamicConfig
- [storage.md](storage.md) — Metagraph, identity, ACL, secrets, event boundaries
- [flow.md](flow.md) — Operation graph, call graph, petgraph mapping
- `/workspace/@alkdev/storage/docs/architecture/encrypted-data.md` — Original encrypted data design (TypeScript)
- `/workspace/research/event_sourcing/event_source_types.md` — Event-driven architecture patterns
- irpc crate — https://docs.rs/irpc — Service protocol definitions, local/remote abstraction
- SLIP-0010 — https://github.com/satoshilabs/slips/blob/master/slip-0010.md — HD key derivation for Ed25519
- BIP39 — https://github.com/bitcoin/bips/blob/master/bip-0039.mediawiki — Mnemonic code for generating deterministic keys (widely used beyond cryptocurrency)
- `ed25519-bip32` crate — https://docs.rs/ed25519-bip32 — BIP32-Ed25519 (Cardano/IOHK approach)
- `bip39` crate — https://docs.rs/bip39 — Mnemonic generation and seed derivation