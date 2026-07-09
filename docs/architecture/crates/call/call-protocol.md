---
status: draft
last_updated: 2026-07-09
---

# Call Protocol

The wire protocol, stream model, framing, and adapter that alknet-call implements on ALPN `alknet/call`.

## What

The call protocol is a bidirectional, stream-agnostic RPC protocol that runs over QUIC bidirectional streams within a single `alknet/call` connection. It supports request/response calls, streaming subscriptions, batch operations, and service discovery — all using the same EventEnvelope wire format.

The `CallAdapter` implements `ProtocolHandler` for ALPN `alknet/call`. It receives a `Connection` from the endpoint, accepts bidirectional streams, and dispatches incoming `EventEnvelope` messages to the operation registry.

## Why

The call protocol is the primary programmatic interface to an alknet node. While SSH provides interactive shell access and HTTP provides REST APIs, the call protocol provides structured, discoverable RPC — the same interface that NAPI clients, MCP tools, and other automation consumers use.

The protocol must be:
- **Cross-language**: JSON wire format consumable from TypeScript, Python, any language
- **Bidirectional**: Both sides can initiate calls (server-to-client is as natural as client-to-server)
- **Stream-agnostic**: QUIC provides stream multiplexing; the protocol shouldn't impose additional constraints
- **Discoverable**: Clients can query what operations exist and their schemas

See ADR-064 for the decision that the call protocol uses hand-rolled
`EventEnvelope` framing (irpc was never integrated — ADR-005, which
accepted "irpc as the call protocol foundation," is superseded) and ADR-012
for the stream model decision.

## Architecture

### CallAdapter

The `CallAdapter` implements `ProtocolHandler`:

```rust
pub struct CallAdapter {
    /// Layer 0 — the curated operation registry. Immutable after startup.
    registry: Arc<OperationRegistry>,
    identity_provider: Arc<dyn IdentityProvider>,
    /// Layer 1 — optional session-overlay source (agent crate supplies this;
    /// None for non-agent deployments). See ADR-024, OQ-19.
    session_source: Option<Arc<dyn SessionOverlaySource + Send + Sync>>,
    /// Default timeout for wire calls (30s). Composed calls inherit the
    /// parent's remaining deadline via `OperationContext.deadline`.
    default_timeout: Duration,
}

impl CallAdapter {
    /// Non-agent deployment: no session overlay, default timeout.
    pub fn new(
        registry: Arc<OperationRegistry>,
        identity_provider: Arc<dyn IdentityProvider>,
    ) -> Self {
        Self { registry, identity_provider, session_source: None,
               default_timeout: Duration::from_secs(30) }
    }

    /// Agent deployment: supply a session-overlay source. The agent crate
    /// implements `SessionOverlaySource`; alknet-call defines the trait.
    pub fn with_session_source(mut self, source: Arc<dyn SessionOverlaySource + Send + Sync>) -> Self {
        self.session_source = Some(source);
        self
    }

    /// Override the default timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }
}

/// Session overlay integration point (ADR-024). Defined in alknet-call
/// because `CallAdapter` must name the type — alknet-call cannot depend on
/// alknet-agent (agent depends on call, not reverse). The agent crate
/// implements this trait; alknet-call defines it. This is the same pattern
/// as `IdentityProvider` (ADR-004: core defines the trait, handlers impl it).
///
/// The session overlay is an `OperationEnv` impl that wraps the curated base
/// (Layer 0). The `CallAdapter` composes it into the root
/// `OperationContext.env` per incoming call when a session is active. The
/// lookup mechanism (session ID in metadata, payload field, connection-bound
/// session state) belongs to the agent crate — this trait is the integration
/// point, not the lookup policy.
pub trait SessionOverlaySource: Send + Sync {
    /// Returns the session overlay env for the given call, if a session is
    /// active. `None` means no session is active for this call — the root
    /// env is `curated base + connection overlay` (no session layer).
    /// The agent crate determines how to map a call to its session.
    fn overlay_for(&self, context: &OperationContext) -> Option<Arc<dyn OperationEnv + Send + Sync>>;
}
```

The `CallAdapter` holds the static curated registry and an optional
session-overlay source. Per-connection imported-ops overlays (Layer 2,
ADR-024) are held with the connection and composed into the root
`OperationContext.env` per incoming call. The composition env is peer-keyed
(`PeerCompositeEnv`, ADR-029 §1) to handle head→N-workers routing — a head
node with multiple worker connections holds a peer-keyed
`HashMap<PeerId, connection_overlay>`, not one overlay. See ADR-024 for the
layering model, ADR-029 for the peer-keyed extension, and `compose_root_env`
below.

### CallConnection

A `CallConnection` represents an established `alknet/call` connection,
regardless of which side opened it (ADR-017). It holds the connection's
imported-ops overlay (Layer 2, ADR-024) — the set of `from_call`-imported
operations discovered when the connection was established.

