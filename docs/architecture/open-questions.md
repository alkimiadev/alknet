---
status: draft
last_updated: 2026-06-19
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
- **Status**: open
- **Door type**: Two-way
- **Priority**: medium
- **Resolution**: When a handler resolves identity inside `handle()`, should the resolved `Identity` be stored somewhere for observability (e.g., connection logging), or is the handler's local variable sufficient? Options: (A) handlers return the resolved identity from `handle()`, (B) handlers call a method on Connection to set identity, (C) handlers log locally and the resolved identity stays local. Two-way door — can be decided during implementation.
- **Cross-references**: ADR-004, ADR-011

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
- **Status**: open
- **Door type**: One-way
- **Priority**: high
- **Resolution**: alknet-call currently specifies only the server side (CallAdapter receives connections and dispatches to the operation registry). A call protocol client is needed for: (1) alknet-napi to expose remote invocation to Node.js, (2) alknet-agent to dispatch tool calls (call, batch, search, schema) to remote nodes, (3) the `from_call` adapter pattern that creates operations whose handlers invoke remote services. The adapter contract (from_openapi, from_mcp, from_call, to_openapi, to_mcp) determines how external specifications and protocols compose with the operation registry. These traits belong in alknet-call because they define how operations are produced and consumed — the same contract that enables an agent to register call/batch/search/schema as tools also enables from_openapi to register HTTP-backed operations. The TypeScript `@alkdev/operations` library demonstrated these patterns; the Rust implementation defines the canonical traits (ADR-013). Two-way door for the specific trait signatures, one-way door for the architectural commitment that the adapter contract lives in alknet-call. ADR-014 constrains the adapter contract: adapters take credential sources from the assembly layer (wired to the vault), not static token strings — the `from_openapi` and `from_jsonschema` patterns receive credentials at registration time, not at call time.
- **Cross-references**: ADR-005, ADR-013, ADR-014, [call-protocol.md](crates/call/call-protocol.md), [operation-registry.md](crates/call/operation-registry.md)

### OQ-16: Safe Vault Operations for Call Protocol Exposure

- **Origin**: [operation-registry.md](crates/call/operation-registry.md), ADR-008
- **Status**: resolved
- **Door type**: One-way
- **Priority**: high
- **Resolution**: No vault operations are exposed over the call protocol for now. The vault is accessed only at the assembly layer (CLI binary at startup). Handlers receive secret material through `OperationContext.capabilities`, not by calling vault operations over the wire. The `operation-registry.md` spec previously showed `vault/derive`, `vault/unlock`, and `vault/decrypt` registered as call protocol operations — that was a contradiction with ADR-008's "capability source" model and has been corrected. If a future use case requires exposing a vault operation over the call protocol (e.g., a restricted `vault/public-key` operation that returns only public key material for identity verification), it would require its own ADR with an explicit threat model justification. See ADR-014.
- **Cross-references**: ADR-008, ADR-014, [operation-registry.md](crates/call/operation-registry.md)

### OQ-17: Abort Cascade Semantics for Nested Calls

- **Origin**: [call-protocol.md](crates/call/call-protocol.md), [operation-registry.md](crates/call/operation-registry.md)
- **Status**: open
- **Door type**: One-way (protocol schema), two-way (mechanism)
- **Priority**: high
- **Resolution**: When a handler composes other operations via `OperationEnv::invoke()`, it creates a call tree (parent → children via `parent_request_id`). When `call.aborted` arrives for a parent request, the protocol cascades the abort to all non-terminal descendants in the tree. The default policy is `abort-dependents`: aborting a request aborts everything downstream, regardless of branch. This is the correct default because aborted parent work has no consumer waiting for results — continuing is wasted work at best and unwanted side effects at worst (e.g., a `bash/exec` that keeps running after the caller stopped caring). An opt-in `continue-running` policy is available for cases where long-running work should survive a parent's abort (e.g., a subscription that should keep streaming).

  The one-way door is the protocol event schema: `call.aborted` must carry cascade semantics before implementation, because retrofitting cascade onto a non-cascading abort is a breaking protocol change (existing clients send `call.aborted` for one ID, the server processes one ID). The mechanism — how the runtime discovers descendants and propagates cancellation (cancellation tokens propagated through `OperationContext`, a parent-indexed map in `PendingRequestMap`, or a separate graph structure consuming call events) — is a two-way door for implementation. The `@alkdev/flowgraph` TypeScript package demonstrates a reactive call-graph approach (directed graph with `descendants()`, `FailurePolicy: "abort-dependents" | "continue-running"`, signal-based status propagation); a Rust adaptation could use `petgraph` for the graph structure or tokio `CancellationToken` for a simpler implicit tree. The flowgraph may live as a separate crate consuming call events (as the TS version does), not necessarily inside alknet-call.

  This is a protocol-level concern, not specific to any single consumer. The call protocol is a general-purpose cross-boundary RPC mechanism — every consumer (NAPI adapter, Python adapter, agent service, future services) inherits whatever abort model is locked in. Nested composition is a core protocol feature, not an agent feature. The agent use case makes the deep/dynamic call tree case concrete, but the abort cascade problem exists for any handler that composes other operations.

  This OQ will be resolved with an ADR before alknet-call implementation begins.
