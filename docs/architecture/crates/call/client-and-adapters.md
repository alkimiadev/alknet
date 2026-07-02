---
status: draft
last_updated: 2026-07-02
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
four decisions (DC-1..4) needed before implementation. DC-1 was initially
resolved by ADR-028 (`remote_safe`/`trusted_peer`), but a subsequent research
pass (`docs/research/alknet-call-peer-routing/findings.md`) found that
ADR-028's model was structurally broken for the head→N-workers pattern (the
primary use case) and that its parallel `remote_safe`/`trusted_peer`
authorization system duplicated the existing `AccessControl`/`Identity`
machinery. **ADR-029 supersedes ADR-028**: peer-keyed overlays + `PeerRef`
routing, and peer authorization through the existing `AccessControl::check(peer_identity)`.
DC-2/3/4 are two-way-door defaults recorded here (DC-2→OQ-27, DC-3→OQ-28
cross-peer dissolved / same-peer stays, DC-4→OQ-26).

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
    registry: Arc<OperationRegistry>,
    identity_provider: Arc<dyn IdentityProvider>,
}

impl CallClient {
    pub fn new(registry: Arc<OperationRegistry>, idp: Arc<dyn IdentityProvider>) -> Self;

    /// Open a QUIC connection to `addr` on ALPN `alknet/call`, perform
    /// credential handshake, and return a CallConnection running the shared
    /// dispatch loop. Credentials come from capabilities (ADR-014), not env
    /// vars — see "No-Env-Vars Invariant" below. The dispatch loop runs on a
    /// spawned task; the returned `CallConnection` is live until the remote
    /// closes the connection or the caller drops it.
    pub async fn connect(
        &self,
        addr: SocketAddr,
        credentials: CallCredentials,
    ) -> Result<CallConnection, ClientError>;
}
```

Peer authorization flows through the existing `AccessControl::check` against
the peer's resolved `Identity` (ADR-029 §3) — there is no `trusted_peer` flag
and no `remote_safe` marking. When a remote peer calls an op, the dispatch
path resolves the peer's `Identity` (from the connection's TLS fingerprint or
the `auth_token` payload, via the existing `IdentityProvider`) and runs
`AccessControl::check(peer_identity)` against the op's `AccessControl`. If
the op's required scopes/resources are satisfied, the call dispatches; if not,
`FORBIDDEN` before the handler runs (capabilities never populated — the
security property). An op that should never be callable from the wire uses
`Visibility::Internal` (existing mechanism, `NOT_FOUND` before ACL). See
[ADR-029](../../decisions/029-peer-graph-routing-model.md) §3 for the full
mapping of the three `remote_safe` cases to `AccessControl`/`Visibility`.

The connection is symmetric after establishment (ADR-017 §2): both sides can
send and receive `call.requested`. Connection direction (who opened it) is
independent of call direction (who calls whom). The `CallClient` is therefore
both a caller and a callee — it dispatches incoming calls from the remote
peer through the same `AccessControl`-gated path, and it initiates outgoing
calls through the `CallConnection::call()` / `subscribe()` / `abort()` API.

#### Shared Dispatcher

The shared dispatch loop lives in `protocol/dispatch.rs` as the `Dispatcher`
struct. This is the architectural mechanism that keeps `CallClient` from
becoming a parallel protocol implementation (ADR-017 §1): both `CallAdapter`'s
accept path and `CallClient`'s connect path construct a `Dispatcher` and call
`run_loop` — the dispatch half is one implementation, the
connection-establishment half differs (accept vs dial).

```rust
/// Shared dispatcher for an established CallConnection. Constructed by both
/// CallAdapter (accept path) and CallClient (connect path). Holds no
/// per-connection state; the CallConnection is passed into run_loop.
pub struct Dispatcher {
    pub registry: Arc<OperationRegistry>,
    pub identity_provider: Arc<dyn IdentityProvider>,
    pub session_source: Option<Arc<dyn SessionOverlaySource + Send + Sync>>,
    pub default_timeout: Duration,
}
```

The dispatch path resolves the peer's `Identity`, runs `AccessControl::check`
against the op's `AccessControl`, and dispatches if allowed — the same
authorization machinery that gates every other call. No `RemoteFilter`, no
`remote_safe` gate (ADR-029 §3 retires these).

`CallClient::spawn_dispatch(connection)` is the lower-level API that takes a
pre-established `Connection`, constructs a `CallConnection`, builds a
`Dispatcher`, spawns the dispatch task, and returns the live `CallConnection`.
`connect()` uses it after the QUIC dial completes; tests use it to wire
mock/loopback connections directly.

#### Peer-keyed composition env (ADR-029)

The composition env that aggregates multiple connections is **peer-keyed**
(ADR-029 §1). `CompositeOperationEnv`'s singular
`connection: Option<Arc<dyn OperationEnv>>` is replaced by `PeerCompositeEnv`
with peer-keyed connections:

```rust
pub struct PeerCompositeEnv {
    pub base: Arc<dyn OperationEnv + Send + Sync>,       // Layer 0 curated
    pub session: Option<Arc<dyn OperationEnv + Send + Sync>>,  // Layer 1
    pub connections: HashMap<PeerId, Arc<dyn OperationEnv + Send + Sync>>,  // Layer 2, peer-keyed
    connection_order: Vec<PeerId>,  // insertion order for PeerRef::Any first-match
}
pub type PeerId = String;  // = Identity.id from IdentityProvider resolution
                           // = PeerEntry.peer_id (stable, not crypto material — ADR-030)
