---
status: draft
last_updated: 2026-06-07
---

# Call Protocol

## What

A bidirectional, transport-agnostic call and event protocol that runs over
authenticated pipes. It supports request/response calls, streaming
subscriptions, and unidirectional events — all using the same wire format. The
protocol is defined as a spec + handler + registry; downstream consumers (NAPI,
Python, head/worker) register their own operations without modifying core.

## Why

The current control channel (ADR-018) is unidirectional (client → server) and
provides fire-and-forget event dispatch without request/response semantics.
The call protocol generalizes it to support bidirectional calls (ADR-024) and
downstream service registration (ADR-025), enabling the head/worker model where
workers expose operations the head invokes.

## Architecture

### Operation Paths

Operation names use slash-based paths aligned with URL routing conventions:

```
/{node}/{service}/{op}
```

- **node** — identity prefix of the node that exposes the operation. The head
  uses this segment to route calls to the correct connected node.
- **service** — the logical service namespace. Groups related operations
  under one handler prefix.
- **op** — the specific operation within that service.

Examples:

| Path | Meaning |
|------|---------|
| `/dev1/fs/readFile` | Node `dev1`, service `fs`, operation `readFile` |
| `/dev1/bash/exec` | Node `dev1`, service `bash`, operation `exec` |
| `/head/agent/chat` | Head's own `agent` service, operation `chat` |
| `/head/sessions/list` | Head's own `sessions` service, operation `list` |
| `/browser-1/notify/alert` | Worker `browser-1`, `notify` service |

This three-level routing mirrors iroh's ALPN dispatch: the first segment
routes to a connected node (like ALPN routes to a protocol handler), the
remaining path dispatches within that node's registry. See ADR-025 for the
handler/spec separation decision.

The `namespace` field on `OperationSpec` is derived from the path (`namespace`
= second path segment). It's a convenience accessor for ACL matching and
service grouping.

### Wire Format: EventEnvelope

Every message on the wire is a length-prefixed JSON `EventEnvelope`:

```rust
pub struct EventEnvelope {
    pub r#type: String,    // Event type (e.g., "call.requested", "call.responded")
    pub id: String,        // Correlation key (requestId, topic, or "" for broadcasts)
    pub payload: Value,   // JSON payload — schema depends on event type
}

// Frame: 4-byte big-endian length prefix + UTF-8 JSON body
```

This is the same format used by `@alkdev/pubsub` adapters. It is JSON because
it must be consumable from JavaScript, Python, and any language. The envelope
is transport-agnostic — it runs over SSH channels, WebTransport streams, iroh
bidirectional streams, WebSocket, or Worker postMessage.

Binary payloads (postcard, protobuf, etc.) are base64-encoded in the `payload`
field. The envelope itself stays JSON for cross-language compatibility.

### Call Protocol Events

Five event types carry request/response and subscription semantics:

| Event | Direction | Purpose |
|-------|-----------|---------|
| `call.requested` | Caller → Handler | Initiate a call or subscription |
| `call.responded` | Handler → Caller | Deliver a result (one for calls, many for subscriptions) |
| `call.completed` | Handler → Caller | Signal end of subscription stream |
| `call.aborted` | Either side | Cancel the call/subscription |
| `call.error` | Handler → Caller | Signal an error |

**`call.error` payload**:
```json
{
  "code": "string",
  "message": "string",
  "retryable": false
}
```

**A call is just a subscribe that resolves after one event.** Both `call()` and
`subscribe()` send the same `call.requested` event. The difference is
consumption pattern:

- **`call()`**: Sends `call.requested`, resolves `Promise` on first `call.responded`
- **`subscribe()`**: Sends `call.requested`, yields each `call.responded` until `call.completed` or `call.aborted`

The `id` field carries the `requestId` for correlation.

### Bidirectional Calls and Routing

Both sides of a connection can initiate calls. The head routes calls to workers
using the first path segment:

