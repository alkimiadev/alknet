---
status: draft
last_updated: 2026-06-20
---

# Open Questions

Questions are organized by theme. Each question has a stable OQ-ID for cross-referencing from spec documents.

Door type classifications follow ADR-009:
- **One-way door**: Reversal requires rewriting significant code or permanently closes a capability. Requires ADR before implementation.
- **Two-way door**: Reversal is cheap or additive. Can be decided during implementation.

## Theme: Core Types

### OQ-01: BiStream Type Definition

- **Origin**: [overview.md](overview.md)
- **Status**: resolved
- **Door type**: One-way
- **Priority**: high
- **Resolution**: BiStream is a trait (`AsyncRead + AsyncWrite + Send + Unpin`). Handlers receive a `Connection` (not a single BiStream). This preserves the WASM door — browser clients can implement BiStream over WebTransport streams. See ADR-007.
- **Cross-references**: ADR-002, ADR-007, ADR-009

### OQ-02: AuthContext Resolution Timing

- **Origin**: [overview.md](overview.md)
- **Status**: resolved
- **Door type**: One-way
- **Priority**: high
- **Resolution**: Hybrid model (Option C) — endpoint resolves what it can (e.g., TLS client certificate), handler resolves what it must (e.g., AuthToken in first frame). AuthContext may be partial when `handle()` is called. See ADR-004.
- **Cross-references**: ADR-002, ADR-004

## Theme: ALPN and Routing

### OQ-03: ALPN String Naming Convention

- **Origin**: [overview.md](overview.md)
- **Status**: resolved
- **Door type**: One-way
- **Priority**: medium
- **Resolution**: Custom ALPNs use `alknet/<name>` prefix (no version), standard ALPNs use IANA strings. No version negotiation initially. See ADR-006.
- **Cross-references**: ADR-001, ADR-006

### OQ-04: Dynamic Handler Registration at Runtime vs Static at Startup

- **Origin**: [overview.md](overview.md)
- **Status**: resolved
- **Door type**: Two-way
- **Priority**: low
- **Resolution**: Static registration at startup. `HandlerRegistry` is immutable after construction. ALPN strings in the TLS `ServerConfig` are derived from the registry at startup — adding a handler at runtime requires rebuilding the TLS config. The `ArcSwap<HandlerRegistry>` pattern can be applied later if needed (two-way door). See ADR-010.
- **Cross-references**: ADR-001, ADR-010, [endpoint.md](crates/core/endpoint.md)

## Theme: Transport and Endpoint

### OQ-05: Multi-Connectivity Endpoint

- **Origin**: [overview.md](overview.md)
- **Status**: resolved
- **Door type**: One-way
- **Priority**: high
- **Resolution**: `AlknetEndpoint` supports both `quinn::Endpoint` (public QUIC+TLS) and `iroh::Endpoint` (P2P relay-assisted) simultaneously, both optional and feature-gated. Both produce QUIC connections that dispatch through the same `HandlerRegistry` by ALPN string. These are not interchangeable transports — they serve fundamentally different deployment contexts (public IP vs NAT traversal). TCP is not an endpoint concern — bare TCP SSH is handled by the SSH handler directly. See ADR-010.
- **Cross-references**: ADR-001, ADR-010, [endpoint.md](crates/core/endpoint.md)

### OQ-06: Server-Side ALPN vs Client-Side ALPN

- **Origin**: ADR-001
- **Status**: resolved
- **Door type**: One-way
- **Priority**: low
- **Resolution**: One ALPN per connection. Clients open one QUIC connection per ALPN. QUIC connections are cheap (multiplexed over the same UDP flow). See ADR-006.
- **Cross-references**: ADR-001, ADR-006

## Theme: Call Protocol

### OQ-07: Call Protocol Scope Within a Connection

