---
status: draft
last_updated: 2026-07-02
---

# Open Questions

Questions are organized by theme. Each question has a stable OQ-ID for cross-referencing from spec documents.

Door type classifications follow ADR-009 — they describe **reversal cost** (how expensive it is to undo), not urgency:
- **One-way door**: Reversal requires rewriting significant code or permanently closes a capability. Getting it wrong is expensive — requires ADR before implementation.
- **Two-way door**: Reversal is cheap or additive. Getting it wrong is recoverable — decide, implement, revert if needed.

Door type is separate from whether a decision is made. A two-way door is a decision you make now and can revert later, not a decision to defer. See ADR-009 §"What this framework is NOT."

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

  **Scope clarification (ADR-024)**: This resolution applies to the
  **`HandlerRegistry`** (ALPN string → `ProtocolHandler`), which is what
  ADR-010 governs. The call protocol's **`OperationRegistry`** (operation
  name → `HandlerRegistration`) is a *separate* registry living inside the
  `CallAdapter`, behind the single ALPN `alknet/call`. Its mutability
  profile is governed by ADR-024, not by this OQ. ADR-024 layers the
  operation registry by trust boundary: curated `Local` ops are immutable
  (same rationale as here — composing ops are privileged, the startup trust
  boundary is where their authority is granted); `Session` and imported
  (`FromCall` etc.) ops are dynamic at their respective trust-boundary
  scopes (session, connection). The pre-ADR-024 blanket immutability claim
  in `operation-registry.md` was inherited by analogy from this OQ and did
  not actually apply — the TLS-config argument that justifies
  `HandlerRegistry` immutability does not touch the `OperationRegistry`.
- **Cross-references**: ADR-001, ADR-010, ADR-024, [endpoint.md](crates/core/endpoint.md), [operation-registry.md](crates/call/operation-registry.md)

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
- **Resolution**: Not an active question — WASM compatibility is a design constraint (see ADR-009, overview.md design principles), not a deliverable. Specific WASM targeting decisions will be made when individual crates are implemented. **BiStream being a trait preserves the *client-side* stream door** — a browser can implement BiStream over WebTransport streams. **The *server-side* dispatch door is NOT preserved by ADR-007 and is a known, accepted closure**: `Connection` is a concrete quinn-bound struct (not a trait), the accept loop uses `tokio::spawn` (tokio does not run on WASM), and the call-protocol dispatch internals (`PendingRequestMap`, `CallAdapter`) use tokio `oneshot`/`mpsc` channels. A WASM server-side peer would require a `Connection` trait and a runtime-abstracted accept loop — not planned. The browser path is client-side via a JS SDK, not server-side Rust-to-WASM. This is an explicit one-way door, not an oversight.
- **Cross-references**: ADR-007, ADR-009

### OQ-10: Git Adapter Scope — Smart Protocol Only or Full Server?