```
Head (server)                              Worker: "dev1" (client)
     │                                           │
     │  call.requested                           │
     │  name: "/dev1/fs/readFile"                │
     │  payload: { path: "/src/main.rs" }        │
     │──────────────────────────────────────────▶│
     │                                           │
     │  call.responded                           │
     │  id: <requestId>                          │
     │  payload: { content: "fn main()..." }     │
     │◀──────────────────────────────────────────│
     │                                           │
     │          Worker exposes /dev1/fs/*,        │
     │          /dev1/bash/* to head              │
     │                                           │
     │◀─ call.requested ────────────────────────│
     │  name: "/head/agent/chat"                  │
     │  payload: { provider: "anthropic", ... }  │
     │                                           │
     │── call.responded ──────────────────────▶ │
     │  id: <requestId>                          │
     │  payload: { completion: "..." }            │
```

The head's registry includes:
- **Head-local operations** (`/head/*`) — handled directly
- **Remote operations** (`/{node}/*`) — forwarded to the worker connection

When the head routes `/dev1/fs/readFile` to worker `dev1`, it strips the node
prefix and delivers the call to the worker's local registry as `/fs/readFile`.
The worker doesn't need to know its own alias.

### Head/Worker Architecture

```
         ┌─────────────────────────────────┐
         │           Head Node             │
         │                                 │
         │  Head-local services:           │
         │  /head/agent/chat  (LLM coord)  │
         │  /head/agent/complete           │
         │  /head/sessions/list            │
         │  /head/sessions/history         │
         │                                 │
         │  Worker registry (discovered):  │
         │  /dev1/fs/* → dev1 connection   │
         │  /dev1/bash/* → dev1 connection │
         │  /dev2/fs/* → dev2 connection   │
         │  /browser-1/notify/* → WT conn │
         └──────┬───────┬───────┬──────────┘
                │       │       │
      ┌─────────▼┐ ┌───▼────┐ ┌▼───────────┐
      │  Worker  │ │Worker  │ │Browser Worker│
      │  "dev1"  │ │"dev2"  │ │"browser-1"  │
      │  /fs/*   │ │/fs/*   │ │/notify/*    │
      │  /bash/* │ │/bash/* │ │             │
      │  /search/*│ │       │ │             │
      └──────────┘ └────────┘ └─────────────┘
```

When a worker connects, it registers its operations with the head:

```
worker → head:  call.requested { name: "/head/services/register", payload: {
  node: "dev1",
  operations: ["/fs/readFile", "/fs/writeFile", "/bash/exec", "/search/query"]
}}
```

The head adds these to its routing table with the node prefix. Other workers
and browser clients can then call `/dev1/fs/readFile` without knowing how
the head routes it internally.

### Operation Registry

The operation registry maps paths to specs and handlers. **Specs and handlers
are separate** — downstream consumers register both (ADR-025).

```rust
pub struct OperationSpec {
    pub name: String,                    // e.g., "/fs/readFile", "/agent/chat"
    pub namespace: String,               // e.g., "fs", "agent"
    pub op_type: OperationType,          // Query, Mutation, Subscription
    pub input_schema: Value,             // JSON Schema for input
    pub output_schema: Value,            // JSON Schema for output
    pub access_control: AccessControl,   // Required scopes/resources
}

pub enum OperationType {
    Query,         // Read-only, idempotent (e.g., "/fs/readFile", "/search/query")
    Mutation,      // Side effects (e.g., "/bash/exec", "/sessions/create")
    Subscription,  // Streaming (e.g., "/events/subscribe")
}

pub struct AccessControl {
    pub required_scopes: Vec<String>,                  // AND-checked
    pub required_scopes_any: Option<Vec<String>>,       // OR-checked
    pub resource_type: Option<String>,                  // e.g., "service"
    pub resource_action: Option<String>,                // e.g., "read"
}
```

**Registration is separated from implementation:**