- **Origin**: ADR-005
- **Status**: resolved
- **Door type**: Two-way
- **Priority**: medium
- **Resolution**: The call protocol uses bidirectional QUIC streams with EventEnvelope framing and ID-based correlation via PendingRequestMap. The protocol is stream-agnostic — the client can open one stream per operation, multiplex on one stream, or any mix. Correlation is by request ID, not by stream. Both sides can initiate calls. One `alknet/call` connection gives access to the full operation registry (call, subscribe, batch, schema). No multiplexing layer is needed inside the connection. See ADR-012.
- **Cross-references**: ADR-005, ADR-012

## Theme: Security

### OQ-08: Vault Integration Point

- **Origin**: [overview.md](overview.md)
- **Status**: resolved
- **Door type**: One-way
- **Priority**: medium
- **Resolution**: CLI-embedded, assembly-layer only. The CLI binary instantiates `VaultServiceHandle` locally at startup, derives and decrypts the credentials each handler needs, and injects them into handler capabilities. alknet-vault has no ALPN, no alknet-core dependency, and no operations registered in the call protocol. The master seed and derived private keys never cross the network. The vault is a capability source, not a network service. See ADR-008 and ADR-014.
- **Cross-references**: ADR-003, ADR-005, ADR-008, ADR-014

## Deferred Questions

These questions are acknowledged but not active. They will be promoted to open when their crate is being specified.

### OQ-09: WASM Target Boundaries

- **Origin**: [overview.md](overview.md)
- **Status**: deferred
- **Door type**: One-way (when applicable)
- **Priority**: low
- **Resolution**: Not an active question — WASM compatibility is a design constraint (see ADR-009, overview.md design principles), not a deliverable. Specific WASM targeting decisions will be made when individual crates are implemented. The BiStream trait decision (ADR-007) has already preserved the most important WASM door.
- **Cross-references**: ADR-007, ADR-009

### OQ-10: Git Adapter Scope — Smart Protocol Only or Full Server?

- **Origin**: [overview.md](overview.md)
- **Status**: deferred
- **Door type**: Two-way
- **Priority**: low
- **Resolution**: Deferred per the cleanup plan. Start with git smart protocol over QUIC streams. ERC721 integration and full server capabilities are additive. Resolve when speccing alknet-git.
- **Cross-references**: ADR-001

## Theme: alknet-core

### OQ-11: Handler-Level Auth Resolution Observability

- **Origin**: [auth.md](crates/core/auth.md)
- **Status**: resolved
- **Door type**: Two-way
- **Priority**: medium
- **Resolution**: **Option B — handlers store resolved identity on the Connection.** When a handler resolves identity inside `handle()` (the handler-level auth phase), it calls `connection.set_identity(identity)` to store the resolved `Identity` on the connection object. The endpoint and observability layers can read it later for connection logging, audit trails, and metrics.

  Why not Option A (return identity from `handle()`): it changes the `ProtocolHandler` trait signature for all handlers, even those that don't do auth resolution (DNS, health check). It also assumes one identity per connection — but the call protocol can have different identities per request on the same connection (one connection, multiple `call.requested` events with different auth tokens). Returning a single identity from `handle()` would be misleading for the call protocol.

  Why not Option C (identity stays local): the resolved identity is useful beyond the handler. The endpoint may want to log "connection from X authenticated as Y." A connection-level observability layer needs the identity. If it stays local, every handler that resolves identity would need to duplicate logging logic, and the endpoint can't correlate connections to identities.

  **Two identity scopes exist and must not be conflated:**
  - **Connection-level identity** (this decision): set once by the handler in `handle()`, stored on `Connection`, read by the endpoint for logging/observability. This is the "connection owner" — who opened this QUIC connection.
  - **Per-request identity** (already in the call protocol spec): set per `call.requested` by the `CallAdapter`, stored on `OperationContext.identity`. This is the "call caller" — who is making this specific call, which may upgrade mid-session (different auth tokens on the same connection).

  Both exist. The connection-level identity is the stable "who is this connection from"; the per-request identity is the dynamic "who is this specific call from." The call protocol's per-request resolution (which may produce a different identity than the connection-level resolution) takes precedence for ACL on `OperationContext` — the connection-level identity is for observability only, not for ACL.

  `Connection` exposes `set_identity` via interior mutability (`OnceLock<Identity>` or `RwLock<Option<Identity>>` — the handler sets it once when resolved, the endpoint and observability layers read it). `handle()` receives `Connection` by value (owned), but the endpoint may also hold a reference for logging. The identity is write-once-read-many.