```

`OperationEnv` gains a peer-routing method with a `PeerRef` selector
(`Specific(PeerId)` / `Any`), default-impl for back-compat. See
[ADR-029](../../decisions/029-peer-graph-routing-model.md) §2 for the full
`invoke_peer` signature and `ScopedPeerEnv` peer-qualified reachability. The
per-`CallConnection` overlay stays flat (one connection = one peer); the
peer-keying is at the aggregation layer (the head node's composition env).

#### services/list

`services/list` filters by `AccessControl::check(calling_peer_identity)` —
the calling peer sees only ops it is authorized to call. The
`services_list_handler` / `services_list_handler_peer_scoped` split collapses
to a single `AccessControl`-filtered handler (the `peer_scoped` variant and
the `remote_safe` filter are removed). `services/list-peers` is the opt-in for
peer-attributed re-export listing (each peer's sub-overlay listed with
attribution, filtered by the calling peer's authorization). See
[ADR-029](../../decisions/029-peer-graph-routing-model.md) §6.

### Credential sources for connections

`CallClient::connect()` takes a `CallCredentials` bundle. Credentials come
from `Capabilities` (ADR-014), never from environment variables. The three
credential dimensions (ADR-017 §7):

```rust
pub struct CallCredentials {
    pub tls_identity: Option<TlsIdentity>,      // RFC 7250 raw key or X.509
    pub auth_token: Option<AuthToken>,           // call-protocol-level token
    pub remote_identity: Option<RemoteIdentity>, // expected fingerprint/cert (None = CA path, see below)
}