```rust
// Core registers discovery operations
registry.register(OperationSpec { name: "/services/list", ... }, list_services_handler);
registry.register(OperationSpec { name: "/services/schema", ... }, schema_handler);

// A dev env worker registers its tools
registry.register(OperationSpec { name: "/fs/readFile", ... }, fs_read_handler);
registry.register(OperationSpec { name: "/bash/exec", ... }, bash_exec_handler);

// A browser client registers notification UDFs
registry.register(OperationSpec { name: "/notify/alert", ... }, notify_handler);
```

Core-provided operations use short paths without a node prefix
(`/services/list`, `/services/schema`). They live on whatever node the
caller is connected to. Worker-prefixed operations (`/dev1/fs/readFile`)
are routed by the head.

### ACL Per Operation Path

Access control maps to path prefixes using standard URL-like matching:

| Pattern | Matches | Purpose |
|---------|---------|---------|
| `/dev1/*` | All operations on node `dev1` | Full access to a worker |
| `/*/fs/*` | `fs` service on any node | Read file access across dev envs |
| `/*/bash/*` | `bash` service on any node | Shell access (higher risk) |
| `/head/agent/*` | Head LLM agent | LLM calls |
| `/head/sessions/*` | Head session management | Session history |
| `/browser-1/notify/alert` | Specific operation on specific node | One UI notification |

Higher-risk operations (shell, filesystem write) can require tighter scopes
than read-only operations. The ACL evaluates against the caller's
`Identity.scopes` and `Identity.resources` from the auth layer (see auth.md).

### Service Discovery

The `/services/list` and `/services/schema` operations expose what a node
offers. Read-only — no admin operations:

| Operation | Type | Description |
|-----------|------|-------------|
| `/services/list` | Query | List registered operation paths + metadata |
| `/services/schema` | Query | Get `OperationSpec` for a specific operation |

These tell the caller: "here's what you can call." They are not a control
panel. Access control is enforced at the operation level.

### PendingRequestMap

Manages in-flight calls and subscriptions. Correlates `call.responded` events
back to the original `call.requested`:

```rust
pub struct PendingRequestMap {
    pending: HashMap<String, PendingEntry>,
}

enum PendingEntry {
    Call {
        tx: oneshot::Sender<Result<Value>>,
        timeout: Instant,
    },
    Subscribe {
        tx: mpsc::Sender<Result<Value>>,
        timeout: Option<Instant>,
    },
}
```

When a `call.responded` event arrives:
- If `PendingEntry::Call` → resolve the oneshot, delete entry
- If `PendingEntry::Subscribe` → push to the mpsc channel, keep entry alive

When `call.completed` arrives on a subscription → close the mpsc channel, delete
entry. When `call.aborted` arrives → cancel/drop whichever side initiated it. A
`call.aborted` for an unknown `requestId` is silently discarded — no error
response is generated.

Timeouts prevent dangling entries. A background task sweeps expired entries
periodically.

### Protocol Adapter Layer

The call protocol is transport-agnostic by design. It maps to any transport
that carries `EventEnvelope` frames:

| Transport | Channel mechanism | Direction |
|-----------|-------------------|-----------|
| SSH | Reserved `direct_tcpip` destination (ADR-018) | Bidirectional over SSH channel |
| WebTransport | Bidirectional stream after CONNECT | Bidirectional over WT stream |
| iroh QUIC | Bidirectional `open_bi()` / `accept_bi()` | Bidirectional over QUIC stream |
| WebSocket | Single WS connection | Bidirectional over WS frames |
| Worker | `postMessage` | Bidirectional over structured clone |

The framing is always: 4-byte BE length prefix + JSON. The envelope shape is
the same regardless of transport.

### Relationship to @alkdev/pubsub and @alkdev/operations

The call protocol in core is a Rust reimplementation of the same protocol
defined in `@alkdev/operations`. The TypeScript implementation provides:

- `PendingRequestMap` — request/response correlation
- `CallHandler` — bridges pubsub events to operation registry
- `OperationSpec`, `AccessControl`, `Identity` — type definitions

The Rust implementation mirrors these types and behaviors. TypeScript consumers
continue using `@alkdev/operations` over `@alkdev/pubsub` adapters (including
the `event-target-alknet` adapter). Rust consumers use core's registry directly.
Both speak the same wire protocol and can interoperate.

The key principle: **the same `EventEnvelope` can flow from a Rust handler
through core, out over SSH channel, into a JavaScript pubsub adapter, and
be dispatched through `@alkdev/operations`'s call handler** — with zero
translation at the wire level.

### Agent Service Pattern

The head commonly runs an agent service that coordinates between LLM providers
and tool calls. This service is just another set of registered operations —
no special treatment:

- `/head/agent/chat` — send a message, get a completion. Routes to the
  appropriate LLM provider based on available workers and configuration.
- `/head/agent/complete` — streaming completion. Yields tokens as they arrive.
- `/head/sessions/list` — list session histories (backed by Honker or other
  durable storage).
- `/head/sessions/history` — retrieve a specific session's message history.

The agent service uses the same call protocol to invoke tools on workers:
`/dev1/fs/readFile` for file access, `/dev1/bash/exec` for shell commands. It
stores session state via whatever mechanism the head deployment provides — core
doesn't mandate Honker or any specific storage.

## Constraints

- The call protocol does not depend on Honker, SQLite, or any database. The
  `PendingRequestMap` is in-memory. Durable session storage is a consumer concern.
- Operation specs use JSON Schema. Complex sub-structures (postcard, protobuf)
  can be carried as base64-encoded blobs in the `payload`, but the envelope
  itself is always JSON.
- Service discovery (`/services/list`, `/services/schema`) is read-only. No
  admin operations are exposed through the call protocol itself.
- Batch is not a protocol primitive. Multiple `call.requested` events with
  correlated `requestId`s provide equivalent semantics.
- The node prefix in the operation path is a routing mechanism, not a security
  boundary. ACL is enforced at the `AccessControl` level, not by path prefix
  alone. A worker that exposes `/dev1/bash/exec` can restrict access via
  `required_scopes` — not every authenticated identity should have shell access.

## Open Questions

- **OQ-20**: How does the head track which workers expose which operations when
  workers connect and disconnect? Registration on connect and cleanup on
  disconnect, or heartbeat-based discovery? See
  [open-questions.md](open-questions.md).

- **OQ-22**: Should the call protocol support streaming inputs (client streaming
  in gRPC terms), or is client→server always a single request payload with
  streaming only server→client? See [open-questions.md](open-questions.md).

## Design Decisions

| ADR | Decision | Summary |
|-----|----------|---------|
| [018](decisions/018-control-channel-for-pubsub.md) | Control channel for pubsub | Reserved destination for event bus |
| [024](decisions/024-bidirectional-call-protocol.md) | Bidirectional call protocol | Generalizes ADR-018, both sides can call |
| [025](decisions/025-handler-spec-separation.md) | Handler/spec separation | Downstream registers operations without modifying core |

## References

- [auth.md](auth.md) — Identity and `IdentityProvider` trait
- [napi-and-pubsub.md](napi-and-pubsub.md) — NAPI wrapper and pubsub adapter
- [server.md](server.md) — Channel handling and control channel routing
- [transport.md](transport.md) — Transport abstraction
- [configuration.md](../research/configuration.md) — ForwardingPolicy, service metadata
- `@alkdev/pubsub` — TypeScript event target adapters and `EventEnvelope`
- `@alkdev/operations` — TypeScript call protocol, `OperationSpec`, registry
- `@alkdev/storage` — `peer_credentials` table, ACL graph, `Identity`
- [irpc](/workspace/irpc) — iroh streaming RPC (postcard-only, Rust-to-Rust)
- [iroh](/workspace/iroh) — P2P QUIC transport