- **Cross-references**: ADR-004, ADR-011, ADR-015 (per-request identity on OperationContext), [auth.md](crates/core/auth.md)

### OQ-12: TLS Identity Provisioning in AlknetEndpoint

- **Origin**: [endpoint.md](crates/core/endpoint.md), [config.md](crates/core/config.md)
- **Status**: resolved
- **Door type**: One-way
- **Priority**: high
- **Resolution**: TLS identity in alknet has two distinct use cases, not one:

  **Use case 1 — P2P / key-based identity (default for most alknet nodes):** RFC 7250 raw Ed25519 public keys. No domain, no CA, no cert renewal. The Ed25519 public key IS the node's identity. This is the same model iroh uses with its `NodeId`. It works natively with SSH auth (same key type) and git (SSH key-based auth). `TlsIdentity::RawKey` in `StaticConfig` covers this. This is the primary identity mode for alknet-native clients — most nodes will use this.

  **Use case 2 — Domain-hosted services (relays, public-facing nodes):** X.509 certificates with domain names. Required for browser/WebTransport clients, which don't support RFC 7250. This has two sub-cases:
  - **Manual**: Provide cert/key file paths via `TlsIdentity::X509`. Already specified in `StaticConfig`.
  - **ACME auto-provisioning**: Let's Encrypt via rustls-acme. The reverse-proxy project (`/workspace/@alkdev/reverse-proxy`) demonstrates the complete pattern: per-listener ACME state machine, `ResolvesServerCertAcme` rustls integration, TLS-ALPN-01 challenge handling, automatic renewal. This is a proven, solved implementation pattern — not speculative future work. It will be adapted to alknet's `AlknetEndpoint` context when domain-hosted nodes need it.

  **Browser constraint**: Browsers require X.509 and don't support RFC 7250. For browser/WebTransport clients, domain-hosted nodes with X.509 certs are mandatory. All other clients (SSH, git, alknet-native) work with raw keys by default.

  The `TlsIdentity` enum in `StaticConfig` already captures all three modes (`X509`, `RawKey`, `SelfSigned`). ACME auto-provisioning is additive — it produces an X.509 cert at runtime rather than from files, and fits naturally as an additional `TlsIdentity` variant or as a `rustls::ResolvesServerCert` implementation behind the existing `X509` path.
- **Cross-references**: ADR-010, [config.md](crates/core/config.md), [endpoint.md](crates/core/endpoint.md)

### OQ-13: Operation Path Format and Routing Scope

- **Origin**: [operation-registry.md](crates/call/operation-registry.md)
- **Status**: resolved
- **Door type**: Two-way
- **Priority**: medium
- **Resolution**: alknet-call uses `/{service}/{op}` (e.g., `/fs/readFile`, `/agent/chat`, `/services/list`). This is the correct format for the alknet-call crate — it is not a "Phase 1 simplification" but the right design for this architecture. The `/{node}/{service}/{op}` pattern from the reference implementation served a head/worker routing model that is a separate architectural concern. Remote dispatch (federation / node-level routing) would be a different mechanism at a different layer, not a prefix added to alknet-call's operation paths. If remote dispatch is ever needed, it would be addressed by a separate crate or a routing layer above the operation registry, not by changing alknet-call's path format. Two-way door — the path format can be extended later if needed, but `/{service}/{op}` is the correct design now.
- **Cross-references**: ADR-005, ADR-012

### OQ-14: Batch Operation Semantics