/// Expected identity of the remote node (ADR-017 §7, extended by
/// ADR-034 §2). Carries a fingerprint string the assembly layer
/// derives from `Capabilities` when the local node has a `PeerEntry`
/// for the remote (the known-peer case → fingerprint pin).
///
/// `remote_identity: None` is the **public X.509 endpoint** case: the
/// local node has no `PeerEntry` for the remote, so there is no
/// fingerprint to pin. Combined with an X.509 transport, `None`
/// selects CA verification (`WebPkiServerVerifier`) per the
/// verifier-selection rule in ADR-034 §3. Combined with an Ed25519
/// raw-key transport, `None` fails closed (raw-key remotes are always
/// known peers — no CA to fall back to).
///
/// The `Option` is therefore load-bearing, not cosmetic: `Some(fingerprint)`
/// means "pin this" (known peer), `None` means "trust the CA or fail"
/// (unknown remote). An implementer must not default `remote_identity`
/// to a placeholder value to "satisfy" the field — `None` is a real
/// state that drives verifier selection.
pub struct RemoteIdentity { pub fingerprint: String }
```

- **TLS identity** — the local node's Ed25519 raw key (RFC 7250) or X.509 cert,
  derived from the vault at startup (ADR-020, ADR-026, ADR-027).
- **Auth token** — an opaque call-protocol-level token, decrypted from the
  vault or derived from a shared secret.
- **Remote identity verification** — the expected fingerprint/cert of the
  remote node, stored as a capability. `Some` → fingerprint pin (known
  peer with a `PeerEntry`); `None` → CA verification for X.509 remotes,
  fail-closed for Ed25519 raw-key remotes (ADR-034 §2/§3). The `None`
  case is the public-X.509-endpoint path, not a missing field.

These are populated by the assembly layer at `CallClient` construction time
from vault-derived `Capabilities`. The credential path is the no-env-vars
invariant (below). The concrete shapes of `TlsIdentity`, `AuthToken`, and
`RemoteIdentity` are implementation-detail two-way doors; the one-way
constraints are that they come from `Capabilities`, not env vars (ADR-014).

**TLS client-auth presentation** (OQ-29 #1, wired): the client presents
its Ed25519 key as an RFC 7250 raw public key client cert — the client-side
equivalent of the server's `RawKeyCertResolver`. This is **wired now**, not
additive: it is what activates the `PeerEntry` fingerprint → `peer_id`
resolution path on quinn connections (ADR-030 §5). Without it, the ADR-029
peer graph doesn't populate for quinn connections — `PeerId` resolution
fails because the server has no client cert to extract a fingerprint from.
The iroh path already works (iroh uses RFC 7250 raw keys and exchanges
Ed25519 public keys during the TLS handshake automatically); the gap was
quinn-only, and OQ-29 #1 resolves it by replacing `with_no_client_auth()`
with presenting the key. The one-way constraint (credentials from
`Capabilities`, not env vars, ADR-014) is unaffected — the `auth_token`
dimension flows through the call-protocol `auth_token` payload field, not
TLS, so the no-env-vars invariant holds independently of the TLS layer.

**Remote-identity verification** (OQ-29 #2, additive): verifying the
server's fingerprint against an expected value (`credentials.remote_identity`)
is **additive** — the server-side fingerprint extraction is what matters for
`PeerId`, not the client-side verification. The verifier for raw keys can
start as "accept any, extract fingerprint" and add fingerprint-pinning later.
This is a two-way-door remainder; the one-way constraint (credentials from
`Capabilities`, not env vars) is unaffected.

**Server cert verifier selection** (OQ-29 #2 + ADR-034 §3): the client-side
`ServerCertVerifier` is selected by whether the local node has a `PeerEntry`
for the remote, not by key type alone. A pure-client
connection to a **public X.509 endpoint** (no `PeerEntry` on the local
side — e.g., dialing `api.alk.dev` or a third-party API) uses
`WebPkiServerVerifier` (CA verification), gets **no `PeerId`** on the
client side, and is **not added to `PeerCompositeEnv`** — it is not in
the call-protocol peer graph (ADR-029). Ops discovered via `from_call`
on such a connection land in the connection's Layer 2 overlay
(ADR-024) and are invoked through the `CallConnection` handle directly,
not via `PeerRef::Specific`. A connection to a **hub** (a `PeerEntry`
with mixed Ed25519 + X.509 fingerprints) uses fingerprint pinning on
both cert paths and does enter the peer graph. An unknown Ed25519
raw-key remote fails closed (no CA to fall back to — raw-key remotes
are always known peers). See
[ADR-034](../../decisions/034-outgoing-only-x509-and-three-peer-roles.md)
for the verifier selection rule and the three-role naming.

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
   - `handler` is a forwarding handler, **branched on `op_type`** (ADR-049):
     - `Query` / `Mutation` → a `Handler` (registered as `HandlerKind::Once`):
       sends `call.requested` via `CallConnection::call_with_payload()`, awaits
       the single `call.responded` (or `call.error`), returns the
       `ResponseEnvelope`.
     - `Subscription` → a `StreamingHandler` (registered as
       `HandlerKind::Stream`): calls `CallConnection::subscribe()`, which
       returns `impl Stream<Item = ResponseEnvelope>` (the client-side
       streaming path, already implemented), maps it to a
       `BoxStream<ResponseEnvelope>`. The remote stream flows end-to-end:
       each `call.responded` the remote sends becomes a stream item; the
       remote's `call.completed` ends the stream (→ wire `call.completed`);
       `call.aborted` drops the stream (cascade per ADR-016). No truncation,
       no first-value fallback — a `from_call`-imported subscription forwards
       the full remote stream.
   - `provenance: FromCall`, `composition_authority: None`, `scoped_env: None`
     (leaf — ADR-022).
4. The caller registers the bundles via
   `CallConnection::register_imported_all()`.

**Re-import on reconnection** (DC-2, OQ-27): `from_call` runs automatically on
connection establishment. The overlay is per-connection (Layer 2, ADR-024), so
a stale overlay dies with the connection; re-import on reconnect is naturally
scoped to the new connection. This is the v1 default; explicit re-import via a
future `CallConnection::refresh()` is additive.

**Namespace collision** (DC-3, OQ-28): under the peer-graph model (ADR-029),
cross-peer collision dissolves — same name on different peers is fine (they
live in separate peer sub-overlays, no prefix needed). Same-peer collision
stays an error (a peer shouldn't expose two ops with the same name).
`FromCallConfig::namespace_prefix` is optional local-naming sugar for when
the importing node wants to expose a peer's ops under a different name
*locally* — a local-naming concern, not a disambiguation concern. It defaults
to `None`.

**Trust is transitive** (recorded in `operation-registry.md`): a
`from_call`-imported operation executes the remote node's code, not yours.
The scoped env (ADR-015) bounds *which* operations are reachable, not *what*
they do. `from_call` means "I trust the remote node as much as my own
handlers." The abort cascade (ADR-016) crosses the node boundary transparently
through the forwarding handler's `parent_request_id`.

**Forwarded-for identity** (ADR-032): the `from_call` forwarding handler
populates `forwarded_for` on the `call.requested` payload it constructs to
send to the spoke. The hub reads its own `OperationContext.identity` (the
end user it authenticated) and sets `forwarded_for` to that identity when
forwarding. The spoke receives it as metadata on its `OperationContext` —
available for logging, auditing, per-user rate limiting, but never used by
`AccessControl::check` (the spoke authorizes the hub, its direct caller,
not the end user). The hub may set `forwarded_for: None` if it doesn't
want to disclose the originator. See [ADR-032](../../decisions/032-forwarded-for-identity.md).

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

5. **DC-1 resolution** (peer-graph routing model, ADR-029) — the
   peer-keyed overlay + `AccessControl`-based peer authorization model that
   replaces ADR-028's `remote_safe`/`trusted_peer`. This is a structural
   change to `CompositeOperationEnv` (→ `PeerCompositeEnv`), the dispatch
   path (retire `RemoteFilter`), and `OperationEnv` (gain `invoke_peer`).
   See ADR-029 for the migration; the POC shapes in the research doc are the
   reference.

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
- **Peer authorization via `AccessControl`.** A remote peer's call is
  authorized by `AccessControl::check(peer_identity)` against the op's
  `AccessControl` — the same mechanism that gates every other call. No
  `remote_safe` flag, no `trusted_peer` bypass (ADR-029 §3). An op with
  `AccessControl::default()` is callable by any peer; an op with
  `required_scopes` is callable only by peers whose `Identity.scopes` satisfy
  them; an op with `Visibility::Internal` is never callable from the wire.
- **Composition env is peer-keyed.** A head node with N worker connections
  holds a `PeerCompositeEnv` with `connections: HashMap<PeerId, Arc<dyn OperationEnv>>`,
  not a singular connection overlay. `invoke_peer()` routes to the right peer
  via `PeerRef::Specific` / `PeerRef::Any` (ADR-029 §1-2).
- **`from_call` re-import is auto-on-reconnect.** v1 default; the overlay is
  per-connection so re-import is naturally scoped (DC-2, OQ-27).
- **`from_call` namespace collision is same-peer only.** Cross-peer collision
  dissolves (same name on different peers is fine — separate sub-overlays,
  ADR-029 §5). Same-peer collision stays an error. `namespace_prefix` is
  optional local-naming sugar, not the disambiguation mechanism (DC-3, OQ-28).
- **`OperationAdapter::import()` returns `Result`.** Failures surface as
  `AdapterError` (DC-4, OQ-26).
- **MCP stdio transport is not built.** Streamable HTTP is the only supported
  MCP transport in alknet. stdio = spawn arbitrary executable = built-in RCE.
  Recorded as an explicit security position, not a feature gap.
- **Pure-client X.509 connections are not in the peer graph on the client
  side.** A `CallClient` connection to a public X.509 endpoint with no
  local `PeerEntry` for the remote gets no `PeerId`, is not added to
  `PeerCompositeEnv`, and is not addressable via `PeerRef::Specific`.
  Ops discovered on it live in the connection's Layer 2 overlay and are
  invoked through the `CallConnection` handle. The client-side
  `ServerCertVerifier` uses CA verification (`WebPkiServerVerifier`) for
  such remotes; known peers (hub with `PeerEntry`) use fingerprint
  pinning. See [ADR-034](../../decisions/034-outgoing-only-x509-and-three-peer-roles.md).
- **`CallCredentials.remote_identity: None` is load-bearing.** `None`
  means "no `PeerEntry` for this remote → use CA verification (X.509)
  or fail closed (Ed25519 raw key)" per the ADR-034 §3 verifier rule.
  The implementation must not default `remote_identity` to a placeholder
  to satisfy the field, and must not treat `None` as "skip verification"
  — `None` + X.509 is CA verification, `None` + raw key is a hard
  failure. `Some(fingerprint)` is the known-peer pin path.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| Call protocol client and adapter contract | [ADR-017](../../decisions/017-call-protocol-client-and-adapter-contract.md) | `CallClient` opens connections; `from_call` imports remote ops; connection direction independent of call direction; trait is async; adapters produce `HandlerRegistration` bundles |
| Peer-graph routing model (DC-1, supersedes ADR-028) | [ADR-029](../../decisions/029-peer-graph-routing-model.md) | Peer-keyed overlays + `PeerRef` routing; peer authorization via existing `AccessControl::check(peer_identity)`; retires `remote_safe`/`trusted_peer` |
| PeerEntry and Identity.id decoupling | [ADR-030](../../decisions/030-peerentry-and-identity-id-decoupling.md) | `PeerId` source changes from UUID to `Identity.id` (= `PeerEntry.peer_id`, stable across key rotation); `Identity.id` decoupled from crypto material on the fingerprint path |
| Forwarded-for identity | [ADR-032](../../decisions/032-forwarded-for-identity.md) | `forwarded_for` field on `call.requested` and `OperationContext`; the `from_call` handler populates it; metadata only, never used by `AccessControl::check` |
| Storage boundary and repo/adapter pattern | [ADR-033](../../decisions/033-storage-boundary-and-repo-adapter-pattern.md) | Core defines repo traits + in-memory defaults; persistence adapters are separate crates |
| ~~Peer-scoped registry filtering~~ (superseded) | ~~[ADR-028](../../decisions/028-callclient-peer-scoped-registry-filtering.md)~~ | ~~Default-deny; `remote_safe: bool`; trusted-peer opt-in~~ — superseded by ADR-029 (flat-namespace single-peer model couldn't express head→N-workers; parallel auth system duplicated existing `AccessControl`) |
| Secret material flow and capability injection | [ADR-014](../../decisions/014-secret-material-flow-and-capability-injection.md) | The no-env-vars invariant's foundation; capabilities injected at assembly layer |
| Handler registration, provenance, and composition authority | [ADR-022](../../decisions/022-handler-registration-provenance-and-composition-authority.md) | The registration bundle adapters produce; `composition_authority: None` for leaves |
| Operation registry layering | [ADR-024](../../decisions/024-operation-registry-layering.md) | Layer 2 per-connection overlay where `from_call` imports land |
| Privilege model and authority context | [ADR-015](../../decisions/015-privilege-model-and-authority-context.md) | Adapter-registered ops are `Internal` by default; default-deny posture |
| Abort cascade for nested calls | [ADR-016](../../decisions/016-abort-cascade-for-nested-calls.md) | Cross-node abort through `from_call` forwarding handler's `parent_request_id` |
| Operation error schemas | [ADR-023](../../decisions/023-operation-error-schemas.md) | `error_schemas` mirrored by `from_call` from remote op's spec |
| Streaming handler for subscriptions | [ADR-049](../../decisions/049-streaming-handler-for-subscriptions.md) | `from_call` `Subscription` ops register a `StreamingHandler` (`HandlerKind::Stream`) that calls `CallConnection::subscribe()` and forwards the remote stream; `Query`/`Mutation` stay `HandlerKind::Once` |
| TLS identity redesign | [ADR-027](../../decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md) | RFC 7250 raw key / X.509 cert dimensions of `CallCredentials` |
| Outgoing-only X.509 and three peer roles | [ADR-034](../../decisions/034-outgoing-only-x509-and-three-peer-roles.md) | Public X.509 endpoint is not a `PeerEntry` on the client side (no `PeerId`, not in peer graph); client-side verifier by `PeerEntry` presence (CA vs fingerprint pin); hub = mixed-fingerprint `PeerEntry` |
| HD derivation for encryption keys | [ADR-020](../../decisions/020-hd-derivation-for-encryption-keys.md) | Vault-derived TLS identity material |
| Vault key model | [ADR-026](../../decisions/026-vault-key-model-hd-derivation.md) | Vault-derived TLS identity material |
| Vault local-only dispatch | [ADR-025](../../decisions/025-vault-local-only-dispatch.md) | Vault access at assembly layer only; the credential injection path's first hop |
| Crate decomposition | [ADR-003](../../decisions/003-crate-decomposition.md) | `alknet-http` depends on `alknet-call` (protocol-foundation exception, noted in Adapter Location Map) |
| One-way door decision framework | [ADR-009](../../decisions/009-one-way-door-decision-framework.md) | Door-type classification for DC-1..4 |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-25** (dissolved by ADR-029): `remote_safe` marking shape — moot.
  `remote_safe`/`trusted_peer` are retired; peer authorization is
  `AccessControl::check(peer_identity)`. No marking to shape.
- **OQ-26** (resolved): `AdapterError` variants — `DiscoveryFailed`,
  `SchemaParse`, `Transport`, `Unauthorized`, `SamePeerCollision`
  (replaces flat `Conflict`). `#[non_exhaustive]`.
