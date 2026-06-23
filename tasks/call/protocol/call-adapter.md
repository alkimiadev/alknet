---
id: call/protocol/call-adapter
name: Implement CallAdapter (ProtocolHandler for alknet/call) with stream handling, identity resolution, and root context construction
status: pending
depends_on: [call/protocol/call-connection, call/registry/operation-env, call/registry/service-discovery, core/endpoint]
scope: broad
risk: high
impact: component
level: implementation
---

## Description

Implement `CallAdapter` in `src/protocol/adapter.rs`. This is the
`ProtocolHandler` implementation for ALPN `alknet/call` — the merge point of the
registry and protocol strands. It ties everything together: stream handling,
identity resolution, root context construction, env composition, dispatch.

### CallAdapter struct

```rust
pub struct CallAdapter {
    registry: Arc<OperationRegistry>,           // Layer 0 — curated, immutable
    identity_provider: Arc<dyn IdentityProvider>,
    session_source: Option<Arc<dyn SessionOverlaySource + Send + Sync>>,  // Layer 1
    default_timeout: Duration,                   // 30s default
}

impl CallAdapter {
    pub fn new(registry: Arc<OperationRegistry>, identity_provider: Arc<dyn IdentityProvider>) -> Self {
        Self { registry, identity_provider, session_source: None,
               default_timeout: Duration::from_secs(30) }
    }

    pub fn with_session_source(mut self, source: Arc<dyn SessionOverlaySource + Send + Sync>) -> Self {
        self.session_source = Some(source);
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }
}
```

### SessionOverlaySource trait

```rust
pub trait SessionOverlaySource: Send + Sync {
    fn overlay_for(&self, context: &OperationContext) -> Option<Arc<dyn OperationEnv + Send + Sync>>;
}
```

Defined in alknet-call because CallAdapter must name the type — alknet-call
cannot depend on alknet-agent (agent depends on call, not reverse). The agent
crate implements this trait; alknet-call defines it. Same pattern as
IdentityProvider (ADR-004).

### ProtocolHandler impl

```rust
#[async_trait]
impl ProtocolHandler for CallAdapter {
    fn alpn(&self) -> &'static [u8] { b"alknet/call" }

    async fn handle(&self, connection: Connection, auth: &AuthContext) -> Result<(), HandlerError> {
        // 1. Create CallConnection from the Connection
        // 2. Spawn a task that continuously calls connection.accept_bi()
        // 3. For each accepted stream, read EventEnvelope frames (FrameFramedReader)
        // 4. Dispatch call.requested events to the operation registry
        // 5. Write response EventEnvelope frames (FrameFramedWriter)
        // 6. Manage PendingRequestMap for outgoing calls
        // 7. On connection close: fail all pending, return Ok or Err(ConnectionClosed)
    }
}
```

### Stream handling

The adapter:
1. Spawns a task that continuously calls `connection.accept_bi()` to receive
   incoming streams
2. For each accepted stream, reads `EventEnvelope` frames using
   `FrameFramedReader`
3. Dispatches `call.requested` events to the operation registry
4. Writes response `EventEnvelope` frames using `FrameFramedWriter`
5. Manages `PendingRequestMap` for outgoing calls initiated by the server

For outgoing calls (server → client), the adapter:
1. Opens a bidirectional stream with `connection.open_bi()`
2. Sends `call.requested` on that stream
3. Adds the request ID to the `PendingRequestMap`
4. Reads responses from any stream, correlates by ID

### Identity resolution (per-request)

The CallAdapter resolves identity per-request, not per-connection:

1. The endpoint provides `AuthContext` with whatever identity it resolved at
   the TLS layer (may be `None`)
2. When a `call.requested` event arrives, the CallAdapter constructs an
   `OperationContext` with the connection-level `AuthContext.identity`
3. If the `call.requested` payload includes an `auth_token` field, the
   CallAdapter resolves it using `IdentityProvider::resolve_from_token()`. If
   resolution succeeds, the resulting `Identity` replaces the connection-level
   identity in the `OperationContext`. If resolution fails, the request
   proceeds with the connection-level identity (which may be `None`)
4. The `OperationContext.identity` is passed to the `OperationRegistry` for
   ACL checking
5. If `identity` is `None` and the operation's `AccessControl` has
   restrictions, the registry returns `FORBIDDEN` with message
   `"authentication required"`

**Key point**: Identity is resolved per-request. This allows a single
connection to upgrade authentication mid-session and allows different operations
on the same connection to have different identity levels.

### Root OperationContext construction

When a `call.requested` arrives from the wire, the CallAdapter constructs the
root `OperationContext` — the entry point of the call tree. This sets
`internal: false`, meaning ACL runs against the caller's `identity`, not a
handler's composition authority (ADR-015, ADR-022).

```rust
fn build_root_context(
    &self,
    request_id: String,
    operation_name: &str,
    identity: Option<Identity>,
    /* connection, session */
) -> OperationContext {
    let registration = self.registry.registration(operation_name);
    OperationContext {
        request_id,
        parent_request_id: None,        // wire request — top of call tree
        identity: identity.clone(),     // caller's identity (inbound)
        handler_identity: registration.composition_authority.clone(),
        capabilities: registration.capabilities.clone(),
        metadata: HashMap::new(),
        deadline: Some(Instant::now() + self.default_timeout),
        scoped_env: registration.scoped_env.clone()
            .unwrap_or_else(ScopedOperationEnv::empty),
        env: self.compose_root_env(/* connection, session */),
        abort_policy: AbortPolicy::default(),  // abort-dependents
        internal: false,                 // external call — ACL against caller identity
    }
}
```

