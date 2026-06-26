---
status: draft
last_updated: 2026-06-26
---

# alknet-call — Client and Adapters

The outbound half of the call protocol: opening connections, importing remote
operations, and the adapter contract that ties import-style adapters together.
This document covers what ADR-017 specced but the server-side implementation
(`call-protocol.md`, `operation-registry.md`) did not include — the `CallClient`
that *opens* a connection, the `from_call`/`from_jsonschema` adapters, and the
`OperationAdapter` trait. The server-side `CallAdapter` and `CallConnection`
dispatch loop are covered in `call-protocol.md`; this document covers the
client-side connection-establishment half and the adapter surface.

## What

This document specifies four components, all in `alknet-call`:

1. **`CallClient`** — opens an outbound `alknet/call` QUIC connection and
   produces a `CallConnection`. The dispatch loop is shared with the
   server-side `CallAdapter` (ADR-017 §1); `CallClient` is the
   connection-establishment + credential-handling half, not a parallel
   protocol implementation.
2. **`from_call`** — discovers operations on a remote call-protocol endpoint
   via `services/list` + `services/schema` (already implemented in
   `registry/discovery.rs`) and registers them in the connection's Layer 2
   overlay as `FromCall`-provenance leaves with forwarding handlers.
3. **`from_jsonschema`** — schema-only registration: produces
   `HandlerRegistration` bundles with no handler, for validation, discovery,
   and composition-graph construction without a runtime.
4. **`OperationAdapter` trait** — the async trait that `from_call`,
   `from_openapi`, `from_mcp`, and `from_jsonschema` all implement.

It also records two cross-cutting architectural mechanisms that the adapter
surface rests on:

- The **adapter location map** — which adapters live in `alknet-call` vs
  `alknet-http`, and why.
- The **no-env-vars invariant** — the architectural mechanism by which
  downstream consumers' `std::env::var` credential reads are made unreachable.

And one downstream pattern this completion unblocks:

- The **exchange-of-operations pattern** (runner / container service) — the
  canonical bilateral composition this client surface enables.

## Why

The server-side `CallAdapter` (accept path) and `CallConnection` (dispatch
loop) are implemented and tested. The client side is the #1 gap blocking every
downstream consumer: the runner pattern (a process that connects outward to a
hub and exposes local ops), the container-service rewrite, the bilateral
exchange, the NAPI projection, and the agent's cross-node tool dispatch all
require a `CallClient`. `from_call` is the #2 gap; the `OperationAdapter`
trait is the enabling gap for `alknet-http`'s `from_openapi`/`from_mcp`.

ADR-017 specced this surface. This document is the spec that operationally
fills the gap ADR-017 left to implementation: the `CallClient` API, the
`from_call`/`from_jsonschema` flows, the trait signature, the adapter
location, the credential invariant, and the bilateral pattern. The gap
analysis (`docs/research/alknet-call-completion/gap-analysis.md`) identified
four decisions (DC-1..4) needed before implementation; DC-1 is resolved by
ADR-028, and DC-2/3/4 are two-way-door defaults recorded here and tracked as
OQs (DC-2→OQ-27, DC-3→OQ-28, DC-4→OQ-26).

## Architecture

### CallClient

`CallClient` opens a QUIC connection to a remote node on ALPN `alknet/call`,
performs credential setup, and produces a `CallConnection`. The
`CallConnection` type is already implemented (`call-protocol.md` §"CallConnection")
— it wraps an established `Connection` and holds the Layer 2 imported-ops
overlay. `CallClient` is the producer on the outbound side; `CallAdapter`'s
accept path is the producer on the inbound side. Both produce the same
`CallConnection` and hand it to the same shared dispatch loop.