- **OQ-27** (resolved): `from_call` re-import trigger — auto-re-import on
  connection establishment. `CallConnection::refresh()` is a feature
  addition, not an unmade decision.
- **OQ-28** (resolved): `from_call` namespace collision — same-peer
  collision = error; cross-peer dissolved by ADR-029 (separate sub-overlays).
  `namespace_prefix` is optional local-naming sugar.
- **OQ-29** (resolved): `CallClient` TLS client-auth — wire quinn
  client-auth (present Ed25519 key as raw public key client cert);
  key-type-aware server cert verification (raw key = fingerprint match,
  X.509 = CA verification); fingerprint normalization (`ed25519:` across
  quinn/iroh). The iroh path already works; the gap was quinn-only.
  See OQ-29 in open-questions.md.
- **OQ-30** (resolved): `PeerRef::Any` routing policy — insertion-order
  first-match. A richer `RoutingPolicy` is a feature extension.
- **OQ-31** (resolved): `services/list-peers` — opt-in; `services/list`
  is "own ops only."
- **OQ-32** (open, feature extension): Multi-hop federation — the one-hop
  model is the architectural commitment; multi-hop is a feature extension
  that doesn't break downstream. The peer-keyed model extends to multi-hop
  without redesign; petgraph is the candidate if path-finding becomes real
  (ADR-029 §3.7).
