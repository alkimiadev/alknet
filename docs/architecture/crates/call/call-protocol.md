---
status: draft
last_updated: 2026-06-19
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

See ADR-005 for the decision to use irpc as the call protocol's foundation and ADR-012 for the stream model decision.

## Architecture

### CallAdapter

The `CallAdapter` implements `ProtocolHandler`:

```rust
pub struct CallAdapter {
    registry: Arc<OperationRegistry>,
    identity_provider: Arc<dyn IdentityProvider>,
}

#[async_trait]
impl ProtocolHandler for CallAdapter {
    fn alpn(&self) -> &'static [u8] { b"alknet/call" }

    async fn handle(&self, connection: Connection, auth: &AuthContext) -> Result<(), HandlerError> {
        // Accept bidirectional streams, read EventEnvelopes,
        // dispatch to registry, write responses
    }
}
```

The adapter:
1. Accepts bidirectional streams on the connection
2. Reads length-prefixed JSON `EventEnvelope` frames from each stream
3. Resolves the peer's identity using `AuthContext` and `IdentityProvider`
4. Dispatches `call.requested` events to the operation registry
5. Writes response `EventEnvelope` frames back to the appropriate stream
6. Manages the `PendingRequestMap` for outgoing calls

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

This is the same framing used by irpc. The Rust implementation in alknet-call is canonical — the `@alkdev/pubsub` TypeScript adapters serve as a reference and browser adaptation, not a parallel implementation (see ADR-013).

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

### `call.error` Payload

```json
{
  "code": "NOT_FOUND",
  "message": "operation not found: /fs/readFile",
  "retryable": false
}
```

Error codes use an extensible string enum. The protocol defines the following codes:
- `NOT_FOUND` — operation not in registry
- `FORBIDDEN` — access denied (insufficient scopes or unauthenticated)
- `INVALID_INPUT` — input doesn't match the operation's JSON Schema
- `INTERNAL` — handler error
- `TIMEOUT` — request timed out (retryable: true)

New error codes may be added in future versions. Clients should treat unknown error codes as `INTERNAL` with `retryable: false`.

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
3. Dispatches `call.requested` events to the operation registry
4. Writes response `EventEnvelope` frames using `FrameFramedWriter`
5. Manages `PendingRequestMap` for outgoing calls initiated by the server

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

### ResponseEnvelope

The universal return type from all operation invocations:

```rust
pub struct ResponseEnvelope {
    pub request_id: String,
    pub result: Result<Value, CallError>,
}

pub struct CallError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}
```

Local dispatch produces `ResponseEnvelope` with no serialization overhead. The `CallAdapter` converts `ResponseEnvelope` to `EventEnvelope` for the wire.

### Connection and Stream Lifecycle

**Connection drop**: When the QUIC connection closes, all pending requests in the `PendingRequestMap` are failed with `call.error` code `INTERNAL` and message `"connection closed"`. All subscription channels are closed. The `CallAdapter::handle()` method returns `Ok(())` (clean shutdown) or `Err(HandlerError::ConnectionClosed)` (unexpected).

**Stream reset**: When a QUIC stream is reset mid-operation, the `FrameFramedReader` returns an error. If the stream was carrying a subscription, the `PendingRequestMap` entry is removed and the mpsc channel is closed. If the stream was carrying a call, the oneshot is resolved with an error. No `call.aborted` is sent — the stream is gone.

**Timeouts**: Default timeout for calls is 30 seconds. Default timeout for subscriptions is optional (the client can specify a timeout in the `call.requested` payload, or leave it open-ended). The `PendingRequestMap` sweeper runs every 10 seconds and removes expired entries. Timeouts are configurable at the `CallAdapter` level, not per-operation.

**Error handling in `CallAdapter::handle()`**: If a handler panics, the stream is closed and the `PendingRequestMap` entry (if any) is cleaned up by the next sweeper pass. Other streams and the connection are unaffected.

### Abort Cascade and Nested Calls

When a handler composes other operations via `OperationEnv::invoke()`, it creates a call tree: a parent request (r1) spawns children (r1-a, r1-b), which may spawn their own children. The `parent_request_id` field on `OperationContext` records this tree.