```rust
pub struct CallClient {
    /// The operation registry. The peer-scoped view is a dispatch-time read
    /// over this registry, not a copy (ADR-028 §5).
    registry: Arc<OperationRegistry>,
    identity_provider: Arc<dyn IdentityProvider>,
    /// Trusted-peer mode (ADR-028 §3): when true, the dispatch path exposes
    /// all External ops to the remote peer and `services/list` lists all
    /// External ops, ignoring the `remote_safe` marking. When false
    /// (default), only registrations with `remote_safe: true` dispatch, and
    /// `services/list` hides non-remote-safe ops (ADR-028 Assumption 2).
    trusted_peer: bool,
}

impl CallClient {
    /// Open a QUIC connection to `addr` on ALPN `alknet/call`, perform
    /// credential handshake, and return a CallConnection running the shared
    /// dispatch loop. Credentials come from capabilities (ADR-014), not env
    /// vars — see "No-Env-Vars Invariant" below.
    pub async fn connect(
        &self,
        addr: SocketAddr,
        credentials: CallCredentials,
    ) -> Result<CallConnection>;

    /// Trusted-peer mode: construct a CallClient that exposes all External
    /// ops from `registry` to the remote peer, ignoring the remote-safe
    /// marking. Explicit opt-in per ADR-028 §3.
    pub fn trusted_peer(
        registry: Arc<OperationRegistry>,
        identity_provider: Arc<dyn IdentityProvider>,
    ) -> Self;
}
```

The v1 mechanism is the `trusted_peer: bool` flag plus the `remote_safe: bool`
field on each `HandlerRegistration` (default `false` across all provenance,
ADR-028 §4). A richer per-peer filtering mechanism (per-peer allowlist,
capability-class tag) is the two-way-door remainder tracked as OQ-25; v1's
boolean limits exposure control to "remote-safe for any peer" vs "not," which
is acceptable for the runner/dispatch pattern (one remote peer per
`CallClient`).

The connection is symmetric after establishment (ADR-017 §2): both sides can
send and receive `call.requested`. Connection direction (who opened it) is
independent of call direction (who calls whom). The `CallClient` is therefore
both a caller and a callee — it dispatches incoming calls from the remote
peer against its peer-scoped registry view, and it initiates outgoing calls
through the `CallConnection::call()` / `subscribe()` / `abort()` API.

### Credential sources for connections

`CallClient::connect()` takes a `CallCredentials` bundle. Credentials come
from `Capabilities` (ADR-014), never from environment variables. The three
credential dimensions (ADR-017 §7):

```rust
pub struct CallCredentials {
    pub tls_identity: Option<TlsIdentity>,      // RFC 7250 raw key or X.509
    pub auth_token: Option<AuthToken>,           // call-protocol-level token
    pub remote_identity: Option<RemoteIdentity>, // expected fingerprint/cert
}
```

- **TLS identity** — the local node's Ed25519 raw key (RFC 7250) or X.509 cert,
  derived from the vault at startup (ADR-020, ADR-026, ADR-027).
- **Auth token** — an opaque call-protocol-level token, decrypted from the
  vault or derived from a shared secret.
- **Remote identity verification** — the expected fingerprint/cert of the
  remote node, stored as a capability.

These are populated by the assembly layer at `CallClient` construction time
from vault-derived `Capabilities`. The credential path is the no-env-vars
invariant (below). The concrete shapes of `TlsIdentity`, `AuthToken`, and
`RemoteIdentity` are implementation-detail two-way doors; the one-way
constraints are that they come from `Capabilities`, not env vars (ADR-014).

### from_call

`from_call` discovers the remote peer's `External` operations and registers
them in the connection's Layer 2 overlay as `FromCall`-provenance leaves with
forwarding handlers. The discovery mechanism (`services/list` +
`services/schema`) is already implemented in `registry/discovery.rs`;
`from_call` is the client-side consumer of that API.