- **Origin**: [call-protocol.md](crates/call/call-protocol.md)
- **Status**: resolved
- **Door type**: Two-way
- **Priority**: low
- **Resolution**: Batch is a client-side pattern — multiple `call.requested` events with correlated IDs, responses arrive independently. This is the correct protocol design, not a simplification to be "upgraded" later. QUIC's stream multiplexing already provides the concurrency and ordering guarantees that batch would need. Batch-specific event types (e.g., `batch.requested`, `batch.responded`) would add protocol complexity without clear benefit over sending multiple `call.requested` events. If a compelling use case for atomic batch semantics emerges, it can be added as a new event type without breaking existing clients. Two-way door.
- **Cross-references**: ADR-012

## Theme: alknet-call

### OQ-15: Call Protocol Client and Adapter Contract

- **Origin**: [call-protocol.md](crates/call/call-protocol.md), [operation-registry.md](crates/call/operation-registry.md), ADR-013
- **Status**: resolved
- **Door type**: One-way
- **Priority**: high
- **Resolution**: `CallClient` opens QUIC connections and shares the dispatch loop with `CallAdapter` — both sides can send and receive `call.requested` once connected. Connection direction (who opened the connection) is independent of call direction (who calls whom). `from_call` adapter discovers remote operations via `services/list` + `services/schema` and registers them with forwarding handlers — same pattern as `from_openapi` and `from_mcp`. `to_openapi` and `to_mcp` project local operations to external protocols. Adapter contract trait (`OperationAdapter`) produces `(OperationSpec, Handler)` pairs. Cross-node call tree: abort cascade (ADR-016) propagates across node boundaries through `from_call` handlers. Credentials for connections come from capabilities (ADR-014). Adapter-registered operations are `Internal` by default (ADR-015). See ADR-017.
- **Cross-references**: ADR-005, ADR-013, ADR-014, ADR-015, ADR-016, ADR-017, [call-protocol.md](crates/call/call-protocol.md), [operation-registry.md](crates/call/operation-registry.md)

### OQ-16: Safe Vault Operations for Call Protocol Exposure

- **Origin**: [operation-registry.md](crates/call/operation-registry.md), ADR-008
- **Status**: resolved
- **Door type**: One-way
- **Priority**: high
- **Resolution**: No vault operations are exposed over the call protocol for now. The vault is accessed only at the assembly layer (CLI binary at startup). Handlers receive secret material through `OperationContext.capabilities`, not by calling vault operations over the wire. The `operation-registry.md` spec previously showed `vault/derive`, `vault/unlock`, and `vault/decrypt` registered as call protocol operations — that was a contradiction with ADR-008's "capability source" model and has been corrected. If a future use case requires exposing a vault operation over the call protocol (e.g., a restricted `vault/public-key` operation that returns only public key material for identity verification), it would require its own ADR with an explicit threat model justification. See ADR-014.
- **Cross-references**: ADR-008, ADR-014, [operation-registry.md](crates/call/operation-registry.md)

### OQ-17: Abort Cascade Semantics for Nested Calls

- **Origin**: [call-protocol.md](crates/call/call-protocol.md), [operation-registry.md](crates/call/operation-registry.md)
- **Status**: resolved
- **Door type**: One-way (protocol schema), two-way (mechanism)
- **Priority**: high
- **Resolution**: `call.aborted` cascades to all non-terminal descendants in the call tree. The CallAdapter walks the tree (indexed by `parent_request_id` in `PendingRequestMap`) and sends `call.aborted` for each descendant. Default policy is `abort-dependents` (abort everything downstream); `continue-running` is an opt-in for long-running work that should survive a parent's abort. Handlers clean up via Rust's async drop semantics (future dropped → `Drop` guards release resources). The cascade is protocol-level (server discovers descendants and propagates); the mechanism (parent-indexed map, cancellation tokens, or a separate graph) is a two-way door. See ADR-016.
- **Cross-references**: ADR-012, ADR-015, ADR-016, [call-protocol.md](crates/call/call-protocol.md), [operation-registry.md](crates/call/operation-registry.md)