When `call.aborted` arrives for a parent request, the protocol cascades the abort to all non-terminal descendants in the tree. The default policy is **`abort-dependents`**: aborting a request aborts everything downstream, regardless of branch. This is the correct default because aborted parent work has no consumer waiting for results — continuing is wasted work at best and unwanted side effects at worst (e.g., a `bash/exec` that keeps running after the caller stopped caring).

An opt-in **`continue-running`** policy is available for cases where long-running work should survive a parent's abort (e.g., a subscription that should keep streaming). The caller or handler specifies the policy at call time.

The one-way door is the protocol event schema: `call.aborted` must carry cascade semantics before implementation, because retrofitting cascade onto a non-cascading abort is a breaking protocol change. The mechanism — how the runtime discovers descendants and propagates cancellation (cancellation tokens, parent-indexed map, or a separate graph structure) — is a two-way door for implementation. See OQ-17.

## Constraints

- The call protocol does not depend on any database. `PendingRequestMap` is in-memory. Durable session storage is a consumer concern.
- Operation specs use JSON Schema. The envelope is always JSON. Binary payloads may be base64-encoded in the `payload` field.
- Batch is not a protocol primitive — multiple `call.requested` events with correlated IDs provide equivalent semantics. See OQ-14.
- The call protocol is transport-agnostic at the envelope level. The `EventEnvelope` framing can run over QUIC streams, WebSocket frames, or Worker `postMessage`. The `CallAdapter` is the QUIC-specific implementation.
- `OperationEnv::invoke()` dispatches through the local registry. Remote dispatch (federation, head/worker routing) would be a separate mechanism at a different layer. See ADR-005 and OQ-13.
- **The call protocol carries no secret material.** Secret material (private keys, API keys, mnemonics, decrypted credentials, raw tokens) must not appear in `call.requested` payloads, `call.responded` payloads, or `OperationContext.metadata`. The wire format carries `serde_json::Value` and cannot enforce this at the type level — the constraint is architectural, enforced by the operation registry and by convention. Operations that need to share public key material use a dedicated operation that returns only the public component. See ADR-014.
- **Abort cascades to descendants.** `call.aborted` for a parent request cascades to all non-terminal descendants in the call tree. Default policy is `abort-dependents`; `continue-running` is an opt-in. See OQ-17.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| irpc as call protocol foundation | [ADR-005](../../decisions/005-irpc-as-call-protocol-foundation.md) | irpc provides framing and service dispatch |
| Call protocol stream model | [ADR-012](../../decisions/012-call-protocol-stream-model.md) | Bidirectional streams, EventEnvelope, ID-based correlation |
| ALPN per connection | [ADR-006](../../decisions/006-alpn-convention-and-connection-model.md) | `alknet/call` is a distinct ALPN, one connection per ALPN |
| ProtocolHandler receives Connection | [ADR-007](../../decisions/007-bistream-type-definition.md) | CallAdapter gets Connection, can accept/open multiple streams |
| Vault integration point | [ADR-008](../../decisions/008-secret-service-integration.md) | Vault is a capability source, accessed at assembly time |
| Secret material flow | [ADR-014](../../decisions/014-secret-material-flow-and-capability-injection.md) | Call protocol carries no secret material; capabilities injected at assembly layer |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-13** (resolved): Operation path format is `/{service}/{op}`. Remote dispatch is a separate mechanism, not a path prefix.
- **OQ-14** (resolved): Batch is a client-side pattern of correlated `call.requested` events, not a protocol primitive.
- **OQ-15** (open): Call protocol client and adapter contract. ADR-014 constrains the adapter contract: adapters take credential sources from the assembly layer, not static tokens.
- **OQ-16** (resolved by ADR-014): No vault operations are exposed over the call protocol for now.
- **OQ-17** (open): Abort cascade semantics — `call.aborted` cascades to descendants, default `abort-dependents`, `continue-running` opt-in. One-way door on the event schema; mechanism is a two-way door.
- **OQ-18** (open): Privilege model and authority context — `internal` flag switches authority to handler identity, not blanket ACL skip. Operations have External/Internal visibility. Scoped composition env + handler identity. Needs agent crate in view.

## References

- [operation-registry.md](operation-registry.md) — OperationSpec, Handler, AccessControl, service discovery
- ADR-005: irpc as call protocol foundation
- ADR-012: Call protocol stream model
- Reference implementation: `/workspace/@alkdev/alknet-main/crates/alknet-core/src/call/`