- **OQ-33** (resolved by ADR-030): `PeerId` is a logical id. Source is
  `Identity.id` from `IdentityProvider` resolution (= `PeerEntry.peer_id`,
  stable across key rotation), not a connection-assigned UUID. The UUID
  workaround is removed. See OQ-33 in open-questions.md.
- **OQ-34** (resolved by ADR-030 + ADR-033): Persistent peer registry —
  the storage boundary is `core trait + in-memory default` (config-backed
  `ConfigIdentityProvider` now; persistence adapters additive in separate
  crates). See OQ-34 in open-questions.md.
- **OQ-35** (dissolved): the "API key asymmetry" framing was wrong;
  `PeerEntry` supports multiple credential paths (fingerprints +
  auth_token_hash), `ApiKeyEntry` is for tokens that ARE the identity.
  See OQ-35 in open-questions.md.
- **OQ-36** (resolved by ADR-035): Concrete persistence adapter shapes —
  read-sync / write-async split (`IdentityStore` async write trait
  extends the sync `IdentityProvider` read trait); SQLite adapter caches
  in memory and uses honker NOTIFY/LISTEN for no-restart cache
  invalidation; `alknet-store-sqlite` crate implements both
  `IdentityStore` and `CredentialStore`. See ADR-035 and OQ-36 in
  open-questions.md.