```rust
/// An established alknet/call connection (either direction — accepted or
/// opened). Holds the connection's Layer 2 overlay (imported ops).
pub struct CallConnection {
    /// The underlying QUIC connection (from endpoint.accept or CallClient.connect).
    connection: Connection,
    /// Layer 2 — this connection's imported-ops overlay. Populated by
    /// `from_call` discovery when the connection is established. Each
    /// imported op is a `HandlerRegistration` with `provenance: FromCall`.
    /// This overlay is an `OperationEnv` impl that the `CallAdapter`
    /// composes into the root `OperationContext.env` per incoming call.
    imported_operations: Arc<RwLock<HashMap<String, HandlerRegistration>>>,
}

impl CallConnection {
    /// Register an imported operation into this connection's overlay
    /// (Layer 2, ADR-024). Called by `from_call` after discovery.
    pub fn register_imported(&self, registration: HandlerRegistration) {
        let name = registration.spec.name.clone();
        self.imported_operations.write().insert(name, registration);
    }

    /// Register multiple imported operations (bulk variant for `from_call`).
    pub fn register_imported_all(&self, registrations: Vec<HandlerRegistration>) {
        let mut overlay = self.imported_operations.write();
        for reg in registrations {
            overlay.insert(reg.spec.name.clone(), reg);
        }
    }

    /// Build an `OperationEnv` impl for this connection's overlay. Used by
    /// the `CallAdapter` when composing the root `OperationContext.env`.
    /// Returns an `OperationEnv` that dispatches to this connection's
    /// imported ops (and reports `contains` only for ops in the overlay).
    pub fn overlay_env(&self) -> Arc<dyn OperationEnv + Send + Sync>;

    /// Call an operation on the remote peer (sends `call.requested`).
    pub async fn call(&self, operation_id: &str, input: Value) -> ResponseEnvelope;

    /// Subscribe to a streaming operation on the remote peer.
    pub async fn subscribe(&self, operation_id: &str, input: Value) -> impl Stream<Item = ResponseEnvelope>;

    /// Abort an in-flight request (sends `call.aborted`, cascades per ADR-016).
    pub async fn abort(&self, request_id: &str);
}
```

**Layer 0 vs Layer 2 registration API** (ADR-024): `OperationRegistryBuilder`
builds Layer 0 (curated, immutable after startup) via `.with_local()` /
`.with_leaf()` / `.with()`. Layer 2 (per-connection) registration uses
`CallConnection::register_imported()` at runtime — the builder is
Layer-0-only; runtime overlay registration uses `CallConnection` methods.
When the connection drops, the overlay (and all imported ops) is dropped —
no explicit deregistration needed.

The adapter:
1. Accepts bidirectional streams on the connection
2. Reads length-prefixed JSON `EventEnvelope` frames from each stream
3. Resolves the peer's identity using `AuthContext` and `IdentityProvider`
4. Dispatches `call.requested` events to the operation registry
5. Writes response `EventEnvelope` frames back to the appropriate stream
6. Manages the `PendingRequestMap` for outgoing calls

The dispatch loop is **shared** with `CallClient` (ADR-017 §1): both
`CallAdapter::handle` (accept path) and `CallClient::connect` (connect path)
construct a `Dispatcher` (`protocol/dispatch.rs`) and call `run_loop` — the
dispatch half is one implementation, the connection-establishment half differs
(accept vs dial). Peer authorization flows through the existing
`AccessControl::check(peer_identity)` — no `RemoteFilter`/`remote_safe` gate
(ADR-029 §3). The composition env is peer-keyed (`PeerCompositeEnv`,
ADR-029 §1) to handle head→N-workers routing. See
[client-and-adapters.md](client-and-adapters.md) for the `Dispatcher` mechanism
and [ADR-029](../../decisions/029-peer-graph-routing-model.md) for the
peer-graph routing model.

### Stream Model

See ADR-012 for the full rationale.

The call protocol uses bidirectional QUIC streams with EventEnvelope framing. Key properties:

- **Either side can open streams**: The client opens a stream to call a server operation. The server opens a stream to call a client operation. Both use `open_bi()` and `accept_bi()`.
- **Correlation by request ID**: The `id` field in `EventEnvelope` correlates requests with responses. A response arriving on stream N can fulfill a request sent on stream M. The `PendingRequestMap` is keyed by ID, not by stream.
- **Stream usage is the client's choice**: A client may open one stream per operation, one stream for all operations, or any mix. The server processes EventEnvelopes regardless of stream origin.
- **One connection, full access**: A single `alknet/call` connection provides access to all operations (call, subscribe, batch, schema). No need for multiple connections or multiple ALPNs.

### Wire Format: EventEnvelope

Every message on the wire is a length-prefixed JSON `EventEnvelope`:

```rust
pub struct EventEnvelope {
    pub r#type: String,    // Event type
    pub id: String,        // Correlation key (request ID, subscription ID)
    pub payload: Value,    // serde_json::Value — schema depends on event type
}

// Frame: 4-byte big-endian length prefix + UTF-8 JSON body
```

The `Value` type is `serde_json::Value`. The envelope is JSON because it must be consumable from JavaScript, Python, and any language. The envelope itself stays JSON for cross-language compatibility.

Binary payloads (postcard, protobuf) are base64-encoded as a JSON string within the `payload` field. The convention is: if an operation's output schema specifies a binary field, the handler encodes it as a base64 string and the client decodes it. The `EventEnvelope` structure is not aware of this convention — it carries a `serde_json::Value` and does not interpret the payload. This is a handler-level concern, not a protocol-level concern.

