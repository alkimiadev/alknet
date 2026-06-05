---
status: draft
last_updated: 2026-06-04
---

# Call Protocol

## What

A bidirectional, transport-agnostic call and event protocol that runs over
authenticated pipes. It supports request/response calls, streaming
subscriptions, and unidirectional events — all using the same wire format. The
protocol is defined as a spec + handler + registry; downstream consumers (NAPI,
Python, hub/spoke) register their own operations without modifying core.

## Why

The current control channel (ADR-018) is unidirectional (client → server) and
provides fire-and-forget event dispatch without request/response semantics.
The call protocol generalizes it to support bidirectional calls (ADR-024) and
downstream service registration (ADR-025), enabling the hub/spoke model where
spokes expose operations the hub invokes.

## Architecture

### Operation Paths

Operation names use slash-based paths aligned with URL routing conventions:

```
/{spoke}/{service}/{op}
```

- **spoke** — identity prefix of the node that exposes the operation. The hub
  uses this segment to route calls to the correct connected node.
- **service** — the logical service namespace. Groups related operations
  under one handler prefix.
- **op** — the specific operation within that service.

Examples:

| Path | Meaning |
|------|---------|
| `/dev1/fs/readFile` | Spoke `dev1`, service `fs`, operation `readFile` |
| `/dev1/bash/exec` | Spoke `dev1`, service `bash`, operation `exec` |
| `/hub/agent/chat` | Hub's own `agent` service, operation `chat` |
| `/hub/sessions/list` | Hub's own `sessions` service, operation `list` |
| `/browser-1/notify/alert` | Browser spoke `browser-1`, `notify` service |

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

Both sides of a connection can initiate calls. The hub routes calls to spokes
using the first path segment:

```
Hub (server)                              Spoke: "dev1" (client)
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
     │          Spoke exposes /dev1/fs/*,        │
     │          /dev1/bash/* to hub              │
     │                                           │
     │◀─ call.requested ────────────────────────│
     │  name: "/hub/agent/chat"                  │
     │  payload: { provider: "anthropic", ... }  │
     │                                           │
     │── call.responded ──────────────────────▶ │
     │  id: <requestId>                          │
     │  payload: { completion: "..." }            │
```

The hub's registry includes:
- **Hub-local operations** (`/hub/*`) — handled directly
- **Remote operations** (`/{spoke}/*`) — forwarded to the spoke connection

When the hub routes `/dev1/fs/readFile` to spoke `dev1`, it strips the spoke
prefix and delivers the call to the spoke's local registry as `/fs/readFile`.
The spoke doesn't need to know its own alias.

### Hub/Spoke Architecture

```
         ┌─────────────────────────────────┐
         │              Hub                │
         │                                 │
         │  Hub-local services:            │
         │  /hub/agent/chat   (LLM coord)  │
         │  /hub/agent/complete            │
         │  /hub/sessions/list             │
         │  /hub/sessions/history          │
         │                                 │
         │  Spoke registry (discovered):   │
         │  /dev1/fs/* → dev1 connection    │
         │  /dev1/bash/* → dev1 connection  │
         │  /dev2/fs/* → dev2 connection    │
         │  /browser-1/notify/* → WT conn  │
         └──────┬───────┬───────┬──────────┘
                │       │       │
      ┌─────────▼┐ ┌───▼────┐ ┌▼───────────┐
      │  Dev Spoke│ │Dev Spk │ │Browser Spoke│
      │  "dev1"   │ │"dev2"  │ │"browser-1"  │
      │  /fs/*    │ │/fs/*   │ │/notify/*    │
      │  /bash/*  │ │/bash/* │ │             │
      │  /search/*│ │        │ │             │
      └───────────┘ └────────┘ └─────────────┘
```

When a spoke connects, it registers its operations with the hub:

```
spoke → hub:  call.requested { name: "/hub/services/register", payload: {
  spoke: "dev1",
  operations: ["/fs/readFile", "/fs/writeFile", "/bash/exec", "/search/query"]
}}
```

The hub adds these to its routing table with the spoke prefix. Other spokes
and browser clients can then call `/dev1/fs/readFile` without knowing how
the hub routes it internally.

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

// A dev env spoke registers its tools
registry.register(OperationSpec { name: "/fs/readFile", ... }, fs_read_handler);
registry.register(OperationSpec { name: "/bash/exec", ... }, bash_exec_handler);

// A browser client registers notification UDFs
registry.register(OperationSpec { name: "/notify/alert", ... }, notify_handler);
```

Core-provided operations use short paths without a spoke prefix
(`/services/list`, `/services/schema`). They live on whatever node the
caller is connected to. Spoke-prefixed operations (`/dev1/fs/readFile`)
are routed by the hub.

### ACL Per Operation Path

Access control maps to path prefixes using standard URL-like matching:

| Pattern | Matches | Purpose |
|---------|---------|---------|
| `/dev1/*` | All operations on spoke `dev1` | Full access to a spoke |
| `/*/fs/*` | `fs` service on any spoke | Read file access across dev envs |
| `/*/bash/*` | `bash` service on any spoke | Shell access (higher risk) |
| `/hub/agent/*` | Hub LLM agent | LLM calls |
| `/hub/sessions/*` | Hub session management | Session history |
| `/browser-1/notify/alert` | Specific operation on specific spoke | One UI notification |

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

The hub commonly runs an agent service that coordinates between LLM providers
and tool calls. This service is just another set of registered operations —
no special treatment:

- `/hub/agent/chat` — send a message, get a completion. Routes to the
  appropriate LLM provider based on available spokes and configuration.
- `/hub/agent/complete` — streaming completion. Yields tokens as they arrive.
- `/hub/sessions/list` — list session histories (backed by Honker or other
  durable storage).
- `/hub/sessions/history` — retrieve a specific session's message history.

The agent service uses the same call protocol to invoke tools on spokes:
`/dev1/fs/readFile` for file access, `/dev1/bash/exec` for shell commands. It
stores session state via whatever mechanism the hub deployment provides — core
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
- The spoke prefix in the operation path is a routing mechanism, not a security
  boundary. ACL is enforced at the `AccessControl` level, not by path prefix
  alone. A spoke that exposes `/dev1/bash/exec` can restrict access via
  `required_scopes` — not every authenticated identity should have shell access.

## Open Questions

- **OQ-20**: How does the hub track which spokes expose which operations when
  spokes connect and disconnect? Registration on connect and cleanup on
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