- **OQ-37** (resolved by ADR-034): X.509 outgoing-only case — three
  remote roles named (public X.509 endpoint, transport relay, hub).
  `PeerEntry` asymmetry is correct: a pure-client connection to a public
  X.509 endpoint is **not** in the call-protocol peer graph on the
  client side — no `PeerEntry`, no `PeerId`, no `PeerRef::Specific`
  routing. Ops discovered via `from_call`/`from_openapi`/`from_mcp`
  land in the connection's Layer 2 overlay and are invoked through the
  connection handle. The client-side `ServerCertVerifier` is selected
  by `PeerEntry` presence: known peer → fingerprint pin; unknown X.509
  remote → CA verification (`WebPkiServerVerifier`). See ADR-034 and
  OQ-37 in open-questions.md.

## References

- ADR-017: Call Protocol Client and Adapter Contract (the spec this document
  operationally fills)
- ADR-029: Peer-Graph Routing Model (supersedes ADR-028; resolves DC-1 with
  peer-keyed overlays + `AccessControl`-based peer authorization)
- ~~ADR-028~~: Peer-Scoped Registry Filtering (superseded by ADR-029)
- `call-protocol.md` — `CallAdapter`, `CallConnection`, dispatch loop, stream
  model (the server-side complement to this document)
- `operation-registry.md` — `HandlerRegistration`, provenance, capability
  injection, service discovery (the discovery API `from_call` consumes)
- `docs/research/alknet-call-completion/gap-analysis.md` — DC-1..4, the
  implementation-state audit, the downstream unblock chain
- `docs/research/alknet-call-peer-routing/findings.md` — the peer-graph
  routing research that identified ADR-028's structural gap and validated
  the ADR-029 design via POC
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