```rust
pub struct FromCallConfig {
    /// Namespace prefix applied to imported operation names. Optional —
    /// default no prefix. Collision on import is an error (DC-3, OQ-28),
    /// not last-wins.
    pub namespace_prefix: Option<String>,
    /// Optional filter — import only operations whose names match. None
    /// imports all External ops discovered via services/list.
    pub operation_filter: Option<HashSet<String>>,
}

/// Discover the remote peer's External ops and construct HandlerRegistration
/// bundles with FromCall provenance and forwarding handlers. The caller
/// registers the bundles in the connection's overlay via
/// CallConnection::register_imported_all().
pub async fn from_call(
    connection: &CallConnection,
    config: FromCallConfig,
) -> Result<Vec<HandlerRegistration>, AdapterError>;
```

The flow (ADR-017 §3):

1. Call `services/list` on the remote → list of `External` operations.
2. Call `services/schema` for each → input/output JSON Schemas and declared
   `error_schemas` (ADR-023).
3. For each discovered op, construct a `HandlerRegistration`:
   - `spec` mirrors the remote op's name (with optional prefix), namespace,
     type, schemas, access control.
   - `handler` is a forwarding handler: sends `call.requested` through the
     `CallConnection`, awaits `call.responded` (or streams for subscriptions).
   - `provenance: FromCall`, `composition_authority: None`, `scoped_env: None`
     (leaf — ADR-022).
4. The caller registers the bundles via
   `CallConnection::register_imported_all()`.

**Re-import on reconnection** (DC-2, OQ-27): `from_call` runs automatically on
connection establishment. The overlay is per-connection (Layer 2, ADR-024), so
a stale overlay dies with the connection; re-import on reconnect is naturally
scoped to the new connection. This is the v1 default; explicit re-import via a
future `CallConnection::refresh()` is additive.

**Namespace collision** (DC-3, OQ-28): optional prefix, default no prefix,
collision = error. A node importing from two remotes that both expose
`/container/exec` without prefixes should fail loudly. The operator adds
prefixes when they know they're importing from multiple sources.

**Trust is transitive** (recorded in `operation-registry.md`): a
`from_call`-imported operation executes the remote node's code, not yours.
The scoped env (ADR-015) bounds *which* operations are reachable, not *what*
they do. `from_call` means "I trust the remote node as much as my own
handlers." The abort cascade (ADR-016) crosses the node boundary transparently
through the forwarding handler's `parent_request_id`.

### from_jsonschema

Schema-only registration: produces `HandlerRegistration` bundles with no
handler (`FromJsonSchema` provenance). Used for validation, discovery, and
composition-graph construction without a runtime — type-checking a composition
plan without executing it, building a UI of available operations without
standing up the transports, etc.

```rust
pub fn from_jsonschema(
    spec: OperationSpec,
    schema: serde_json::Value,
) -> HandlerRegistration;
```

Distinct from `from_call` (gap analysis DC-5, confirmed not a decision):

| | `from_jsonschema` | `from_call` |
|---|---|---|
| Schema source | Provided directly (caller fetches, passes in) | Discovered over wire (`services/list` + `services/schema`) |
| Handler at call time | None (schema-only, `FromJsonSchema` provenance) | Forwards over QUIC (`FromCall` provenance, leaf) |
| Use case | Type validation, discovery, composition graph construction | Actually invoking remote operations |

Keeping them separate preserves the "schema-only, no execution" use case
(type checking, safe composition planning without runtime).

### OperationAdapter trait

The shared shape across import-style adapters. The trait lives in
`alknet-call` (where the types live); the implementations live where their
transport dependencies live (see "Adapter Location Map" below).

```rust
#[async_trait]
pub trait OperationAdapter: Send + Sync {
    async fn import(&self) -> Result<Vec<HandlerRegistration>, AdapterError>;
}
```

The trait is **async** because `from_call` requires async discovery
(`services/list` + `services/schema` over a QUIC connection). Sync adapters
(`from_openapi`, `from_mcp` reading a static spec) trivially satisfy an async
trait — their `import()` bodies contain no `.await` points. This is locked by
ADR-017 §5.