This is hand-rolled length-prefixed JSON framing (ADR-064), coincidentally
the same shape irpc uses. The Rust implementation in alknet-call is
canonical — the `@alkdev/pubsub` TypeScript adapters serve as a reference
and browser adaptation, not a parallel implementation (see ADR-013).

### Event Types

Five event types carry request/response and subscription semantics:

| Event | Direction | Purpose |
|-------|-----------|---------|
| `call.requested` | Caller → Handler | Initiate a call or subscription |
| `call.responded` | Handler → Caller | Deliver a result (one for calls, many for subscriptions) |
| `call.completed` | Handler → Caller | Signal end of subscription stream |
| `call.aborted` | Either side | Cancel the call/subscription |
| `call.error` | Handler → Caller | Signal an error |

**A call is a subscribe that resolves after one event.** Both `call()` and `subscribe()` send the same `call.requested` event. The difference is consumption pattern:
- **call()**: Sends `call.requested`, resolves on first `call.responded`
- **subscribe()**: Sends `call.requested`, yields each `call.responded` until `call.completed` or `call.aborted`

The `id` field carries the `requestId` for correlation.

`call.completed` is sent only for subscriptions. A plain `call()` (request/response)
is complete after its single `call.responded`; no `call.completed` follows. The
`PendingRequestMap` entry for a `Call` is deleted on the first `call.responded`.

### `call.requested` Payload

The `payload` of a `call.requested` event has this shape:

```json
{
  "operationId": "/fs/readFile",
  "input": { ... },
  "auth_token": "alk_...",           // optional — see Identity Resolution below
  "forwarded_for": {                 // optional (ADR-032) — present when a hub forwards a call
    "id": "alice",
    "scopes": ["fs:read", "docker:start"],
    "resources": {}
  }
}
```