- **Origin**: [overview.md](overview.md)
- **Status**: deferred
- **Door type**: Two-way
- **Priority**: low
- **Resolution**: Deferred per the cleanup plan. Start with git smart protocol over QUIC streams. ERC721 integration and full server capabilities are additive. **Composability fork (review #002 W18)**: whether git operations are registered in the `OperationRegistry` and callable via `env.invoke()`, or only available as raw smart protocol on `alknet/git`, is a separate decision from ERC721 scope. The path of least resistance (raw smart protocol only) forecloses agent composition of git operations — an agent handler that wants to compose `git/clone` cannot, because there's no `OperationSpec`, no `Handler`, no registration. To make git composable, a call-protocol projection (a set of `HandlerRegistration` bundles wrapping git operations behind the registry) must be built alongside or instead of the raw handler. Resolve this when speccing alknet-git, not deferred past it.
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

  **C13 resolution (review #002)**: the endpoint does **not** read
  `identity()` after `handle()` returns. The `Connection` is moved into the
  spawned handler task (endpoint.md), so the endpoint no longer has a
  reference to it. Connection-level observability (remote addr, ALPN,
  connection ID) is logged by the endpoint *before* the move. Identity-level
  observability is logged by the handler (the handler knows which identity
  it resolved and can log it). There is no `Arc<Connection>` sharing or
  channel-based identity-reporting mechanism — the simplest honest answer
  that avoids over-engineering the observability path before there's a
  demonstrated need. If a future use case requires the endpoint to
  correlate connections to identities, an `Arc<Connection>` or a
  side-channel can be added then.
- **Cross-references**: ADR-004, ADR-011, ADR-015 (per-request identity on OperationContext), [auth.md](crates/core/auth.md)

### OQ-12: TLS Identity Provisioning in AlknetEndpoint

- **Origin**: [endpoint.md](crates/core/endpoint.md), [config.md](crates/core/config.md)
- **Status**: resolved
- **Door type**: One-way
- **Priority**: high
- **Resolution**: TLS identity in alknet has two distinct use cases, not one:

  **Use case 1 — P2P / key-based identity (default for most alknet nodes):** RFC 7250 raw Ed25519 public keys. No domain, no CA, no cert renewal. The Ed25519 public key IS the node's identity. This is the same model iroh uses with its `NodeId`. It works natively with SSH auth (same key type) and git (SSH key-based auth). `TlsIdentity::RawKey(Ed25519SecretKey)` in `StaticConfig` covers this. As of [ADR-027](decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md), `RawKey` uses `ed25519_dalek::SigningKey` (via an alknet-core wrapper), **not** `iroh::SecretKey` — so raw-key TLS identity is available in quinn-only builds without the `iroh` feature.

  **Use case 2 — Domain-hosted services (relays, public-facing nodes):** X.509 certificates with domain names. Required for browser/WebTransport clients, which don't support RFC 7250. This has two sub-cases:
  - **Manual**: Provide cert/key file paths via `TlsIdentity::X509`. Already specified in `StaticConfig`.
  - **ACME auto-provisioning**: Let's Encrypt via `rustls-acme`. `TlsIdentity::Acme { domains, cache_dir, directory, contact }` carries static config; the endpoint constructs the `AcmeState` async state machine at setup time. Feature-gated behind `acme`. Designed in [ADR-027](decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md). The reverse-proxy project (`/workspace/@alkdev/reverse-proxy`) demonstrates the proven pattern: `AcmeConfig`, `ResolvesServerCertAcme`, TLS-ALPN-01 challenge handling, automatic renewal.

  **Browser constraint**: Browsers require X.509 and don't support RFC 7250. For browser/WebTransport clients, domain-hosted nodes with X.509 certs are mandatory. All other clients (SSH, git, alknet-native) work with raw keys by default.

  The `TlsIdentity` enum in `StaticConfig` captures all four modes (`X509`, `RawKey`, `SelfSigned`, `Acme`). [ADR-027](decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md) records the design decisions for ACME integration and RawKey decoupling.
- **Cross-references**: ADR-010, ADR-027, [config.md](crates/core/config.md), [endpoint.md](crates/core/endpoint.md)

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

  **Implementation guard**: `OperationEnv` must remain a trait, not a concrete type. A session-scoped env wraps the global env (check session registry first, fall through to global). Making `OperationEnv` concrete or hardcoding the global registry into the dispatch path would close this pattern. The static registration constraint (OQ-04) applies to the curated (Layer 0) registry only; session registries are dynamic by nature and are a different registry overlaying the curated one. **Generalized by ADR-024**: connection-scoped remote imports (`from_call`) use the same overlay mechanism as session-scoped ops. Both are per-scope dynamic overlays on the static curated base, composed into the per-call `OperationContext.env` by the `CallAdapter`. `OperationEnv` being a trait object (`Arc<dyn OperationEnv + Send + Sync>`) is what enables both overlay patterns.

  Session-scoped operations run in a locked-down sandbox (no direct net/fs/env access), can only reach operations in the handler's scoped env, and their output should be validated against their declared schema before returning. The promotion path requires review — an agent with a `promote` scope (the architect role) performs the promotion; the writing agent (lower-privileged role) requests it. This is the role-based escalation pattern (ADR-015): privileges escalate through a chain of command, not through direct authority.

  The agent-specific mechanism (quickjs sandbox, session registry lifecycle, promotion workflow) belongs to the agent crate spec. The call protocol's job is to keep the `OperationEnv` trait composable and the visibility/ACL model consistent across tiers.
- **Cross-references**: OQ-04, ADR-014, ADR-015, ADR-016, ADR-024, [operation-registry.md](crates/call/operation-registry.md)

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
- **Status**: resolved
- **Door type**: One-way (vault crate is local-only by construction)
- **Priority**: medium
- **Resolution**: Remote vault access is **not a feature of the vault crate**. ADR-025 dropped irpc from the vault, making the vault local-only by construction — no `RemoteService` trait, no wire format for vault messages, no default-insecure remote handler. The vault's API is `VaultServiceHandle` (direct method calls), nothing else.

  If remote vault access is ever needed (e.g., the machine→worker pattern), it requires a **separate vault-server crate** that depends on both alknet-core (for `IdentityProvider`, scopes, auth-wrapping) and alknet-vault (for `VaultServiceHandle`). That crate would define its own threat model, access policy, operation filtering (Unlock/Lock local-only), and wire format — and requires its own ADR. This is a deliberate addition, not a flag flip on a default that was already loaded.

  The pre-ADR-025 deferral framed remote access as "non-breaking" (the wire format was additive). That framing was misleading: once workers build dependencies on the remote vault API, disabling it breaks them — the door is operationally one-way even if the wire format is additive. ADR-025 inverts the default: the vault is local-only by construction, and remote access requires building something new, not removing a default.

  Per-node vaults are the recommended pattern for multi-node deployments: each node has its own vault and mnemonic; credentials are encrypted *for* the receiving node's public key, not decrypted centrally. This is end-to-end encryption between nodes, matching ADR-008's "capability source" model.
- **Cross-references**: ADR-005, ADR-008, ADR-014, ADR-018, ADR-019, ADR-025, [protocol.md](crates/vault/protocol.md), [service.md](crates/vault/service.md)

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
- **Resolution**: `OperationSpec` gains `error_schemas: Vec<ErrorDefinition>` where each `ErrorDefinition` carries a `code`, `description`, `schema` (JSON Schema for the error detail payload), and optional `http_status` (for adapter projection). The `call.error` payload gains an optional `details` field carrying the typed error payload.   Protocol-level codes (`NOT_FOUND`, `FORBIDDEN`, `INVALID_INPUT`,
  `INVALID_OPERATION_TYPE`, `INTERNAL`, `TIMEOUT`) are distinct from
  operation-level domain codes (`FILE_NOT_FOUND`, `RATE_LIMITED`, etc.) —
  protocol codes are emitted by the dispatch machinery, operation codes by
  handlers. The six-code protocol-level list was extended from five by
  ADR-049 (`INVALID_OPERATION_TYPE`). `from_openapi`/`to_openapi` map OpenAPI response status codes to/from `ErrorDefinition`s, making the adapter contract from ADR-017 faithful on the error axis. `services/schema` exposes `error_schemas` for client code generation. See ADR-023.
- **Cross-references**: ADR-017, ADR-023, docs/reviews/001-pre-implementation-architecture-sanity-check.md (C5), [operation-registry.md](crates/call/operation-registry.md), [call-protocol.md](crates/call/call-protocol.md)

## Theme: Call Client and Adapters

These open questions are the remainders from the call-completion gap analysis
(`docs/research/alknet-call-completion/gap-analysis.md`, DC-1..4) and the
peer-graph routing research (`docs/research/alknet-call-peer-routing/findings.md`).
ADR-029 supersedes ADR-028 and dissolves OQ-25 and the cross-peer half of
OQ-28. Most of the remaining OQs are now resolved (decisions made, defaults
recorded). OQ-29 is the exception — it's load-bearing on ADR-030 and
requires a decision before the ADR-029 migration lands. OQ-32 (multi-hop)
is a feature extension, not an unmade architecture decision.

### OQ-25: ~~Remote-Safe Marking Shape for CallClient Peer-Scoped Filtering~~ (Dissolved by ADR-029)

- **Origin**: [client-and-adapters.md](crates/call/client-and-adapters.md), ADR-017 (§1 Consequences), ADR-028
- **Status**: **dissolved** (ADR-029)
- **Door type**: ~~Two-way (shape only — existence is one-way, resolved by ADR-028)~~
- **Priority**: ~~medium~~
- **Resolution**: **Dissolved by [ADR-029](decisions/029-peer-graph-routing-model.md).**
  ADR-028's `remote_safe: bool` / `trusted_peer` model is superseded — it was a
  parallel, weaker authorization system that duplicated the existing
  `AccessControl`/`Identity` machinery. ADR-029 retires `remote_safe`/
  `trusted_peer` entirely; peer authorization flows through
  `AccessControl::check(peer_identity)`. The op's `AccessControl` *is* the
  peer-authorization policy — there is no separate marking. Per-peer
  differentiation is via `IdentityProvider` config (different peers get
  different scopes), not a per-op boolean. The "shape" question is moot
  because there is no marking to shape. See ADR-029 §3.
- **Cross-references**: ADR-009, ADR-014, ADR-015, ADR-017, ADR-022, ADR-024,
  ~~ADR-028~~ (superseded), ADR-029, [client-and-adapters.md](crates/call/client-and-adapters.md),
  [operation-registry.md](crates/call/operation-registry.md)

### OQ-26: OperationAdapter Error Type (AdapterError Variants)

- **Origin**: [client-and-adapters.md](crates/call/client-and-adapters.md), ADR-017 §5, [ADR-029](decisions/029-peer-graph-routing-model.md) §5
- **Status**: **resolved** (2026-06-27)
- **Door type**: Two-way
- **Priority**: medium
- **Resolution**: The `AdapterError` enum is `#[non_exhaustive]` +
  `thiserror::Error`, with these v1 variants:
  - `DiscoveryFailed { message: String }` — `from_call` remote unreachable / `services/list` failed
  - `SchemaParse { message: String }` — `from_openapi` / `from_jsonschema` couldn't parse the spec
  - `Transport { message: String }` — underlying transport error (QUIC for `from_call`, HTTP for `from_openapi`/`from_mcp`)
  - `Unauthorized { message: String }` — HTTP 401 for `from_openapi`/`from_mcp`, auth rejected for `from_call`
  - `SamePeerCollision { message: String }` — namespace collision *within a single peer* (ADR-029 §5: cross-peer collision dissolves; same-peer collision stays an error). Replaces the flat `Conflict` variant from the pre-ADR-029 implementation.

  `#[non_exhaustive]` lets `alknet-http`'s adapters extend without breaking
  match arms. The variant payloads are `String` messages — kept simple and
  `Send + Sync` by construction. This matches the shipped implementation
  (`crates/alknet-call/src/client/mod.rs`) except `Conflict` →
  `SamePeerCollision` (the ADR-029 migration renames it). Two-way door:
  adding variants later is non-breaking; renaming a variant is a match-arm
  update but not an architectural change.
- **Cross-references**: ADR-017, ADR-029, [client-and-adapters.md](crates/call/client-and-adapters.md)

### OQ-27: from_call Re-Import Trigger

- **Origin**: [client-and-adapters.md](crates/call/client-and-adapters.md), ADR-017 Assumption 4
- **Status**: **resolved** (2026-06-27)
- **Door type**: Two-way
- **Priority**: low
- **Resolution**: The decision is **auto-re-import on connection
  establishment**. The overlay is per-connection (Layer 2, ADR-024), so a
  stale overlay dies with the connection; re-import on reconnect is
  naturally scoped to the new connection. This is the right default for the
  runner pattern (a worker reconnects → the hub re-discovers the worker's
  ops automatically). An explicit `CallConnection::refresh()` method is a
  genuine feature addition — non-breaking, additive — if a deployment
  needs manual control.
- **Cross-references**: ADR-017, ADR-024, [client-and-adapters.md](crates/call/client-and-adapters.md)

### OQ-28: from_call Namespace Collision Behavior

- **Origin**: [client-and-adapters.md](crates/call/client-and-adapters.md), ADR-017 §3
- **Status**: **resolved** (2026-06-27)
- **Door type**: Two-way
- **Priority**: low
- **Resolution**: ADR-017 §3's `FromCallConfig` namespace prefix is
  **optional, default no prefix, same-peer collision = error**. A node
  importing from a peer that exposes two ops with the same name should fail
  loudly rather than silently overwrite. This matches the default-deny,
  explicit-allow posture (ADR-015). The alternative (last-wins) would
  silently mask one op behind another, which is the kind of surprise the
  default-deny posture exists to avoid.

  **Cross-peer collision dissolved by ADR-029.** Under the peer-keyed
  overlay model, same name on different peers is fine — they live in
  separate peer sub-overlays, no collision, no prefix needed.
  `FromCallConfig::namespace_prefix` is optional local-naming sugar for
  when the importing node wants to expose a peer's ops under a different
  name *locally* — a local-naming concern, not a disambiguation concern.
  See ADR-029 §5.
- **Cross-references**: ADR-015, ADR-017, ~~ADR-028~~ (superseded), ADR-029,
  [client-and-adapters.md](crates/call/client-and-adapters.md)

### OQ-29: CallClient TLS Client-Auth and Remote-Identity Verification

- **Origin**: [client-and-adapters.md](crates/call/client-and-adapters.md), ADR-017 §7
- **Status**: **resolved** (2026-06-27 by ADR-030 §6 + this decision)
- **Door type**: One-way (identity model interaction), two-way (mechanism)
- **Priority**: ~~high~~ → resolved
- **Resolution**: **Three things are decided:**

  1. **Wire quinn client-auth.** The client presents its Ed25519 key as an
     RFC 7250 raw public key client cert (the client-side equivalent of
     the server's `RawKeyCertResolver`). The server's
     `AcceptAnyCertVerifier` already requests client certs and extracts
     the fingerprint — the gap was entirely on the client side
     (`with_no_client_auth()` → present the key). This activates the
     `PeerEntry` fingerprint → `peer_id` resolution path on quinn
     connections.

  2. **Key-type-aware server cert verification.** The client's
     `ServerCertVerifier` depends on the remote's identity type:
     - **Ed25519 raw key** (the common case): accept the cert, extract the
       fingerprint, match against `PeerEntry.fingerprints`. The fingerprint
       IS the trust anchor — no CA needed. (Same model as iroh.)
     - **X.509** (domain-facing endpoints, ACME): verify against a CA
       (rustls's `WebPkiServerVerifier` with the platform root store or a
       configured CA). `AcceptAnyServerCertVerifier` is a security hole for
       X.509 — it's only safe for raw keys.
     - The verifier choice is driven by `CallCredentials.remote_identity`,
       which carries the expected key type.

  3. **Fingerprint normalization** (ADR-030 §6): the quinn path extracts
     the raw Ed25519 public key from the SPKI cert and formats it as
     `ed25519:<hex>`, matching iroh. The same key has the same fingerprint
     regardless of transport. X.509 fingerprints stay as `SHA256:<hex of
     DER>`.

  **The iroh path already works** — iroh uses RFC 7250 raw keys, both
  sides automatically exchange Ed25519 public keys during the TLS
  handshake, and `extract_iroh_client_fingerprint` already gets the
  `NodeId`. No client-auth wiring needed for iroh (direct or relay). The
  gap was quinn-only.

  **What's genuinely additive** (not blocking the ADR-029 migration):
  remote-identity verification (the client verifying the server's
  fingerprint against an expected value) is additive — the server-side
  fingerprint extraction is what matters for `PeerId`, not the client-side
  verification. The verifier for raw keys can start as "accept any, extract
  fingerprint" and add fingerprint-pinning later.

  See ADR-030 §6 for the fingerprint normalization details.
- **Cross-references**: ADR-014, ADR-017, ADR-027, ADR-029, ADR-030,
  [client-and-adapters.md](crates/call/client-and-adapters.md),
  [endpoint.md](crates/core/endpoint.md), [auth.md](crates/core/auth.md)

### OQ-30: PeerRef::Any Routing Policy

- **Origin**: [ADR-029](decisions/029-peer-graph-routing-model.md) §2, [client-and-adapters.md](crates/call/client-and-adapters.md), `docs/research/alknet-call-peer-routing/findings.md` §3.2
- **Status**: **resolved** (2026-06-27)
- **Door type**: Two-way
- **Priority**: low
- **Resolution**: `PeerRef::Any` uses **insertion-order first-match** —
  deterministic but order-dependent (worker A connects before worker B →
  `Any` routes to A until A disconnects). This is the simplest routing
  policy and is correct for the immediate use case (the head picks the
  first worker that serves the op). A richer `RoutingPolicy` (round-robin,
  least-loaded, affinity) is a feature extension — the `PeerRef` enum is
  designed to compose with a `Route { selector, policy }` struct without
  breaking the `invoke_peer` signature. Adding a routing policy is
  non-breaking; it's a feature addition when a fan-out use case needs it,
  not an unmade architectural decision.
- **Cross-references**: ADR-029, [client-and-adapters.md](crates/call/client-and-adapters.md)

### OQ-31: services/list-peers Re-Export Semantics

- **Origin**: [ADR-029](decisions/029-peer-graph-routing-model.md) §6, `docs/research/alknet-call-peer-routing/findings.md` §3.5
- **Status**: **resolved** (2026-06-27)
- **Door type**: Two-way
- **Priority**: low
- **Resolution**: `services/list` defaults to **"own ops only"** — it shows
  the head's own Layer 0 `External` ops, filtered by
  `AccessControl::check(calling_peer)`, unchanged from today (minus the
  retired `remote_safe` filter). A `services/list-peers` opt-in (new
  built-in operation) lists the peer overlays with attribution: each
  peer's sub-overlay listed as `{ peer: Option<PeerId>, operations: [...] }`,
  filtered by the calling peer's authorization. The re-export policy is an
  `AccessControl` decision on the listing op. Whether `services/list-peers`
  is built now or as a feature addition is a scheduling question — the
  decision (opt-in, `AccessControl`-filtered) is made.
- **Cross-references**: ADR-029, [client-and-adapters.md](crates/call/client-and-adapters.md)

### OQ-32: Multi-Hop Federation

- **Origin**: [ADR-029](decisions/029-peer-graph-routing-model.md) §3.7, `docs/research/alknet-call-peer-routing/findings.md` §3.7
- **Status**: open (feature extension, not an unmade architecture decision)
- **Door type**: One-way (federation model), two-way (mechanism)
- **Priority**: low
- **Resolution**: The model is **one-hop** — worker A does not transitively
  see worker B's ops through the head unless the head explicitly re-exports
  them. The peer-keyed overlay model extends to multi-hop without redesign
  (a chain of `PeerRef::Specific` routing decisions), but path-finding
  (which peer reaches which op transitively) is where a graph library
  (petgraph) would pay off. For one-hop (shallow), a nested
  `HashMap<PeerId, HashMap<String, ...>>` suffices. Multi-hop federation is
  a feature extension — the one-hop model is the architectural commitment;
  extending to multi-hop doesn't break downstream crates. Whether multi-hop
  becomes a real use case is a future decision; the peer-keyed model does
  not foreclose it.
- **Cross-references**: ADR-029, [client-and-adapters.md](crates/call/client-and-adapters.md)

### OQ-33: PeerId — Cryptographic Identity vs Stable Logical Identifier

- **Origin**: [ADR-029](decisions/029-peer-graph-routing-model.md) Assumption 1, `docs/research/alknet-call-peer-routing/findings.md` §6.1
- **Status**: **resolved** (2026-06-27 by ADR-030)
- **Door type**: One-way (composition semantics), two-way (id source)
- **Priority**: high
- **Resolution**: `PeerId` is a **logical identifier, decoupled from the
  cryptographic identity**. It is *not* the raw fingerprint or API-key
  prefix — those change on key rotation, which would break every
  in-flight `PeerRef::Specific` and every ACL entry referencing that peer.

  ADR-029 established the one-way door (`PeerId` is logical, not crypto)
  with a v1 UUID source as a no-storage workaround. **ADR-030 supersedes
  the UUID source**: `Identity.id` becomes `PeerEntry.peer_id` (stable
  across key rotation) on the fingerprint path, and `PeerId =
  Identity.id` from `IdentityProvider` resolution. The UUID workaround is
  removed — the stable logical id is the real thing, sourced from the auth
  system, not an ephemeral connection-assigned value.

  The `PeerEntry` config model (`peer_id`, `fingerprint`, `scopes`,
  `resources`, `display_name`, `enabled`) lives in `AuthPolicy`. Key
  rotation is a single `PeerEntry.fingerprint` update — the `peer_id`,
  ACL entries, and `PeerRef::Specific` references stay stable. The
  no-DB posture is preserved (core has the trait + the in-memory
  `ConfigIdentityProvider` adapter; persistence adapters are additive
  separate crates, ADR-033).

  **The one-way door (preserved from ADR-029):** `PeerId` is a logical id,
  not `Identity.id` (the fingerprint). This determines the
  `PeerCompositeEnv` key type, the `PeerRef::Specific` payload type, and
  the `ScopedPeerEnv.peer_pinned` entry shape. The *source* of the logical
  id (ADR-029's UUID → ADR-030's `PeerEntry.peer_id`) was the two-way-door
  remainder; it is now resolved.
- **Cross-references**: ADR-009, ADR-014, ADR-015, ADR-017, ADR-021, ADR-027,
  ADR-029, ADR-030, OQ-34, OQ-35, [client-and-adapters.md](crates/call/client-and-adapters.md),
  [operation-registry.md](crates/call/operation-registry.md),
  [auth.md](crates/core/auth.md)

### OQ-34: Persistent Peer Registry (Cross-Node State Storage)

- **Origin**: OQ-33 (the storage dimension it surfaced), the no-DB posture of ADR-008/018/025
- **Status**: **resolved** (2026-06-27 by ADR-030 + ADR-031 + ADR-033)
- **Door type**: One-way (storage boundary), two-way (backend choice)
- **Priority**: ~~medium (not a v1 blocker)~~ → resolved
- **Resolution**: The storage boundary is: **core defines repo traits +
  in-memory default adapters; persistence adapters are separate crates;
  the assembly layer wires the adapter.** This is the repo/adapter
  pattern (ADR-033), already established by `IdentityProvider` (ADR-004)
  and now extended to `CredentialStore` (ADR-031).

  - `IdentityProvider` (ADR-004) — the auth repo trait, in core.
    `ConfigIdentityProvider` is the in-memory default, backed by
    `AuthPolicy.peers` (ADR-030). A future `alknet-peer-store-sqlite`
    adapter that persists `PeerEntry` records in a `peers` table is
    additive — it implements the same trait.
  - `CredentialStore` (ADR-031) — the credential repo trait, in core.
    `InMemoryCredentialStore` is the in-memory default. A future
    persistence adapter is additive.

  The no-DB posture of the core crates is preserved in the sense that
  matters: core has **no backend dependency** (no SQLite, no honker). The
  in-memory default adapters carry no persistence. The persistence
  adapters are additive crates, built when a concrete use case forces
  them, wired by the assembly layer.

  The concrete adapter shapes (table schemas, backend choice, indexing,
  caching) were the two-way-door remainder, tracked as OQ-36 — **now
  resolved by [ADR-035](decisions/035-concrete-persistence-adapter-shapes.md)**
  (read/write split, honker+SQLite, `alknet-store-sqlite` crate). The
  trait shapes are the one-way door, committed by ADR-030, ADR-031, and
  ADR-033; ADR-035 builds on them.
- **Cross-references**: ADR-008, ADR-018, ADR-021, ADR-025, ADR-029,
  ADR-030, ADR-031, ADR-033, ADR-035, OQ-33, OQ-36,
  [auth.md](crates/core/auth.md), [config.md](crates/core/config.md)

## Theme: Storage and Adapters

### OQ-35: ~~API Key Identity vs Peer Identity~~ (Dissolved)

- **Origin**: ADR-030 §"API keys" (the asymmetry between the two auth paths)
- **Status**: **dissolved** (2026-06-27 — the framing was wrong)
- **Door type**: ~~One-way~~
- **Priority**: ~~medium~~
- **Resolution**: **Dissolved.** The original framing ("the fingerprint
  path gets `PeerEntry` id-decoupling, the API-key path doesn't — the
  asymmetry is deliberate") was based on a false distinction between "peer
  bearer" and "auth bearer" tokens. The correct framing is the three
  credential types (Ed25519, X.509, bearer token) and whether the token
  needs a stable logical id across rotation:

  - `PeerEntry` supports multiple credential paths: `fingerprints: Vec<String>`
    (Ed25519 and/or X.509) + `auth_token_hash: Option<String>` (bearer
    token). All resolve to the same `peer_id`.
  - `ApiKeyEntry` is for bearer tokens that ARE the identity (rotation =
    new identity, no stable logical id needed).

  A bearer token that is one credential path among several for a stable
  peer goes in `PeerEntry.auth_token_hash`. A bearer token that IS the
  identity stays in `ApiKeyEntry`. The distinction is whether the token
  needs a stable logical id across rotation, not "peer bearer vs auth
  bearer." See ADR-030 §"Bearer tokens."
- **Cross-references**: ADR-030, [auth.md](crates/core/auth.md),
  [config.md](crates/core/config.md)

### OQ-36: Concrete Persistence Adapter Shapes

- **Origin**: ADR-033 §"What this does NOT do" (concrete adapter shapes not
  specified), the project's note that the repo pattern is a tool to reach
  for, not a one-size-fits-all mold
- **Status**: **resolved** (2026-06-28 by ADR-035)
- **Door type**: Two-way (adapter shapes are implementation details;
  the trait shapes are the one-way doors, already committed by ADR-030/031/033)
- **Priority**: medium → resolved
- **Resolution**: **[ADR-035](decisions/035-concrete-persistence-adapter-shapes.md)
  commits the concrete adapter shape.** The design is driven by two
  constraints: the hot-path read trait (`IdentityProvider::resolve_from_
  fingerprint`, `CredentialStore::get`) is **sync** (called in the
  accept loop, no `.await`), and auth changes must take effect **without
  a restart** (an early issue the project already fixed for
  `ConfigIdentityProvider` via `ArcSwap` config reload).

  The resolution:
  - **Read trait stays sync; persistence adapters cache in memory.** A
    SQLite-backed adapter serves sync reads from an in-memory index
    (`HashMap<fingerprint, PeerEntry>` / `HashMap<String, EncryptedData>`),
    loaded from SQLite at construction and refreshed on honker `NOTIFY`.
    Same `ArcSwap`-backed full-reload pattern as `ConfigIdentityProvider`,
    generalized from "config file is source of truth" to "SQLite is
    source of truth, honker signals when it changed."
  - **New async `IdentityStore` write trait** (`put_peer` / `update_peer`
    / `remove_peer`) extends `IdentityProvider` for peer mutations.
    `ConfigIdentityProvider` does NOT implement it (config reload is its
    write path); the SQLite adapter does. The read trait stays lean;
    the write surface is opt-in.
  - **`CredentialStore::put`/`delete` become async** (refines ADR-031's
    sync sketch — within the one-way door ADR-031 committed; `get` stays
    sync/cached). `InMemoryCredentialStore`'s write methods are
    async-with-no-awaits (signature change only).
  - **honker is the cache-invalidation mechanism** — a hard dependency of
    `alknet-store-sqlite`, NOT of `alknet-core`. honker's SQLite
    `NOTIFY`/`LISTEN` (single-digit-ms wake, no polling) is what makes
    the sync-read + cached-index + no-restart combination work. Without
    it, the adapter either polls (stale window) or requires restart
    (the bug already fixed). Not optional for the SQLite adapter.
  - **`alknet-store-sqlite`** — one crate, both adapters
    (`SqliteIdentityProvider: IdentityProvider + IdentityStore`,
    `SqliteCredentialStore: CredentialStore`), shared SQLite connection
    pool + honker LISTEN loop + bootstrap migrations. Splitting into
    two crates later is a two-way door (additive).
  - **Schema shape committed** (one row per `PeerEntry` with JSON
    columns for `fingerprints`/`scopes`/`resources`; one row per
    `EncryptedData` blob keyed by `provider`); exact DDL is an
    implementation-detail two-way door in the adapter crate.
  - **Shared `StoreError`** (`#[non_exhaustive]`, `thiserror::Error`)
    in alknet-core for both adapters.

  The keypal adapter-factory pattern is **intentionally not ported** to
  Rust (runtime column-mapping/type-coercion is a TS affordance; in
  Rust each adapter is a concrete type, cross-cutting concerns are a
  shared helper module). Two trait families (not one generic
  `Storage<T>`) preserved per ADR-033 §4. Redis / Postgres / on-chain
  adapters are **not needed for current scope** — the trait shapes
  make them possible; the adapter crates get built when a use case
  forces them.
- **Cross-references**: ADR-004, ADR-011, ADR-014, ADR-020, ADR-025,
  ADR-030, ADR-031, ADR-033, ADR-035, OQ-33, OQ-34,
  [auth.md](crates/core/auth.md), [config.md](crates/core/config.md)

## Theme: TLS Identity

### OQ-37: X.509 Outgoing-Only Case (Three Peer Roles)

- **Origin**: ADR-030 §"Bearer tokens" (the three credential types), the
  discussion that X.509 is fundamentally different from Ed25519
- **Status**: **resolved** (2026-06-28 by ADR-034)
- **Door type**: One-way (how X.509 server identity integrates with the
  peer model)
- **Priority**: medium → resolved
- **Resolution**: **The pre-ADR-034 framing conflated three distinct
  remote roles under "X.509 endpoint."** [ADR-034](decisions/034-outgoing-only-x509-and-three-peer-roles.md)
  names them and resolves the peer-model question:

  1. **Public X.509 endpoint** — a remote HTTPS / `alknet/call`-over-TLS
     server reachable by domain, authenticated by CA verification
     (`WebPkiServerVerifier`). The local node is a *client*; it
     authenticates by bearer token. **Not a `PeerEntry` on the client
     side** — it is not in the call-protocol peer graph (ADR-029), gets
     no `PeerId`, and is not addressable via `PeerRef::Specific`. Ops
     discovered via `from_call`/`from_openapi`/`from_mcp` land in the
     connection's Layer 2 overlay and are invoked through the
     connection handle.
  2. **Transport relay** — iroh's DERP-equivalent (`iroh-relay`).
     Infrastructure, not an alknet peer; no `PeerEntry` / `PeerId`.
     Inherited with the `iroh` feature; its identity is iroh's concern.
  3. **Hub / hosting node** — an alknet application peer (head/worker
     hub, git-hosting hub) that *also* exposes a public domain + X.509
     for browsers. A single `PeerEntry` with **mixed fingerprints**
     (`ed25519:...` + `SHA256:...`), already supported by ADR-030.
     Browsers connecting to it are *not* alknet peers — served by
     `alknet-http`, bearer-token auth, no `PeerId`.

  **The "make `PeerEntry` symmetric" instinct is rejected.** `PeerEntry`
  is for peers in the call-protocol peer graph; pure-client connections
  to public X.509 endpoints are not in that graph on the client side.
  The asymmetry reflects a real trust-model difference: known peers have
  stable logical identities (pin the fingerprint); public APIs don't
  (trust the CA, hold the connection handle directly).

  **Client-side verifier selection rule (extends OQ-29):** known peer
  (`PeerEntry` present) → fingerprint pin (Ed25519 `ed25519:<hex>` or
  X.509 `SHA256:<hex>`); unknown X.509 remote (`PeerEntry` absent) → CA
  verification. An unknown Ed25519 raw-key remote cannot be verified at
  all (no CA fallback) and fails closed — same model as iroh.

  **Downstream, not blocking, recorded so they don't get lost:**
  WebTransport relay-as-proxy (browser → proxy → P2P hub) is the
  remaining scope question tracked as OQ-38 (h3/WebTransport itself is
  now in scope, ADR-038); ADR-030 §6's fingerprint normalization already
  keeps the proxied path clean. On-chain / smart-contract peer
  discovery (relays syncing git repos via iroh gossip) is a *source* of
  `PeerEntry` records, fits the OQ-36 repo/adapter pattern
  (`alknet-peer-store-onchain` implementing `IdentityProvider`), and
  does not change the auth model.

  Not blocking the ADR-029 migration — the Ed25519 path is the primary
  use case and was already resolved; this ADR closes the X.509
  outgoing-only remainder.
- **Cross-references**: ADR-027, ADR-029, ADR-030, ADR-033, ADR-034,
  OQ-29, OQ-36, [client-and-adapters.md](crates/call/client-and-adapters.md),
  [endpoint.md](crates/core/endpoint.md), [auth.md](crates/core/auth.md)

## Theme: alknet-http

### OQ-38: WebTransport Standalone Relay Service Scope

- **Origin**: [ADR-034](decisions/034-outgoing-only-x509-and-three-peer-roles.md)
  §5, [webtransport.md](crates/http/webtransport.md)
- **Status**: open (scope, not deferral)
- **Door type**: One-way (crate boundary), two-way (mechanism)
- **Priority**: low
- **Resolution**: There are two distinct "WebTransport proxy" concepts
  that must not be conflated:

  1. **In-process ALPN-stream-proxy (resolved, in `alknet-http`).**
     The `h3` handler hands a WebTransport stream to another ALPN
     handler (`SshAdapter`, `GitAdapter`, etc.) as a `Connection`, so
     a browser with a WASM parser can reach any ALPN service via
     WebTransport. This is resolved by
     [ADR-040](decisions/040-webtransport-alpn-stream-proxy.md) and
     lives in `alknet-http`'s `h3` handler. Not this OQ.

  2. **Standalone relay service (this OQ).** A full relay — a fork of
     `iroh-relay` — that provides NAT traversal infrastructure with
     WebTransport-based proxy as a fallback alongside WebSocket. This
     is a separate service, not a mode of the `h3` handler: it
     terminates the browser's WebTransport connection and forwards
     encrypted traffic to a P2P hub's Ed25519 endpoint (so the hub need
     not expose its own public X.509 cert). ADR-034 §5 recorded it in
     the h3/WebTransport bucket; ADR-038 brought h3/WebTransport into
     scope (later superseded by [ADR-044](decisions/044-defer-webtransport-browsers-use-websocket.md),
     which deferred h3/WebTransport as a scope decision — the browser
     bidirectional path uses WebSocket); ADR-040 resolved the in-process
     proxy (now parked per ADR-044). This OQ is the remaining scope
     question: does the standalone relay live in a future `alknet-relay`
     crate (a fork of `iroh-relay` with WebTransport proxy fallback) or
     is it out of scope for the current alknet work?

  This is a genuine scope question, not a deferral. The relay use case
  is not yet concrete enough to commit the crate boundary — no
  deployment has asked for a standalone relay with WebTransport
  fallback yet, and the design (transport-only proxy, no auth-model
  change per ADR-034 §5) is clear but the home is not. The decision is
  made when the browser-to-P2P-peer relay use case becomes concrete;
  until then it is tracked here, not deferred with "v1/later" language.
  The relay does not change the auth model (bearer token +
  `PeerEntry.auth_token_hash`; relay is transport-only), so it does not
  block any other ADR.
- **Cross-references**: ADR-027, ADR-030, ADR-034, ADR-038 (superseded),
  ADR-040 (parked), ADR-044, [webtransport.md](crates/http/webtransport.md)

### OQ-39: `to_openapi` Published-Spec Versioning

- **Origin**: [ADR-017](decisions/017-call-protocol-client-and-adapter-contract.md)
  Consequences, [http-adapters.md](crates/http/http-adapters.md)
- **Status**: **resolved** (2026-06-30 by ADR-045)
- **Door type**: One-way (after first publication), two-way (before)
- **Priority**: medium → resolved
- **Resolution**: **[ADR-045](decisions/045-to-openapi-gateway-spec-versioning.md)
  commits the versioning scheme.** The gateway pattern (ADR-042)
  dissolved most of the original concern: the published doc describes
  **5 fixed gateway endpoints** (`/search`, `/schema`, `/call`,
  `/batch`, `/subscribe`), not the per-operation surface. Per-caller
  operation changes (add/remove/modify an operation, change an
  operation's schema) do **not** change the published doc — the
  operation set is discovered at runtime via `AccessControl`-filtered
  `/search`, not preloaded into the doc. So the version does not churn
  on every operation change (the original OQ-39 worry, framed under the
  pre-ADR-042 per-operation-paths model).

  What remains is narrow: how the published gateway doc signals its
  version. The decision:

  1. **`to_openapi` emits `info.version` as semver.** Standard OpenAPI
     field, standard semver interpretation — no alknet-specific
     detection mechanism.
  2. **The version tracks the gateway endpoint contract, not the
     operation set.** Major = breaking change to the gateway (endpoint
     removed/renamed, required request field added, response shape
     changed, error-mapping semantics changed per ADR-023); Minor =
     additive (new endpoint, new optional field); Patch = wording/docs.
     Per-caller operation changes do **not** bump the version.
  3. **Bump on change to the gateway shape, not on regeneration.**
     A restart that regenerates the same gateway shape yields the same
     version.
  4. **Consumers detect breaking changes via the major version.** A
     client compares `info.version`'s major component to the version it
     built against; a major bump signals "re-read the doc, something
     broke." Minor/patch are informational.
  5. **The additive traditional per-operation-paths projection
     (ADR-042 §5) versions independently** on its own schedule — its
     surface *does* change with the operation set, so its versioning is
     the per-operation churn the original OQ-39 framed. That projection
     is opt-in and out of scope for ADR-045; the gateway doc is the
     default published contract and the one ADR-045 governs.

  The original "version marker emitted so consumers can detect mapping
  changes" constraint (from ADR-017 Consequences) is satisfied by
  `info.version` semver. ADR-045 lifts the "published artifact is a
  contract" blind spot in ADR-009's framework (it classifies doors by
  reversal cost in the codebase, not by compatibility cost for external
  consumers) into its Context and honors the constraint without changing
  ADR-009's framework.
- **Cross-references**: ADR-009, ADR-017, ADR-023, ADR-036, ADR-042,
  ADR-045, [http-adapters.md](crates/http/http-adapters.md)

### OQ-40: reqwest Client Config and Connection Pooling

- **Origin**: [http-adapters.md](crates/http/http-adapters.md),
  [http-mcp.md](crates/http/http-mcp.md), the alknet-http Phase 0
  findings DH-7
- **Status**: resolved (2026-06-30)
- **Door type**: Two-way
- **Priority**: low
- **Resolution**: `alknet-http` owns a shared HTTP client constructed
  once and reused across all `from_openapi`/`from_mcp` forwarding
  handlers. The client carries connection pooling, keep-alive, TLS,
  and a retry stack. The config shape is:

  | Aspect | Decision |
  |--------|----------|
  | Shared client type | `reqwest_middleware::ClientWithMiddleware` (not a bare `reqwest::Client`) — required because both retry and Retry-After are middleware on the stack |
  | Middleware stack | `RetryTransientMiddleware` (from `reqwest-retry` — exponential backoff on transient failures: connection errors, 5xx) + inlined `RetryAfterMiddleware` (parses the `Retry-After` header on 429/503 and sleeps before the next request to that URL) |
  | `Retry-After` handler | Inlined from `melotic/reqwest-retry-after` (MIT, ~50 lines of real logic). The crate is complementary to `reqwest-retry`, not a replacement — `reqwest-retry`'s default strategy does not honor `Retry-After`, which is why the separate middleware exists. Inlining lets the unbounded `HashMap<Url, SystemTime>` storage in the upstream crate be bounded (the melotic version grows without limit over a long-running process). |
  | Pooling / keep-alive / TLS | `reqwest::ClientBuilder` defaults; system trust store for outbound HTTPS (standard calls to OpenAI, Anthropic, etc.) |
  | Hot-reload | Rebuild-and-swap the `ClientWithMiddleware` via `ArcSwap` (same pattern as `ConfigIdentityProvider`, ADR-035). A rebuild drops the connection pool / keep-alive state — acceptable, since a config change wanting a fresh pool is the case that triggers it. Retry policy is baked into the middleware at `ClientBuilder::build()` time; live policy mutation is not supported by `reqwest-retry` (no cheap per-policy update path exists). |
  | Credentials | Per-request from `OperationContext.capabilities` — see the one-way constraints below |

  The one-way constraints (settled before this OQ, restated unchanged):
  (1) `alknet-http` owns its HTTP client — no env-var-based client
  config, no shared global client; (2) credential injection happens
  per-request (from `OperationContext.capabilities`), not at client
  construction — the client is shared across all operations, the
  credentials are per-call; (3) TLS for outbound calls uses the
  system trust store by default (custom CA bundle + client certs are
  an optional config for self-hosted API gateways).

  **Downstream layering boundary (so the agent crate doesn't
  accidentally re-invent a client).** The agent crate's provider SSE
  normalization (replicating the solid part of aisdk's pattern — the
  Vercel-UI-message normalization that maps different providers' SSE
  to a common shape) sits *on top of* this `ClientWithMiddleware`: it
  consumes the `reqwest::Response` stream the forwarding handler
  produces and emits `call.responded` events. It does not replace the
  client or own transport/pooling/retry. `alknet-http` owns transport;
  the agent crate owns provider-specific SSE → Vercel-UI-message
  mapping. The aisdk `core/client.rs` reference for HTTP client
  construction is *not* carried forward — its env-var config and
  hand-rolled retry are the anti-patterns being discarded; the
  aisdk/`@alkdev/operations/src/from_openapi.ts` SSE *normalization*
  pattern is separate and stays referenced in the forwarding-handler
  section of [http-adapters.md](crates/http/http-adapters.md).

  No ADR — the decision is internal to `alknet-http`: the client type
  does not cross crate boundaries (`alknet-call` never sees reqwest),
  the library choice is reversible, and it does not touch the
  system's structure, constraints, or API surface across crates.
- **Cross-references**: ADR-014, ADR-017, ADR-035,
  [http-adapters.md](crates/http/http-adapters.md),
  [http-mcp.md](crates/http/http-mcp.md)

### OQ-41: Stream Operators Library

- **Origin**: [ADR-049](decisions/049-streaming-handler-for-subscriptions.md),
  [operation-registry.md](crates/call/operation-registry.md) §"OperationEnv"
- **Status**: open (feature extension — a library to build, not a decision
  to make before implementation)
- **Door type**: Two-way (additive utility library; no protocol or API-surface
  change)
- **Priority**: low
- **Resolution**: ADR-049 establishes that stream composition (filter, map,
  combine, window, dedupe) is a **handler-level concern**, not a protocol
  composition concern. `OperationEnv::invoke()` is request/response-only;
  stream manipulation happens at the handler level with stream operators on
  the `BoxStream<ResponseEnvelope>` the handler itself produces. The
  `@alkdev/pubsub` `operators.ts` is the prior art: 13 operators (`filter`,
  `map`, `take`, `batch`, `dedupe`, `window`, `chain`, `join`, `reduce`,
  `groupBy`, `flat`, `pipe`, `toArray`) that operate on `AsyncIterable<T>`,
  forked from graphql-yoga's subscription implementation.

  The Rust analogue — a stream-operators utility crate or module providing
  the same set of operators on `BoxStream<T>` / `impl Stream<Item = T>` — is
  a **feature extension**, not an unmade architectural decision. Handlers can
  produce streams today without it (`Box::pin(stream::iter(...))`,
  `async_stream::stream!`, `futures::stream` combinators all work); the
  operators library is a convenience that reduces boilerplate for handlers
  that transform streams (filter, batch, dedupe, window). No ADR is needed
  for the library itself — it's internal utility code that doesn't cross
  crate boundaries as a contract. An ADR would be warranted only if the
  operators become part of a public API surface (e.g., a handler-registration
  DSL that references operator names).

  This OQ exists so the operators library is tracked and findable, not left
  as inline hedging in the spec docs. It is not a deferral of a decision —
  the architectural decision (stream composition is handler-level, not
  protocol-level) is made in ADR-049. This tracks the *implementation* of
  the utility library, which is scheduling work, not architecture work.
- **Cross-references**: ADR-049,
  [operation-registry.md](crates/call/operation-registry.md) §"OperationEnv",
  `/workspace/@alkdev/pubsub/src/operators.ts` (TS prior art)