The **error type** (DC-4, OQ-26) is `Result<Vec<HandlerRegistration>,
AdapterError>` where `AdapterError` is a crate-level enum covering the
failure modes real implementations hit: discovery transport failure
(`from_call` remote unreachable), schema parse failure (`from_openapi`,
`from_jsonschema`), unauthorized (HTTP 401 for `from_openapi`,
`from_mcp`). The exact `AdapterError` variants are the two-way-door
remainder; the *presence* of an error type is filled in here. ADR-017 §5
showed `async fn import(&self) -> Vec<HandlerRegistration>` with no error
type; the spec omitted the error type as an implementation-detail two-way
door, recorded here.

Implementations:
- `FromCall` — QUIC-backed (in `alknet-call`).
- `FromJsonSchema` — pure parse, no transport (in `alknet-call`).
- `FromOpenAPI` — HTTP-backed (in `alknet-http`).
- `FromMCP` — MCP streamable-HTTP-backed (in `alknet-http`, feature-gated).

The `to_*` adapters (`to_openapi`, `to_mcp`) are outbound projections, not
`OperationAdapter` implementations — they consume the registry, they don't
produce entries for it (ADR-017 §5).

### Adapter Location Map

The decomposition principle: **the adapter trait lives where the types live
(`alknet-call`); the adapter implementations live where their transport
dependencies live.**

```
alknet-call (lean — no HTTP client, no HTTP server)
├── OperationAdapter trait          (the contract — async, per ADR-017 §5)
├── from_call                     (QUIC — discovers remote ops via call protocol)
├── from_jsonschema               (pure parse — caller fetches the doc, passes it in)
└── CallClient                    (outbound connection opener — the #1 gap)

alknet-http (owns HTTP server + HTTP client — separate crate, separate Phase 0)
├── ProtocolHandler for h2/http1.1/h3   (axum server — inbound HTTP)
├── from_openapi                   (parse OpenAPI doc + reqwest forwarding handler)
├── to_openapi                     (generate OpenAPI doc from local registry)
├── from_mcp  (feature-gated)       (import remote MCP tools over streamable HTTP — reqwest)
└── to_mcp    (feature-gated)       (expose local ops as MCP tools over streamable HTTP — axum)

Not built: MCP stdio transport
  — stdio = spawn arbitrary executable = built-in RCE ("download untrusted MCP servers")
  — streamable HTTP is the only supported MCP transport in alknet
  — recorded as an explicit security position, not a feature gap
```

`alknet-call` never sees the HTTP client. The `from_openapi`/`from_mcp`
forwarding handlers are opaque `Arc<dyn Handler>` from the registry's
perspective — constructed by `alknet_http::from_openapi()` at registration
time, stored in `HandlerRegistration`, dispatched by the `CallAdapter` which
doesn't know reqwest is involved. `alknet-call` stays lean (no reqwest, no
axum); `alknet-http` owns both HTTP directions.

**ADR-003 dependency note**: `alknet-http` implementing `from_openapi`/
`from_mcp` means `alknet-http` depends on `alknet-call` (for `OperationSpec`,
`Handler`, `HandlerRegistration`, `OperationAdapter`). ADR-003's rule is "no
handler crate depends on another handler crate" — but `alknet-call` is both
a handler *and* the protocol foundation that `alknet-agent` and `alknet-napi`
already consume. `alknet-http` depending on `alknet-call` is "HTTP uses the
call protocol types," not "HTTP depends on SSH." This is within the spirit of
ADR-003 (`alknet-call` is protocol-foundation, not a peer handler). The
`alknet-http` spec should note this explicitly; a one-line amendment to
ADR-003 clarifying that `alknet-call` is a protocol-foundation crate is
deferred to the `alknet-http` Phase 0.

### No-Env-Vars Invariant

The architectural mechanism for the env-var problem in downstream consumers
(the Rust port of Vercel's AI SDK at `/workspace/aisdk/`, whose providers all
read `std::env::var("OPENAI_API_KEY")` in their `Default` impls). The fix is
**not** to modify those consumers — it's that the env-var path is never taken
because the assembly layer never calls `Default::default()`.

The credential injection path:

```
vault (seed)
  → assembly layer (derive + decrypt at startup, per ADR-014/019/025)
    → Capabilities (non-serializable, zeroized, immutable — ADR-014)
      → HandlerRegistration.capabilities (ADR-022, the registration bundle)
        → OperationContext.capabilities (per-request, populated by dispatch
          path from the bundle — ADR-022 §6)
          → from_openapi handler reads context.capabilities.get("openai")
            → injects into HTTP Authorization header
              → reqwest request goes out with vault-derived credential
```

The `from_openapi`/`from_mcp` forwarding handlers (in `alknet-http`) are the
credential injection point. They read from `context.capabilities`, not from
`std::env::var`. The downstream consumers' `Default` impls reading env vars
are simply never called — the assembly layer constructs providers with
vault-derived credentials through the builder API, or the provider's HTTP
calls are routed through `from_openapi` operations that carry the credential
in `Capabilities`.

**This is a spec-level invariant in `alknet-call`, not a runtime convention.**
The dispatch path (`build_root_context` and `OperationEnv::invoke()` per
ADR-022 §6) populates `OperationContext.capabilities` from the registration
bundle. The invariant is: *no handler reads outbound credentials from any
source other than `OperationContext.capabilities`.* This is already the
architectural intent of ADR-014; this document records it as an explicit
invariant that the `from_openapi`/`from_mcp` handler implementations (in
`alknet-http`) are verified against.

### Exchange-of-Operations Pattern (Runner / Container Service)