### compose_root_env

The per-call `env` composition (ADR-024) builds a `CompositeOperationEnv` from:
- Layer 0: `LocalOperationEnv` (curated registry)
- Layer 1: session overlay (if active, from `session_source.overlay_for()`)
- Layer 2: connection overlay (from `CallConnection.overlay_env()`)

```rust
fn compose_root_env(&self, connection: &CallConnection, context: &OperationContext) -> Arc<dyn OperationEnv + Send + Sync> {
    let base = Arc::new(LocalOperationEnv { registry: self.registry.clone() });
    let session = self.session_source.as_ref()
        .and_then(|s| s.overlay_for(context));
    let connection_overlay = connection.overlay_env();
    Arc::new(CompositeOperationEnv { session, connection: Some(connection_overlay), base })
}
```

### operationId normalization

The `call.requested` payload's `operationId` has a leading slash (`/fs/readFile`).
The CallAdapter strips it before registry lookup (`fs/readFile`). This is a
single rule applied consistently — the registry stores names without leading
slash, the wire format adds it.

### ResponseEnvelope → EventEnvelope

The CallAdapter converts `ResponseEnvelope` (from local dispatch) to
`EventEnvelope` for the wire:

| `ResponseEnvelope` | `EventEnvelope` |
|--------------------|-----------------|
| `Ok(value)` | `{ type: "call.responded", id: request_id, payload: { output: value } }` |
| `Err(call_error)` | `{ type: "call.error", id: request_id, payload: <serialized CallError> }` |

For subscriptions, each `call.responded` is a separate `EventEnvelope` with the
same `id`; `call.completed` is `{ type: "call.completed", id, payload: {} }`.

### Timeout handling

- Default timeout for wire calls is 30 seconds (`default_timeout`)
- `build_root_context` sets `OperationContext.deadline` to `now + default_timeout`
- Composed calls inherit the parent's deadline (children do NOT get a fresh 30s)
- A composed call that exceeds the deadline is cancelled and returns
  `CallError { code: "TIMEOUT", retryable: true }`
- Subscriptions default to no deadline (`deadline: None` — unbounded); the
  client can specify a timeout in the `call.requested` payload
- The `PendingRequestMap` sweeper runs every 10 seconds and removes expired
  wire entries

### Error handling in handle()

- If a handler panics, the stream is closed and the PendingRequestMap entry is
  cleaned up by the next sweeper pass. Other streams and the connection are
  unaffected.
- Connection drop: all pending requests failed with `call.error` code
  `INTERNAL` and message `"connection closed"`. All subscription channels
  closed. `handle()` returns `Ok(())` (clean) or `Err(ConnectionClosed)`.
- Stream reset: `FrameFramedReader` returns an error. If subscription, remove
  PendingRequestMap entry, close mpsc. If call, resolve oneshot with error. No
  `call.aborted` sent — stream is gone.

## Acceptance Criteria

- [ ] `CallAdapter` struct with registry, identity_provider, session_source, default_timeout
- [ ] `CallAdapter::new()`, `with_session_source()`, `with_timeout()` constructors
- [ ] `SessionOverlaySource` trait defined with `overlay_for()` method
- [ ] `ProtocolHandler::alpn()` returns `b"alknet/call"`
- [ ] `handle()` accepts streams, reads EventEnvelope frames, dispatches
- [ ] `handle()` spawns task for continuous `accept_bi()`
- [ ] Outgoing calls: open_bi, send call.requested, add to PendingRequestMap
- [ ] Identity resolution: AuthContext.identity used, auth_token overrides per-request
- [ ] auth_token resolution failure → proceed with connection-level identity
- [ ] `build_root_context` sets internal: false, deadline, capabilities from registration
- [ ] `compose_root_env` builds CompositeOperationEnv (base + session + connection)
- [ ] operationId leading slash stripped before registry lookup
- [ ] ResponseEnvelope → EventEnvelope conversion (Ok → responded, Err → error)
- [ ] Subscriptions: multiple call.responded with same id, then call.completed
- [ ] Timeout: 30s default, composed calls inherit parent deadline
- [ ] Handler panic: stream closed, PendingRequestMap cleaned up, others unaffected
- [ ] Connection drop: fail all pending with INTERNAL, return Ok or Err
- [ ] Unit test: CallAdapter alpn returns b"alknet/call"
- [ ] Integration test: call.requested → dispatch → call.responded round-trip
- [ ] Integration test: auth_token overrides connection-level identity
- [ ] Integration test: Internal op called from wire → NOT_FOUND
- [ ] Integration test: ACL denied → FORBIDDEN
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call` succeeds with no warnings

## References

- docs/architecture/crates/call/call-protocol.md — CallAdapter, stream handling, root context
- docs/architecture/crates/call/operation-registry.md — OperationContext construction
- docs/architecture/decisions/015-privilege-model-and-authority-context.md — ADR-015 (internal: false for wire)
- docs/architecture/decisions/024-operation-registry-layering.md — ADR-024 (env composition)
- docs/architecture/decisions/012-call-protocol-stream-model.md — ADR-012

## Notes

> This is the merge point of the registry and protocol strands — the highest-
> risk task in the call crate. It ties together stream handling, identity
> resolution, root context construction, env composition, and dispatch. The
> per-request identity resolution (auth_token overrides connection-level) is
> important — a single connection can upgrade auth mid-session. The
> compose_root_env builds the CompositeOperationEnv per call from the active
> layers. operationId on the wire has a leading slash; strip it before lookup.

## Summary

> To be filled on completion