### OQ-18: Privilege Model and Authority Context

- **Origin**: [operation-registry.md](crates/call/operation-registry.md)
- **Status**: resolved
- **Door type**: One-way (ACL model), two-way (specific APIs)
- **Priority**: high
- **Resolution**: The `internal` flag on `OperationContext` marks calls that originated from composition (a handler calling another operation via `OperationEnv`), as opposed to external calls that arrived as `call.requested` from a wire client. The `internal` flag switches the authority context: the ACL check runs against the composing handler's identity (set at registration), not the caller's identity and not as a blanket skip. This replaces the previous `trusted` flag, which skipped ACL entirely — a privilege escalation vector. Operations have External/Internal visibility. Internal operations return `NOT_FOUND` when called from the wire and are excluded from `services/list`. The composition env is scoped — a handler can only invoke a declared set of operations. Handler identity is carried on `OperationContext` alongside caller identity (the principal/agent pair). See ADR-015.
- **Cross-references**: ADR-014, ADR-015, [call-protocol.md](crates/call/call-protocol.md), [operation-registry.md](crates/call/operation-registry.md)

### OQ-19: Session-Scoped Operation Registries and Agent-Written Operations

- **Origin**: [operation-registry.md](crates/call/operation-registry.md)
- **Status**: resolved
- **Door type**: Two-way (protocol doesn't need changes), one-way (if implementation closes the door)
- **Priority**: medium
- **Resolution**: The call protocol supports session-scoped registries through `OperationEnv` trait layering. No protocol changes needed. The pattern is documented here and in [operation-registry.md](crates/call/operation-registry.md) to prevent an implementation from accidentally closing it.

  The registry model has three tiers:

  | Tier | Scope | Lifetime | Visibility | Who populates it |
  |------|-------|----------|------------|-------------------|
  | Core (global) | All sessions | Process lifetime, static at startup | External + Internal (curated) | Assembly layer at startup |
  | Session | One session | Session lifetime, dynamic | Internal only (never wire-facing) | Agent during session (sandbox) |
  | Promotion | Session → Core | One-time transition | Manual/curated review | Human or architect agent reviews, then redeploys |

  Session-scoped operations are always `Internal` (ADR-015), run under the handler's identity (the agent handler that authorized the sandbox), can only compose operations in the handler's scoped env, and are ephemeral (gone when the session ends). Core operations are curated — reviewed before promotion. The promotion path is the curation checkpoint where autonomous (session-scoped) becomes curated (core). This is not auto-promotion.

  **Implementation guard**: `OperationEnv` must remain a trait, not a concrete type. A session-scoped env wraps the global env (check session registry first, fall through to global). Making `OperationEnv` concrete or hardcoding the global registry into the dispatch path would close this pattern. The static registration constraint (OQ-04) applies to the global registry only; session registries are dynamic by nature and are a different registry overlaying the global one.

  Session-scoped operations run in a locked-down sandbox (no direct net/fs/env access), can only reach operations in the handler's scoped env, and their output should be validated against their declared schema before returning. The promotion path requires review — an agent with a `promote` scope (the architect role) performs the promotion; the writing agent (lower-privileged role) requests it. This is the role-based escalation pattern (ADR-015): privileges escalate through a chain of command, not through direct authority.

  The agent-specific mechanism (quickjs sandbox, session registry lifecycle, promotion workflow) belongs to the agent crate spec. The call protocol's job is to keep the `OperationEnv` trait composable and the visibility/ACL model consistent across tiers.
- **Cross-references**: OQ-04, ADR-014, ADR-015, ADR-016, [operation-registry.md](crates/call/operation-registry.md)

## Theme: alknet-vault

### OQ-20: Salt/KDF and Encryption Key Derivation Method

- **Origin**: [encryption.md](crates/vault/encryption.md)
- **Status**: resolved
- **Door type**: One-way (key derivation method), two-way (salt field usage)
- **Priority**: high
- **Resolution**: The vault uses SLIP-0010 HD derivation from the BIP39 seed at path `m/74'/2'/0'/0'` to produce the AES-256-GCM encryption key — not PBKDF2. The `salt` field in `EncryptedData` is unused for key derivation (kept for wire-format compatibility with the TS predecessor). The TypeScript `@alkdev/storage` crypto module used PBKDF2 with a password + salt; data encrypted by that method (key_version=1) cannot be decrypted by the vault and must be migrated via one-time re-encryption to key_version=2. See ADR-020 for the full rationale and migration path.
- **Cross-references**: ADR-020, [encryption.md](crates/vault/encryption.md)

### OQ-21: Remote Vault Administration

- **Origin**: [service.md](crates/vault/service.md), [protocol.md](crates/vault/protocol.md), ADR-019
- **Status**: deferred
- **Door type**: One-way (if implemented — wire format exposure), two-way (enabling is non-breaking)
- **Priority**: medium
- **Resolution**: The `VaultProtocol` is a remote-capable irpc service by construction — the `#[rpc_requests]` macro generates both `Service` (local) and `RemoteService` (remote) trait implementations. `DerivedKey`'s dual serialization (JSON redacts private key for safety; postcard preserves bytes for remote dispatch) was designed for this. Enabling remote vault access is a server-setup change (register `IrohProtocol` with an ALPN), not a protocol change.

  **What's already in place:**
  - Protocol: `VaultProtocol` is already a `RemoteService`
  - Serialization: `DerivedKey` redacts in JSON, preserves in postcard
  - Actor: `VaultServiceActor` processes all message types, transport-agnostic
  - Auth transport: irpc over iroh uses iroh's QUIC connections (NodeId auth, RFC 7250 raw keys)

  **What's not in place (the gap):**
  - The `IrohProtocol` handler forwards all message types without auth checks
  - Remote use needs: (1) NodeId allowlist, (2) message filtering (reject `Unlock`/`Lock` from remote callers), (3) forwarding to the actor
  - This auth-wrapping handler cannot live in the vault crate (standalone, ADR-018) — it needs alknet-core's auth model (`IdentityProvider`, scopes). It lives in the assembly layer or a dedicated vault-server crate that depends on both alknet-core and alknet-vault.

  **Operation access policy:**
  - `Unlock` and `Lock` are local-only (mnemonic and lock control must not be remotely accessible)
  - All other operations (`DeriveEd25519`, `DeriveEncryptionKey`, `DeriveEthereumKey`, `DerivePassword`, `Encrypt`, `Decrypt`) are remote-capable
  - The policy is documented in the vault spec; the assembly-layer listener enforces it

  **Use case:** machine node (head, holds mnemonic) exposes restricted vault API to workers (ephemeral, no mnemonic) over irpc/iroh. Per-machine-node vaults, not shared — compartmentalization limits blast radius.

  **What's breaking vs. non-breaking:**
  - Enabling remote access: non-breaking (server-setup change)
  - Restricting operations / adding auth: non-breaking (handler policy)
  - Adding new `VaultProtocol` variants: wire break (inherent to irpc; manageable via ALPN versioning `alknet/vault/v2`)
  - Changing `DerivedKey` serialization: non-breaking (dual serialization already in place)

  **Why deferred:** the capability is available and the use cases are clear (machine→worker credential access), but no current deployment needs it. The door is left open intentionally — irpc's remote support was chosen for this reason. When a use case materializes, the assembly-layer auth-wrapping handler is the implementation task, not a protocol change. The vault spec documents the policy (which operations are remote-capable) so the future implementer has clear guidance.
- **Cross-references**: ADR-005, ADR-008, ADR-014, ADR-018, ADR-019, [protocol.md](crates/vault/protocol.md), [service.md](crates/vault/service.md)

### OQ-22: Key Rotation Mechanism

- **Origin**: [encryption.md](crates/vault/encryption.md)
- **Status**: resolved
- **Door type**: One-way (path scheme), two-way (rotation policy)
- **Priority**: medium
- **Resolution**: Key rotation uses version-indexed derivation paths. Each key version maps to a distinct SLIP-0010 path: `m/74'/2'/0'/{version-2}'`. v2 (current) is at `m/74'/2'/0'/0'`; v3 is at `m/74'/2'/0'/1'`; etc. The `decrypt` method derives the key at the path indicated by `encrypted.key_version` (not always at `PATHS::ENCRYPTION`). The `rotate` method decrypts with the old version's key and re-encrypts with the new version's key — no new mnemonic needed. The assembly layer or a migration tool iterates stored blobs and calls `rotate` on each; the vault does not self-rotate. Partial rotation is safe (old keys remain derivable). See ADR-021.
- **Cross-references**: ADR-020, ADR-021, [encryption.md](crates/vault/encryption.md), [service.md](crates/vault/service.md)

### OQ-23: Handler Identity Registration Path and Composition Authority

- **Origin**: [operation-registry.md](crates/call/operation-registry.md), [call-protocol.md](crates/call/call-protocol.md), ADR-015
- **Status**: resolved
- **Door type**: One-way (security model), two-way (bundle shape)
- **Priority**: high
- **Resolution**: ADR-015 said handler identity was "set at registration by the assembly layer" but the registration API (`register(spec, handler)`) had no place for it — meaning every internal call would check ACL against `None`, reproducing the escalation gap ADR-015 was written to close. ADR-022 resolves this with a registration bundle (`HandlerRegistration`) carrying `provenance`, `composition_authority` (replacing `handler_identity: Identity` — it's a declared authority bundle, not a peer identity), `scoped_env`, and `capabilities`. The dispatch path (`build_root_context` and `OperationEnv::invoke()`) reads from the bundle. Provenance determines which ops can compose: only `Local` and `Session` get composition authority; leaves (`FromOpenAPI`, `FromMCP`, `FromCall`) get `None` — they don't compose, so they don't need it. Capabilities are per-request on `OperationContext`, populated from the bundle (resolving the closure-capture vs context ambiguity). The kernel/user analogy: user's authority checked once at the External gate; handler's composition authority used for all composition inside; scoped env bounds reachability. No intersection — the user's authority does not limit internal calls. See ADR-022.
- **Cross-references**: ADR-014, ADR-015, ADR-022, docs/reviews/001-pre-implementation-architecture-sanity-check.md (C1–C4), [operation-registry.md](crates/call/operation-registry.md), [call-protocol.md](crates/call/call-protocol.md)

### OQ-24: Operation Error Schemas

- **Origin**: [operation-registry.md](crates/call/operation-registry.md), [call-protocol.md](crates/call/call-protocol.md), ADR-017
- **Status**: resolved
- **Door type**: One-way (wire format), two-way (mapping mechanism)
- **Priority**: high
- **Resolution**: `OperationSpec` gains `error_schemas: Vec<ErrorDefinition>` where each `ErrorDefinition` carries a `code`, `description`, `schema` (JSON Schema for the error detail payload), and optional `http_status` (for adapter projection). The `call.error` payload gains an optional `details` field carrying the typed error payload. Protocol-level codes (`NOT_FOUND`, `FORBIDDEN`, `INVALID_INPUT`, `INTERNAL`, `TIMEOUT`) are distinct from operation-level domain codes (`FILE_NOT_FOUND`, `RATE_LIMITED`, etc.) — protocol codes are emitted by the dispatch machinery, operation codes by handlers. `from_openapi`/`to_openapi` map OpenAPI response status codes to/from `ErrorDefinition`s, making the adapter contract from ADR-017 faithful on the error axis. `services/schema` exposes `error_schemas` for client code generation. See ADR-023.
- **Cross-references**: ADR-017, ADR-023, docs/reviews/001-pre-implementation-architecture-sanity-check.md (C5), [operation-registry.md](crates/call/operation-registry.md), [call-protocol.md](crates/call/call-protocol.md)