The canonical downstream pattern this completion unblocks, recorded here so
Phase 1 specs can reference it. Concrete example: the container service at
`/workspace/@alkdev/dispatch` (axum + russh SSH client for "reverse git
runner" over Docker/vast.ai) gets rewritten as a call-protocol service.

**Bilateral exchange**:

```
Container service (runs on a vast.ai/docker instance):
  Defines Local ops: /container/exec, /container/list, /container/logs...
  (real handlers — calls bollard or vast.ai API)
  Connects to hub as a CallClient (outbound connection — runner pattern)

Hub (central server):
  Runs CallAdapter (server) on alknet/call (already implemented)
  When the container service connects:
    hub runs from_call → discovers /container/* via services/list + services/schema
    registers them as FromCall provenance (leaf, forwarding handlers) in the
    connection's Layer 2 overlay (ADR-024)
  Now the hub (or anything connected to the hub) can call /container/exec
  The from_call handler forwards over the connection back to the container service

Bilateral: the container service ALSO runs from_call against the hub,
  discovers the hub's External ops, and can call them.
  Connection direction (container → hub) is independent of call direction
  (both can call each other) per ADR-017 §2.
```

**What this requires**:
1. `CallClient` — the container service uses it to open the outbound
   connection to the hub. The #1 gap.
2. `from_call` — both sides run it to populate their Layer 2 overlays with
   the other side's `External` ops. The #2 gap.
3. `OperationAdapter` trait — `from_call` implements it. The #3 gap (enabling,
   not blocking — `from_call` can be built as a free function before the trait
   exists, but the trait is needed for `alknet-http`'s adapters).

**Why the container service doesn't need alknet-ssh**: under the call
protocol, the container service is a `CallClient` that dials the hub's
`alknet/call` ALPN directly over QUIC — no SSH in the loop. SSH port
forwarding becomes the *transitional* mechanism for targets that can't run a
call-protocol client (the `alknet-ssh` phase-0 findings document this
transition). Once the container service runs a `CallClient`, SSH is out of
the path entirely.

This is the "dev runner" pattern: a call-protocol client that connects back
to a hub and exposes core dev tools (bash, fs, etc.) as operations. The agent
service (`alknet-agent`, downstream) is the consumer that orchestrates these
via `env.invoke()`.

## Implementation Priority Order

Based on the gap analysis and the downstream unblock chain:

1. **`CallClient`** (critical) — outbound connection opener. Without it, no
   runner, no container service, no bilateral exchange. Reuses the existing
   `CallConnection` for the dispatch loop; adds only the
   connection-establishment + credential-handling half. The single
   highest-value piece of work in the entire `alknet-call` completion.

2. **`from_call`** (critical, depends on `CallClient`) — consumes the
   already-implemented `services/list` + `services/schema` discovery API.

3. **`OperationAdapter` trait** (enabling) — the async trait. Small,
   standalone, unblocks `alknet-http` Phase 1.

4. **`from_jsonschema`** (medium, standalone) — schema-only registration, no
   handler. Small.

5. **DC-1 resolution** (peer-scoped registry filtering, ADR-028) — the
   security dimension of `CallClient`'s registry. Addressed in parallel with
   #1 — it's a filtering layer on the registry the `CallClient` exposes, not
   a blocker for the connection-establishment work.

## What This Completion Unblocks

| Downstream crate | What it needs from alknet-call | Status without completion |
|-------------------|-------------------------------|--------------------------|
| alknet-http | `OperationAdapter` trait (to implement `from_openapi`/`from_mcp`) | Blocked — can't define HTTP-backed adapters without the trait |
| alknet-ssh | Stable alknet-call types (no adapter dependency) | Not blocked — ssh depends on alknet-core, not alknet-call's adapters. Proceeds in parallel. |
| alknet-agent | `CallClient` (tool dispatch), `from_call` (remote tool import), `OperationAdapter` (provider adapters) | Blocked on `CallClient` + `from_call` |
| Container service (dispatch rewrite) | `CallClient` + `from_call` | Blocked — this is the primary consumer |
| Runner pattern (dev runner, opencode runner) | `CallClient` + `from_call` | Blocked — the runner IS a `CallClient` |
| alknet-napi | `CallClient` (Node.js calls remote ops) | Blocked — NAPI projects `CallClient` to JS |

## Constraints

- **No HTTP in alknet-call.** `from_openapi`/`from_mcp`/`to_openapi`/`to_mcp`
  live in `alknet-http`. The `OperationAdapter` trait and the QUIC-backed
  adapters (`from_call`, `from_jsonschema`) live in `alknet-call`. See
  Adapter Location Map.
- **No secret material on the wire.** `CallCredentials` carries vault-derived
  material for the *outbound* connection (TLS identity, auth token); the
  call protocol's wire format carries no private keys, API keys, or decrypted
  credentials (ADR-014). The no-env-vars invariant (above) is the dispatch-side
  corollary.
- **Peer-scoped registry is default-deny.** A `CallClient` exposes no
  operations to the remote peer unless marked remote-safe. Trusted-peer
  opt-in is explicit (ADR-028).
- **`from_call` re-import is auto-on-reconnect.** v1 default; the overlay is
  per-connection so re-import is naturally scoped (DC-2, OQ-27).
- **`from_call` namespace collision is an error.** Default no prefix; the
  operator adds prefixes when importing from multiple sources (DC-3, OQ-28).
- **`OperationAdapter::import()` returns `Result`.** Failures surface as
  `AdapterError` (DC-4, OQ-26).
- **MCP stdio transport is not built.** Streamable HTTP is the only supported
  MCP transport in alknet. stdio = spawn arbitrary executable = built-in RCE.
  Recorded as an explicit security position, not a feature gap.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| Call protocol client and adapter contract | [ADR-017](../../decisions/017-call-protocol-client-and-adapter-contract.md) | `CallClient` opens connections; `from_call` imports remote ops; connection direction independent of call direction; trait is async; adapters produce `HandlerRegistration` bundles |
| Peer-scoped registry filtering (DC-1) | [ADR-028](../../decisions/028-callclient-peer-scoped-registry-filtering.md) | Default-deny; `remote_safe: bool` on `HandlerRegistration`; trusted-peer opt-in; one-way door on the security dimension |
| Secret material flow and capability injection | [ADR-014](../../decisions/014-secret-material-flow-and-capability-injection.md) | The no-env-vars invariant's foundation; capabilities injected at assembly layer |
| Handler registration, provenance, and composition authority | [ADR-022](../../decisions/022-handler-registration-provenance-and-composition-authority.md) | The registration bundle adapters produce; `composition_authority: None` for leaves |
| Operation registry layering | [ADR-024](../../decisions/024-operation-registry-layering.md) | Layer 2 per-connection overlay where `from_call` imports land |
| Privilege model and authority context | [ADR-015](../../decisions/015-privilege-model-and-authority-context.md) | Adapter-registered ops are `Internal` by default; default-deny posture |
| Abort cascade for nested calls | [ADR-016](../../decisions/016-abort-cascade-for-nested-calls.md) | Cross-node abort through `from_call` forwarding handler's `parent_request_id` |
| Operation error schemas | [ADR-023](../../decisions/023-operation-error-schemas.md) | `error_schemas` mirrored by `from_call` from remote op's spec |
| TLS identity redesign | [ADR-027](../../decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md) | RFC 7250 raw key / X.509 cert dimensions of `CallCredentials` |
| HD derivation for encryption keys | [ADR-020](../../decisions/020-hd-derivation-for-encryption-keys.md) | Vault-derived TLS identity material |
| Vault key model | [ADR-026](../../decisions/026-vault-key-model-hd-derivation.md) | Vault-derived TLS identity material |
| Vault local-only dispatch | [ADR-025](../../decisions/025-vault-local-only-dispatch.md) | Vault access at assembly layer only; the credential injection path's first hop |
| Crate decomposition | [ADR-003](../../decisions/003-crate-decomposition.md) | `alknet-http` depends on `alknet-call` (protocol-foundation exception, noted in Adapter Location Map) |
| One-way door decision framework | [ADR-009](../../decisions/009-one-way-door-decision-framework.md) | Door-type classification for DC-1..4 |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-25** (open, two-way): Remote-safe marking shape — `remote_safe: bool`
  v1 vs per-peer allowlist vs capability-class tag. The *existence* of
  filtering is locked by ADR-028; the shape is the two-way-door remainder.
- **OQ-26** (open, two-way): `AdapterError` enum variants (DC-4). The
  *presence* of an error type is recorded here; the variants are
  implementation-detail.
- **OQ-27** (open, two-way): `from_call` re-import trigger — auto-on-reconnect
  (v1 default, recorded here) vs explicit `CallConnection::refresh()`. v1 is
  auto-on-reconnect; the explicit path is additive.
- **OQ-28** (open, two-way): `from_call` namespace collision behavior — error
  on collision (v1 default, recorded here) vs last-wins.

## References

- ADR-017: Call Protocol Client and Adapter Contract (the spec this document
  operationally fills)
- ADR-028: Peer-Scoped Registry Filtering for CallClient Inbound Dispatch
  (resolves DC-1)
- `call-protocol.md` — `CallAdapter`, `CallConnection`, dispatch loop, stream
  model (the server-side complement to this document)
- `operation-registry.md` — `HandlerRegistration`, provenance, capability
  injection, service discovery (the discovery API `from_call` consumes)
- `docs/research/alknet-call-completion/gap-analysis.md` — DC-1..4, the
  implementation-state audit, the downstream unblock chain
- `/workspace/@alkdev/operations/` — TypeScript prior art (`from_openapi.ts`,
  `from_mcp.ts`, `from_schema.ts`, `scanner.ts`)
- `/workspace/@alkdev/dispatch/` — concrete downstream consumer (container
  service / "reverse git runner") this completion unblocks
- `/workspace/aisdk/` — downstream consumer (Rust port of Vercel AI SDK); the
  no-env-vars invariant makes its `std::env::var` reads unreachable
- `/workspace/rust-sdk/` — MCP Rust SDK (rmcp); streamable HTTP transport for
  `alknet-http`'s `from_mcp`/`to_mcp` (separate crate, separate Phase 0)
- `docs/research/alknet-ssh/phase-0-findings.md` — alknet-ssh Phase 0;
  confirms ssh depends on alknet-core not alknet-call's adapters, so it
  proceeds in parallel with this completion