- `operationId` — the operation to invoke, **with a leading slash** on the wire (e.g., `/fs/readFile`, `/agent/chat`, `/services/list`). This is the display form of the operation name. The registry stores names without the leading slash (`fs/readFile` — see [operation-registry.md](operation-registry.md#operationspec)); the wire format adds it. The `CallAdapter` strips the leading slash before registry lookup.
- `input` — the operation input, matching the operation's `input_schema` (JSON Schema). Always a `serde_json::Value`.
- `auth_token` — optional. If present, the `CallAdapter` resolves it via `IdentityProvider::resolve_from_token()` and the resulting `Identity` takes precedence over the connection-level identity for this request. See [Identity Resolution](#authcontext-and-identity-resolution) below.
- `forwarded_for` — optional (ADR-032). Present when a `from_call` forwarding handler propagates the originator's identity to a spoke. Carries a serialized `Identity` (id, scopes, resources) — the end user the hub authenticated. **Metadata only** — `AccessControl::check` never reads it; the spoke authorizes the hub (its direct caller), not the end user. The hub may set `forwarded_for: None` if it doesn't want to disclose the originator. See [ADR-032](../../decisions/032-forwarded-for-identity.md).

The `call.requested` payload does **not** carry an abort policy field. The abort policy (`abort-dependents` vs `continue-running`, ADR-016) is set on `OperationContext` and propagated through `OperationEnv::invoke()` — the composing handler decides the child's policy, not the wire caller. See [Abort Cascade and Nested Calls](#abort-cascade-and-nested-calls) below.

**Leading-slash convention**: `operationId` on the wire always has a leading slash (`/fs/readFile`). `OperationSpec.name` in the registry and in `services/list` responses never has a leading slash (`fs/readFile`). `OperationSpec.path()` produces the wire form (`/fs/readFile`). This is a single rule applied consistently — do not mix the two forms.

### `call.error` Payload

```json
{
  "code": "FILE_NOT_FOUND",
  "message": "file not found: /etc/nonexistent",
  "retryable": false,
  "details": { "path": "/etc/nonexistent", "errno": 2 }
}
```

Error codes use an extensible string enum. The protocol defines the following **protocol-level codes** (emitted by the dispatch machinery, not by handlers):
- `NOT_FOUND` — operation not in registry (or Internal op called from wire)
- `FORBIDDEN` — access denied (insufficient scopes or unauthenticated)
- `INVALID_INPUT` — input doesn't match the operation's JSON Schema
- `INVALID_OPERATION_TYPE` — wrong dispatch path for the operation's type (`invoke()` called on a `Subscription`, or `invoke_streaming()` on a `Query`/`Mutation`, or `OperationEnv::invoke()` on a `Subscription` during composition — ADR-049)
- `INTERNAL` — handler error, panic, connection failure
- `TIMEOUT` — request timed out (retryable: true)

Operations may also declare **operation-level domain codes** in their `error_schemas` (ADR-023) — e.g., `FILE_NOT_FOUND`, `RATE_LIMITED`, `INSUFFICIENT_CREDITS`. These are emitted by handlers and carry a `details` payload conforming to the declared `ErrorDefinition.schema`. Protocol-level errors omit `details` or carry protocol-specific context (e.g., the operation name for `NOT_FOUND`).

Fields:
- `code` — the error code (protocol-level or operation-level)
- `message` — human-readable error message. For logging and debugging, not for programmatic handling. Clients should switch on `code`, not parse `message`.
- `retryable` — whether the caller should retry. `true` for transient failures, `false` for permanent ones.
- `details` — optional. When the code matches a declared `ErrorDefinition`, `details` conforms to that definition's schema. This is the typed error payload — it makes errors structured instead of string-matched. See ADR-023.

New error codes may be added in future versions. Clients should treat unknown error codes as `INTERNAL` with `retryable: false`.

### Wire Payload Schemas

The `payload` field of `EventEnvelope` has a different shape per event type:

| Event | `payload` shape |
|-------|----------------|
| `call.requested` | `{ "operationId": "/fs/readFile", "input": {...}, "auth_token": "alk_..." (optional), "forwarded_for": { "id": "...", "scopes": [...], "resources": {} } (optional, ADR-032) }` |
| `call.responded` | `{ "output": <Value> }` — the operation's output, matching `output_schema` |
| `call.completed` | `{}` — empty object (subscription stream end signal) |
| `call.aborted` | `{}` — empty object (cancellation signal; the `id` identifies which request) |
| `call.error` | `{ "code": "...", "message": "...", "retryable": bool, "details": {...} (optional) }` |

### `ResponseEnvelope` → `EventEnvelope` Conversion

Local dispatch produces `ResponseEnvelope { request_id, result: Result<Value, CallError> }`. The `CallAdapter` converts it to `EventEnvelope` for the wire:

| `ResponseEnvelope` | `EventEnvelope` |
|--------------------|-----------------|
| `Ok(value)` | `{ type: "call.responded", id: request_id, payload: { output: value } }` |
| `Err(call_error)` | `{ type: "call.error", id: request_id, payload: <serialized CallError> }` |

The `request_id` becomes the `id` field. For subscriptions, each `call.responded` is a separate `EventEnvelope` with the same `id`; `call.completed` is `{ type: "call.completed", id, payload: {} }`. The streaming dispatch path (`invoke_streaming()` → write each → write `call.completed`) produces these frames from a `StreamingHandler`'s stream; the single-response path (`invoke()` → write one) produces them from a `Handler`'s future. See ADR-049 and [operation-registry.md](operation-registry.md#handler).

### Protocol Operations

The call protocol defines four top-level operations, expressed through event types and operation names:

| Operation | Event Pattern | Description |
|-----------|--------------|-------------|
| **call** | `call.requested` → `call.responded` or `call.error` | Request/response — one result |
| **subscribe** | `call.requested` → many `call.responded` → `call.completed` or `call.aborted` | Streaming — zero or more results |
| **batch** | multiple `call.requested` (different IDs) → multiple `call.responded` | Multiple operations in one round |
| **schema** | `call.requested` name `services/list` or `services/schema` → `call.responded` | Discover available operations |

Batch is not a separate event type — it's multiple `call.requested` events with different request IDs. The client sends them (on one or many streams) and correlates the responses by ID. See OQ-14.

### Bidirectional Calls

Both sides of the connection can initiate calls. The server can call operations on the client just as the client calls operations on the server.

```
Client                                           Server
  │                                                │
  │── open_bi() → stream ─────────────────────────▶│
  │── call.requested { id: "c1", ... } ────────────▶│  (client calls server)
  │◀─ call.responded { id: "c1", ... } ───────────│
  │                                                │
  │◀─ open_bi() ← stream ──────────────────────────│
  │◀─ call.requested { id: "s1", ... } ────────────│  (server calls client)
  │── call.responded { id: "s1", ... } ───────────▶│
  │                                                │
```

The server calls client operations using the same `PendingRequestMap` and the same `EventEnvelope` format. The operation registry on the client side dispatches `call.requested` events just like the server side.

This enables patterns where the server pushes notifications, requests configuration from the client, or orchestrates workflows that require the client to perform operations.

### Streaming Subscribe Example: LLM Chat

The subscribe operation pattern maps naturally to LLM streaming. An agent handler exposing `/agent/chat` as a subscription receives a `call.requested` event and streams `call.responded` events back as the LLM generates tokens. The output payloads use a normalized streaming UI format (e.g., Vercel AI SDK UI chunks — text-delta, tool-input-delta, etc.):

```
Client                                               Server (agent handler)
  │                                                    │
  │── open_bi() → stream ──────────────────────────────▶│
  │── call.requested  { id: "c1",                      │
  │                       operationId: "/agent/chat",   │
  │                       input: { messages, model } }  │
  │                                                    │  handler reads capabilities (API key)
  │                                                    │  handler makes HTTP request to LLM provider
  │                                                    │  handler normalizes provider SSE → UI chunks
  │←─ call.responded  { id: "c1", output: { type: "text-start", ... } }        │
  │←─ call.responded  { id: "c1", output: { type: "text-delta", delta: "Hel" } }│
  │←─ call.responded  { id: "c1", output: { type: "text-delta", delta: "lo" } } │
  │←─ call.responded  { id: "c1", output: { type: "text-end", ... } }           │
  │←─ call.completed  { id: "c1" }                                              │
```

The API key used for the outbound LLM HTTP request comes from `OperationContext.capabilities`, not from the call protocol input and not from environment variables. See ADR-014 and [operation-registry.md → Capability Injection](operation-registry.md#capability-injection).

### PendingRequestMap

Manages in-flight calls and subscriptions. Correlates `call.responded` events back to the original `call.requested`:

```rust
pub struct PendingRequestMap {
    pending: HashMap<String, PendingEntry>,
}

enum PendingEntry {
    Call {
        tx: oneshot::Sender<Result<Value, CallError>>,
        timeout: Instant,
    },
    Subscribe {
        tx: mpsc::Sender<Result<Value, CallError>>,
        timeout: Option<Instant>,
    },
}
```

When a `call.responded` event arrives:
- If `PendingEntry::Call` → resolve the oneshot, delete entry
- If `PendingEntry::Subscribe` → push to the mpsc channel, keep entry alive

When `call.completed` arrives on a subscription → close the mpsc channel, delete entry.
When `call.aborted` arrives → cancel/drop whichever side initiated it.
A `call.aborted` for an unknown `requestId` is silently discarded.

Timeouts prevent dangling entries. A background task sweeps expired entries periodically.

### CallAdapter Stream Handling

The `CallAdapter::handle()` method:

1. Spawns a task that continuously calls `connection.accept_bi()` to receive incoming streams
2. For each accepted stream, reads `EventEnvelope` frames using `FrameFramedReader`
3. Dispatches `call.requested` events to the operation registry, **branching on `op_type`** (ADR-049):
   - **`Query` / `Mutation`** → `OperationRegistry::invoke()` → write one `call.responded` (or `call.error`) `EventEnvelope` frame
   - **`Subscription`** → `OperationRegistry::invoke_streaming()` → write each `call.responded` `EventEnvelope` as the stream yields → write `call.completed` on natural stream end (or `call.error` if the stream yields an `Err`). `deadline: None` for subscriptions (unbounded — see Timeouts below). Abort (`call.aborted` arriving for the request ID, or the stream being dropped) cascades per ADR-016: the stream future is dropped, `Drop` guards release the handler's resources, and descendants are aborted.
4. Writes response `EventEnvelope` frames using `FrameFramedWriter`
5. Manages `PendingRequestMap` for outgoing calls initiated by the server

The streaming branch is the server-side path that makes `Subscription` operations work end-to-end. Without it, a `Subscription` op registered with a `StreamingHandler` had no server-side dispatch path — the handler produced a stream but the dispatcher only read one `ResponseEnvelope` and closed. ADR-049 adds the `StreamingHandler` type and the `invoke_streaming()` dispatch path; this section wires them into the accept loop. See [operation-registry.md](operation-registry.md#handler) for the `Handler` / `StreamingHandler` / `HandlerKind` types.

For outgoing calls (server → client), the adapter:
1. Opens a bidirectional stream with `connection.open_bi()`
2. Sends `call.requested` on that stream
3. Adds the request ID to the `PendingRequestMap`
4. Reads responses from any stream, correlates by ID

### AuthContext and Identity Resolution

The `CallAdapter` receives an `AuthContext` from the endpoint. The call protocol resolves identity per-request, not per-connection:

**Resolution flow**:

1. The endpoint provides `AuthContext` with whatever identity it resolved at the TLS layer (e.g., client certificate fingerprint). This may be `None` — the `AuthContext.identity` field is `Option<Identity>`.
2. When a `call.requested` event arrives, the `CallAdapter` constructs an `OperationContext` with the connection-level `AuthContext.identity`.
3. If the `call.requested` payload includes an `auth_token` field, the `CallAdapter` resolves it using `IdentityProvider::resolve_from_token()`. If resolution succeeds, the resulting `Identity` replaces the connection-level identity in the `OperationContext`. If resolution fails, the request proceeds with the connection-level identity (which may be `None`).
4. The `OperationContext.identity` is passed to the `OperationRegistry` for ACL checking.
5. If `identity` is `None` and the operation's `AccessControl` has restrictions, the registry returns `FORBIDDEN` with message `"authentication required"`.

**Key point**: Identity is resolved per-request, not per-connection. This allows a single connection to upgrade authentication mid-session (e.g., after an `auth/login` operation returns a token), and allows different operations on the same connection to have different identity levels.

### Root OperationContext Construction

When a `call.requested` arrives from the wire, the `CallAdapter` constructs the root `OperationContext` — the entry point of the call tree. This is the counterpart to `OperationEnv::invoke()` (which constructs nested contexts with `internal: true`): the wire path sets `internal: false`, meaning ACL runs against the caller's `identity`, not a handler's composition authority (ADR-015, ADR-022).

```rust
// CallAdapter dispatch path — root context for an incoming wire request
fn build_root_context(
    &self,
    request_id: String,
    operation_name: &str,        // looked up in registry for the registration bundle
    identity: Option<Identity>,  // resolved per-request above (caller's identity)
    forwarded_for: Option<Identity>,  // from call.requested.forwarded_for (ADR-032)
) -> OperationContext {
    let registration = self.registry.registration(operation_name);
    OperationContext {
        request_id,
        parent_request_id: None,        // wire request — top of the call tree
        identity: identity.clone(),     // caller's identity (inbound — gate credential)
        // Composition authority from the registration bundle (ADR-022).
        // None for leaves (FromOpenAPI/FromMCP/FromCall); Some for Local/Session.
        // This is on the context for PROPAGATION to children via invoke(),
        // not for the root's own ACL (which uses identity above).
        handler_identity: registration.composition_authority.clone(),
        // Forwarded-for identity (ADR-032): the originator when this call was
        // forwarded by a from_call handler. Metadata only — AccessControl::check
        // never reads it; ACL always authorizes `identity` (the direct caller).
        // None when the call wasn't forwarded or the forwarder chose not to
        // propagate it. Populated from the wire call.requested.forwarded_for
        // field; NOT inherited by composed children (wire-ingress only).
        forwarded_for,
        capabilities: registration.capabilities.clone(),  // from the registration bundle
        metadata: HashMap::new(),        // fresh per request
        deadline: Some(Instant::now() + self.default_timeout),  // root deadline (W7)
        scoped_env: registration.scoped_env.clone()
            .unwrap_or_else(ScopedPeerEnv::empty),  // from the bundle, empty for leaves
        // Per-call env composition (ADR-024 + ADR-029): the root env is a
        // PeerCompositeEnv — the curated base + this connection's imported-
        // ops overlay (peer-keyed in the head's aggregation env, ADR-029 §1)
        // + the active session overlay (if any). The CallAdapter builds this
        // composite per incoming call — same shape as per-call identity
        // resolution via IdentityProvider. Handlers call env.invoke() (peer-
        // agnostic) or env.invoke_peer(peer, ...) (peer-specific, ADR-029 §2);
        // the composite routes to the right overlay.
        env: self.compose_root_env(/* peer_id, connection_overlay, session */),
        abort_policy: AbortPolicy::default(),  // abort-dependents (ADR-016 Decision 6)
        internal: false,                 // external call — ACL against caller identity
    }
}
```

The `internal: false` here is what makes a wire call a wire call — ACL checks against the caller's resolved `identity`. When a handler subsequently calls `context.env.invoke(...)`, the `OperationEnv::invoke()` path (see [operation-registry.md](operation-registry.md#operationenv)) constructs a nested `OperationContext` with `internal: true`, switching authority to `handler_identity`. The two construction paths — `CallAdapter` for wire-originated, `OperationEnv::invoke()` for composition-originated — are the only places `internal` is set. Handlers cannot set it themselves (the field is module-private for writes — see [operation-registry.md](operation-registry.md#operationcontext) and ADR-015).

The per-call `env` composition (ADR-024 + ADR-029) is the operation-dispatch analogue of the per-call identity resolution the CallAdapter already does via `IdentityProvider`. Both are integration-point patterns: the trait object owns the routing, the CallAdapter supplies the right sources per call. A connection's imported-ops overlay is part of the root env only for calls arriving on that connection — and on a head node with multiple worker connections, the overlays are peer-keyed (`PeerCompositeEnv`, ADR-029 §1); a session overlay is part of the root env only when a session is active. See ADR-024, ADR-029, and the `PeerCompositeEnv` sketch in [operation-registry.md](operation-registry.md#operationenv).

### ResponseEnvelope

The universal return type from all operation invocations:

```rust
pub struct ResponseEnvelope {
    pub request_id: String,
    pub result: Result<Value, CallError>,
}

pub struct CallError {
    pub code: String,        // protocol-level (NOT_FOUND, FORBIDDEN, ...) or operation-level (ADR-023)
    pub message: String,     // human-readable, for logging — not for programmatic handling
    pub retryable: bool,
    pub details: Option<Value>,  // typed error payload, conforms to ErrorDefinition.schema (ADR-023)
}
```

Local dispatch produces `ResponseEnvelope` with no serialization overhead. The `CallAdapter` converts `ResponseEnvelope` to `EventEnvelope` for the wire. When a handler returns a `CallError` whose `code` matches a declared `ErrorDefinition`, the `details` field carries the typed error payload. See ADR-023.

### Connection and Stream Lifecycle

**Connection drop**: When the QUIC connection closes, all pending requests in the `PendingRequestMap` are failed with `call.error` code `INTERNAL` and message `"connection closed"`. All subscription channels are closed. The `CallAdapter::handle()` method returns `Ok(())` (clean shutdown) or `Err(HandlerError::ConnectionClosed)` (unexpected).

**Stream reset**: When a QUIC stream is reset mid-operation, the `FrameFramedReader` returns an error. If the stream was carrying a subscription, the `PendingRequestMap` entry is removed and the mpsc channel is closed. If the stream was carrying a call, the oneshot is resolved with an error. No `call.aborted` is sent — the stream is gone.

**Timeouts**: Default timeout for wire calls is 30 seconds, configurable via
`CallAdapter::with_timeout()`. The `build_root_context` sets
`OperationContext.deadline` to `now + default_timeout`. Composed calls
inherit the parent's deadline (children do **not** get a fresh 30s — the
root call's deadline bounds the entire call tree, preventing a depth-5
composition from running 150s). A composed call that exceeds the deadline
is cancelled (future dropped, `Drop` guards release resources) and returns
`CallError { code: "TIMEOUT", retryable: true }`. Subscriptions default to
no deadline (`deadline: None` — unbounded); the client can specify a
timeout in the `call.requested` payload. The `PendingRequestMap` sweeper
runs every 10 seconds and removes expired wire entries.

**Error handling in `CallAdapter::handle()`**: If a handler panics, the stream is closed and the `PendingRequestMap` entry (if any) is cleaned up by the next sweeper pass. Other streams and the connection are unaffected.

### Abort Cascade and Nested Calls

When a handler composes other operations via `OperationEnv::invoke()`, it creates a call tree: a parent request (r1) spawns children (r1-a, r1-b), which may spawn their own children. The `parent_request_id` field on `OperationContext` records this tree — it is the agency chain (ADR-015).

When `call.aborted` arrives for a parent request, the protocol cascades the abort to all non-terminal descendants in the tree. The CallAdapter walks the tree (indexed by `parent_request_id` in `PendingRequestMap`) and sends `call.aborted` for each descendant. The default policy is **`abort-dependents`**: aborting a request aborts everything downstream, regardless of branch. This is the correct default because aborted parent work has no consumer waiting for results — continuing is wasted work at best and unwanted side effects at worst (e.g., a `bash/exec` that keeps running after the caller stopped caring).

An opt-in **`continue-running`** policy is available for cases where long-running work should survive a parent's abort (e.g., a subscription that should keep streaming). Under `continue-running`, descendants that have already started continue to completion; descendants that haven't started yet are aborted; no new descendants start.

The abort policy is set on `OperationContext` and propagated through `OperationEnv::invoke()` — the composing handler decides the child's policy, not the wire caller. The `call.requested` payload does not carry an abort policy field (the wire caller doesn't know the composition tree). The root context gets the default (`abort-dependents`); a handler can opt a child into `continue-running` at `invoke()` time. See ADR-016 Decision 6.

Handlers clean up resources when their call is cancelled (in Rust, the future is dropped and `Drop` guards release resources — HTTP streams, file handles, locks). This is a handler-level concern; the protocol's job is to cascade the abort. See ADR-016.

## Constraints

- The call protocol does not depend on any database. `PendingRequestMap` is in-memory. Durable session storage is a consumer concern.
- Operation specs use JSON Schema. The envelope is always JSON. Binary payloads may be base64-encoded in the `payload` field.
- Batch is not a protocol primitive — multiple `call.requested` events with correlated IDs provide equivalent semantics. See OQ-14.
- The call protocol is transport-agnostic at the envelope level. The `EventEnvelope` framing can run over QUIC streams, WebSocket frames, or Worker `postMessage`. The `CallAdapter` is the QUIC-specific implementation. **The `EventEnvelope` shape (`{ type, id, payload }`) was derived from the `@alkdev/pubsub` `EventEnvelope` (`/workspace/@alkdev/pubsub/src/types.ts`), which already has a working WebSocket client/server implementation (`event-target-websocket-client.ts` / `event-target-websocket-server.ts`) and a generalized "event target" abstraction. The call protocol refined the envelope with typed event names (`call.requested`, `call.responded`, etc.) and structured payloads; the delta is small and well-defined, making a browser (and Node) WebSocket client straightforward to derive from the pubsub prior art. See ADR-044, [ADR-048](../../decisions/048-websocket-native-session-not-gateway.md), and [websocket.md](../http/websocket.md).
- `OperationEnv::invoke()` dispatches through the local registry. Remote dispatch (federation, head/worker routing) would be a separate mechanism at a different layer. See ADR-064 and OQ-13.
- **The call protocol carries no secret material.** Secret material (private keys, API keys, mnemonics, decrypted credentials, raw tokens) must not appear in `call.requested` payloads, `call.responded` payloads, or `OperationContext.metadata`. The wire format carries `serde_json::Value` and cannot enforce this at the type level — the constraint is architectural, enforced by the operation registry and by convention. Operations that need to share public key material use a dedicated operation that returns only the public component. See ADR-014.
- **Abort cascades to descendants.** `call.aborted` for a parent request cascades to all non-terminal descendants in the call tree. Default policy is `abort-dependents`; `continue-running` is an opt-in. See ADR-016.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| Hand-rolled EventEnvelope framing (irpc never integrated) | [ADR-064](../../decisions/064-irpc-never-integrated-hand-rolled-framing.md) | Hand-rolled length-prefixed JSON framing, operation registry, dispatch; supersedes ADR-005 (irpc was never imported) |
| Call protocol stream model | [ADR-012](../../decisions/012-call-protocol-stream-model.md) | Bidirectional streams, EventEnvelope, ID-based correlation |
| ALPN per connection | [ADR-006](../../decisions/006-alpn-convention-and-connection-model.md) | `alknet/call` is a distinct ALPN, one connection per ALPN |
| ProtocolHandler receives Connection | [ADR-007](../../decisions/007-bistream-type-definition.md) | CallAdapter gets Connection, can accept/open multiple streams |
| Vault integration point | [ADR-008](../../decisions/008-secret-service-integration.md) | Vault is a capability source, accessed at assembly time |
| Secret material flow | [ADR-014](../../decisions/014-secret-material-flow-and-capability-injection.md) | Call protocol carries no secret material; capabilities injected at assembly layer |
| Privilege model and authority context | [ADR-015](../../decisions/015-privilege-model-and-authority-context.md) | `internal` = authority switch not ACL skip; External/Internal visibility; handler identity + scoped env |
| Abort cascade for nested calls | [ADR-016](../../decisions/016-abort-cascade-for-nested-calls.md) | `call.aborted` cascades to descendants; default `abort-dependents`, `continue-running` opt-in |
| Call protocol client and adapter contract | [ADR-017](../../decisions/017-call-protocol-client-and-adapter-contract.md) | `CallClient` opens connections; `from_call` imports remote ops; connection direction independent of call direction. Client/adapter surface specced in [client-and-adapters.md](client-and-adapters.md) |
| Handler registration, provenance, and composition authority | [ADR-022](../../decisions/022-handler-registration-provenance-and-composition-authority.md) | Registration bundle carries provenance, composition authority, scoped env, capabilities; dispatch path reads from bundle |
| Peer-graph routing model (supersedes ADR-028) | [ADR-029](../../decisions/029-peer-graph-routing-model.md) | Peer-keyed overlays + `PeerRef` routing; `AccessControl`-based peer authorization; retires `remote_safe`/`trusted_peer` |
| Forwarded-for identity | [ADR-032](../../decisions/032-forwarded-for-identity.md) | `forwarded_for` field on `call.requested` and `OperationContext`; metadata only — `AccessControl::check` never reads it; the `from_call` handler populates it |
| Operation error schemas | [ADR-023](../../decisions/023-operation-error-schemas.md) | Operations declare domain errors; `call.error` carries typed `details` |
| Streaming handler for subscriptions | [ADR-049](../../decisions/049-streaming-handler-for-subscriptions.md) | `StreamingHandler` type, `invoke_streaming()` dispatch path, `INVALID_OPERATION_TYPE` protocol code; the server-side streaming branch in `handle_stream` |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-13** (resolved): Operation path format is `/{service}/{op}`. Remote dispatch is a separate mechanism, not a path prefix.
- **OQ-14** (resolved): Batch is a client-side pattern of correlated `call.requested` events, not a protocol primitive.
- **OQ-16** (resolved by ADR-014): No vault operations are exposed over the call protocol for now.
- **OQ-19** (resolved): Session-scoped operation registries — agent-written operations overlaid on global registry via `OperationEnv` trait layering. Protocol doesn't need changes; `OperationEnv` must remain a trait.
- **OQ-25** (dissolved by ADR-029): `remote_safe` marking shape — moot;
  `remote_safe`/`trusted_peer` retired; peer authorization is
  `AccessControl::check(peer_identity)`.
- **OQ-26** (resolved): `OperationAdapter` error type — `AdapterError`
  variants (`DiscoveryFailed`, `SchemaParse`, `Transport`, `Unauthorized`,
  `SamePeerCollision`); `#[non_exhaustive]`. See
  [client-and-adapters.md](client-and-adapters.md).
- **OQ-27** (resolved): `from_call` re-import trigger — `from_call` is a manual
  free function; the assembly layer calls it after `connect()`. See
  [ADR-069](../../decisions/069-from-call-manual-free-function.md).
- **OQ-28** (resolved): `from_call` namespace collision — same-peer collision
  = error; cross-peer dissolved by ADR-029 (separate sub-overlays). See
  [client-and-adapters.md](client-and-adapters.md).
- **OQ-29** (resolved): `CallClient` TLS client-auth — wire quinn client-auth
  (present Ed25519 key as raw public key client cert); key-type-aware server
  cert verification; fingerprint normalization. See
  [client-and-adapters.md](client-and-adapters.md).
- **OQ-30** (resolved): `PeerRef::Any` routing policy — insertion-order
  first-match. See [client-and-adapters.md](client-and-adapters.md).
- **OQ-31** (resolved): `services/list-peers` re-export semantics — opt-in;
  `services/list` is "own ops only." See [client-and-adapters.md](client-and-adapters.md).
- **OQ-32** (open, feature extension): Multi-hop federation — the one-hop
  model is the architectural commitment; multi-hop is a feature extension
  that doesn't break downstream. See [client-and-adapters.md](client-and-adapters.md).
- **OQ-33** (resolved by ADR-030): `PeerId` source — `Identity.id` from
  `IdentityProvider` resolution (= `PeerEntry.peer_id`, stable across key
  rotation), not a connection-assigned UUID.
- **OQ-34** (resolved by ADR-030 + ADR-033): Persistent peer registry —
  the storage boundary is `core trait + in-memory default`; persistence
  adapters are separate crates.
- **OQ-37** (resolved by ADR-034): X.509 outgoing-only case — three remote
  roles named (public X.509 endpoint, transport relay, hub); pure-client
  X.509 connections are not in the peer graph on the client side. See
  [client-and-adapters.md](client-and-adapters.md).

## References

- [operation-registry.md](operation-registry.md) — OperationSpec, Handler, AccessControl, service discovery
- [client-and-adapters.md](client-and-adapters.md) — CallClient, from_call, OperationAdapter, peer-keyed composition env
- ADR-064: Hand-rolled EventEnvelope framing (irpc never integrated; supersedes ADR-005)
- ADR-012: Call protocol stream model
- ADR-029: Peer-graph routing model (peer-keyed overlays + `PeerRef` routing)
- ADR-030: PeerEntry and Identity.id decoupling (`PeerId` source)
- ADR-032: Forwarded-for identity (`forwarded_for` on `call.requested` and `OperationContext`)
- ADR-034: Outgoing-only X.509 and the three peer roles
- ADR-049: Streaming handler for subscriptions (server-side streaming dispatch path)
- Reference implementation: `/workspace/@alkdev/alknet-main/crates/alknet-core/src/call/`