- **Cross-references**: ADR-012, [call-protocol.md](crates/call/call-protocol.md), [operation-registry.md](crates/call/operation-registry.md)

### OQ-18: Privilege Model and Authority Context

- **Origin**: [operation-registry.md](crates/call/operation-registry.md)
- **Status**: open
- **Door type**: One-way (ACL model), two-way (specific APIs)
- **Priority**: high
- **Resolution**: The `internal` flag on `OperationContext` marks calls that originated from composition (a handler calling another operation via `OperationEnv`), as opposed to external calls that arrived as `call.requested` from a wire client. The `internal` flag switches the authority context: the ACL check runs against the composing handler's identity (set at registration), not the caller's identity and not as a blanket skip. This replaces the previous `trusted` flag, which skipped ACL entirely — a privilege escalation vector.

  The model has two analogies: kernel/user mode (external operations are syscalls — curated entry points; internal operations are kernel functions — composition-only), and domain/integration events (external operations are integration events — cross-boundary; internal operations are domain events — within the bounded context).

  Three controls prevent privilege escalation through composition:
  1. **Operation visibility**: `OperationSpec` has a `Visibility` field (`External` — callable from the wire, or `Internal` — composition-only). When a `call.requested` arrives from a client, the registry checks visibility: an `Internal` operation returns `NOT_FOUND` (not `FORBIDDEN` — don't leak that it exists). `services/list` only returns `External` operations to remote callers.
  2. **Handler identity**: Each handler has its own `Identity` with scopes scoped to its composition needs, set at registration by the assembly layer. Internal calls use the handler's identity for ACL, not the caller's. The handler's identity has only the scopes it needs (least privilege), not blanket root and not the caller's scopes.
  3. **Scoped composition env**: The `OperationEnv` given to a handler can only invoke a declared set of operations. This bounds the parameterized-dispatch attack surface — a caller (or an LLM) picking which operation to invoke picks from the declared set, not from the entire registry.

  Two escalation vectors that this model addresses:
  - **Buggy handler**: a handler accidentally calls an operation it shouldn't. With handler identity + scoped env, the call either isn't reachable (scoped env) or fails ACL (handler identity lacks the scope). Under the old `trusted` model, ACL was skipped entirely.
  - **Parameterized dispatch**: a handler takes caller input that determines which internal operation to call. With scoped env, the handler can only reach declared operations. With handler identity, the ACL checks against the handler's scopes, not the caller's. The caller's scopes only gate entry to the external operation; they don't propagate into the composition.

  The one-way door is the ACL model (internal = authority context switch, not skip; visibility = External/Internal; handler identity + scoped env). The specific APIs — how handler identity is declared, how the scoped env trait works, how visibility interacts with `services/list` filtering — are two-way doors.

  This is a protocol-level concern. The call protocol is a general-purpose cross-boundary RPC mechanism — every consumer (NAPI adapter, Python adapter, agent service, future services speaking the EventEnvelope wire format) inherits whatever privilege model is locked in. The privilege boundary between external and internal calls, and the authority context switch for composition, are core protocol semantics, not features of any single consumer. The agent use case is a useful test case for thinking through the edge cases (parameterized dispatch via LLM tool selection makes the escalation vector concrete), but the decisions belong to the call protocol.

  This OQ will be resolved with an ADR before alknet-call implementation begins.
- **Cross-references**: ADR-014, [call-protocol.md](crates/call/call-protocol.md), [operation-registry.md](crates/call/operation-registry.md)