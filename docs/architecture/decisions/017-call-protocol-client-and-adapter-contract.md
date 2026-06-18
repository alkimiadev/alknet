# ADR-017: Call Protocol Client and Adapter Contract

## Status

Accepted

## Context

The call protocol spec (ADR-012) defined the stream model as bidirectional —
"both sides can initiate calls." But the spec only described the server side:
`CallAdapter` implements `ProtocolHandler`, accepts incoming QUIC connections,
and dispatches to the operation registry. The client side — who opens the
connection, how calls are sent, how remote operations are discovered and
imported — was left as OQ-15.

The need for the client side is concrete and immediate:

- **Head/worker dispatch**: a head node manages worker nodes (Vast.ai, RunPod,
  local Docker). The head needs to call operations on workers (exec, sync,
  status) and workers need to call back (report status, request work). The
  POC at `/workspace/@alkdev/dispatch` demonstrated this over SSH+axum; under
  the call protocol, it's cross-node composition.
- **NAPI/Python adapters**: Node.js and Python clients need to call operations
  on an alknet node. They speak the EventEnvelope wire format over a QUIC
  connection.
- **Agent tool dispatch**: an agent handler needs to call operations on remote
  nodes (tools, services) the same way it calls local operations — through
  `OperationEnv::invoke()`. The `from_call` adapter makes remote operations
  appear in the local registry.
- **Cross-protocol interop**: external systems (HTTP APIs, MCP servers) are
  imported via `from_openapi` and `from_mcp`. The reverse direction —
  exposing local operations to external systems — needs `to_openapi` and
  `to_mcp`.

The `@alkdev/operations` TypeScript package demonstrated the adapter patterns
(`from_openapi`, `from_mcp`) and the `buildEnv` composition mechanism. The Rust
implementation defines the canonical traits (ADR-013).

OQ-15 was constrained by ADR-014 (adapters take credential sources, not static
tokens) and ADR-015 (adapter-registered operations are `Internal` by default).
This ADR locks the remaining one-way door: the client/adapter contract
architecture.

## Decision

### 1. `CallClient` opens connections and shares the dispatch loop

`CallClient` opens a QUIC connection to a remote node with ALPN `alknet/call`.
Once connected, the connection is symmetric — both sides can send and receive
`call.requested`. The `CallClient` is not just a caller; it is also a callee.
It has its own operation registry to dispatch incoming calls from the remote
side.

```rust
pub struct CallClient {
    registry: Arc<OperationRegistry>,
    identity_provider: Arc<dyn IdentityProvider>,
}

impl CallClient {
    pub async fn connect(&self, addr: SocketAddr, credentials: CallCredentials) -> Result<CallConnection>;
}
```

The dispatch loop is shared between `CallAdapter` and `CallClient`. Once a
connection is established (whether accepted by the adapter or opened by the
client), the same logic applies: read `EventEnvelope` frames, dispatch to the
operation registry, write responses, and send outgoing `call.requested` events
for calls initiated on this side. The only difference is who opened the
connection.

`CallConnection` provides:
- `call(operation_id, input) -> ResponseEnvelope` — send `call.requested`,
  await `call.responded` (one result)
- `subscribe(operation_id, input) -> Stream<ResponseEnvelope>` — send
  `call.requested`, yield each `call.responded` until `call.completed` or
  `call.aborted`
- `abort(request_id)` — send `call.aborted`, cascade to descendants (ADR-016)
- `services_list() -> Vec<OperationSpec>` — call `services/list`
- `services_schema(name) -> OperationSpec` — call `services/schema`

### 2. Connection direction is independent of call direction

Who opens the QUIC connection (who has the public IP, who uses a relay, who
connects out reverse-runner style) is a connection-layer concern, not a
protocol-layer concern. Once connected, both sides can call each other.

| Topology | Who advertises | Who opens connection | Who can call whom |
|----------|---------------|----------------------|-------------------|
| Public service | Server (public IP/domain) | Client | Both directions |
| P2P (iroh relay) | Both (relay-assisted) | Either | Both directions |
| Reverse (runner pattern) | Head (public IP) | Worker connects out | Both directions |
| Reverse (dispatch pattern) | Worker (public SSH port) | Head connects out | Both directions |

The protocol does not distinguish "server" and "client" after connection
establishment. The `CallAdapter` accepts connections; the `CallClient` opens
connections. Both dispatch incoming and outgoing calls through the same
mechanism.

### 3. `from_call` adapter imports remote operations

`from_call` does for call protocol endpoints what `from_openapi` does for HTTP
APIs: discovers operations and registers them in the local registry with
forwarding handlers.

```rust
pub async fn from_call(
    connection: &CallConnection,
    config: FromCallConfig,
) -> Vec<(OperationSpec, Handler)>
```

The adapter:
1. Calls `services/list` on the remote node → gets the list of `External`
   operations
2. Calls `services/schema` for each → gets the input/output JSON Schemas
3. For each discovered operation, constructs an `(OperationSpec, Handler)` pair:
   - The spec mirrors the remote operation's name, namespace, type, schemas,
     and access control
   - The handler sends `call.requested` through the `CallConnection` and awaits
     `call.responded` (or streams for subscriptions)
4. The caller registers these pairs in their local registry

`from_call`-registered operations are `Internal` by default (ADR-015) — they
are composition material, not directly callable from the wire. The handler
that composes them is `External`.

The `FromCallConfig` includes:
- The credential source for the outbound connection (ADR-014) — TLS identity,
  auth token, or capability-provided credentials
- An optional namespace prefix (to avoid collisions when importing from
  multiple remote nodes)
- An optional operation filter (to import only specific operations)

### 4. `to_openapi` and `to_mcp` adapters export local operations

The reverse direction — exposing local operations to external systems:

- **`to_openapi`**: generates an OpenAPI spec from the local registry's
  `External` operations. External systems (HTTP clients, API gateways) can
  discover and call alknet operations through a standard HTTP interface.
- **`to_mcp`**: exposes local operations as MCP tools. MCP clients (editors,
  AI tools) can discover and call alknet operations through the MCP protocol.

These adapters are outbound bridges — they translate the call protocol's
operation model into external protocol formats. They do not modify the local
registry; they project it.

### 5. The adapter contract trait

The adapter patterns share a common shape: they produce `(OperationSpec,
Handler)` pairs that register in the local registry. The trait:

```rust
pub trait OperationAdapter: Send + Sync {
    fn import(&self) -> Vec<(OperationSpec, Handler)>;
}
```

Implementations:
- `FromOpenAPI` — imports from an OpenAPI spec (HTTP-backed handlers)
- `FromMCP` — imports from an MCP server (MCP-backed handlers)
- `FromCall` — imports from a remote call protocol endpoint
  (call-protocol-backed handlers)
- `FromJsonSchema` — imports from a JSON Schema definition (schema-only, no
  handler — used for validation or client generation)

The `to_*` adapters are outbound projections, not `OperationAdapter`
implementations — they consume the registry, they don't produce entries for it.

The specific trait signatures (async vs sync, error types, configuration
parameters) are two-way doors for implementation. The one-way door is the
architectural commitment that adapters produce `(OperationSpec, Handler)`
pairs and live in alknet-call.

### 6. Cross-node call tree and abort cascade

When a `from_call` handler sends `call.requested` to a remote node, the call
participates in the local call tree via `parent_request_id`. If the parent is
aborted, the cascade (ADR-016) reaches the `from_call` handler, which sends
`call.aborted` to the remote node. The remote node cascades to its own
descendants. The abort crosses the node boundary transparently.

```
Head node                                    Worker node
  r1: /dispatch/run_training
    r1-a: worker/exec (from_call handler)
      → call.requested { id: r1-a } ────────→ receives, dispatches to exec
                                                r1-a-1: exec spawns child
  user aborts r1
    cascade to r1-a
      from_call handler sends:
        call.aborted { id: r1-a } ───────────→ receives, cascades to r1-a-1
                                                  aborts exec and children
```

### 7. Credential sources for connections

The `CallClient` needs credentials to authenticate to the remote node. These
come from capabilities (ADR-014), not environment variables. The credential
types:

- **TLS identity**: the local node's Ed25519 key (RFC 7250 raw key) or X.509
  cert, derived from the vault at startup
- **Auth token**: an opaque token for call-protocol-level authentication,
  decrypted from the vault or derived from a shared secret
- **Remote identity verification**: the expected fingerprint or cert of the
  remote node, stored as a capability (not an env var or config file)

The `from_call` adapter receives these credentials at registration time,
same as `from_openapi` receives HTTP credentials.

## Consequences

**Positive:**
- Cross-node composition works the same as local composition. A handler calls
  `env.invoke("worker", "exec", ...)` and doesn't know (or care) whether
  `worker/exec` is a local operation or a `from_call`-imported remote
  operation. The composition is transparent.
- The head/worker pattern (dispatch, runners) is a connection topology, not a
  protocol feature. Workers can connect to heads (runner pattern) or heads can
  connect to workers (dispatch pattern) — the protocol handles both.
- `from_call` is the same pattern as `from_openapi` and `from_mcp`: discover,
  register, forward. The adapter contract is unified.
- `to_openapi` and `to_mcp` enable interop with non-alknet systems without
  those systems needing to speak EventEnvelope.
- The abort cascade (ADR-016) crosses node boundaries transparently. No
  consumer needs to implement cross-node abort propagation.
- The NAPI and Python adapters can use `CallClient` directly to call remote
  operations — they don't need a separate client implementation.

**Negative:**
- `CallClient` has its own operation registry (for dispatching incoming calls
  from the remote side). This is a second registry instance, not the global
  one — it needs to be populated with the operations this node wants to expose
  to that specific remote peer. The specific mechanism (sharing the global
  registry, a peer-scoped subset, or a separate registry) is a two-way door.
- `from_call`-registered operations have a latency cost: each invocation sends
  a `call.requested` over QUIC and awaits a `call.responded`. This is
  inherent to remote calls and not specific to the adapter pattern. Caching
  or batching strategies are consumer concerns.
- The `to_*` adapters need to translate the call protocol's operation model
  (JSON Schema, EventEnvelope, subscribe/stream) into external formats
  (OpenAPI paths, MCP tools). Some semantics don't map cleanly (e.g.,
  subscriptions in OpenAPI, bidirectional calls in MCP). The adapters handle
  these with best-effort mappings and document the gaps.
- The `CallConnection` abstraction adds a layer between the handler and the
  raw QUIC stream. This is necessary for the `from_call` handler to be
  transparent — it shouldn't know about QUIC streams, only about call/request
  semantics.

## Assumptions

1. **The connection is symmetric after establishment.** Both sides can send
   and receive `call.requested`. If a future use case requires one-directional
   connections (e.g., a fire-and-forget notification where the receiver can't
   call back), the model needs extension. The assumption is that bidirectional
   is the correct default.

2. **`services/list` and `services/schema` are the discovery mechanism for
   `from_call`.** The remote node exposes its `External` operations through
   these built-in operations. If a remote node doesn't support service
   discovery (e.g., a minimal worker that only accepts specific calls),
   `from_call` needs an alternative discovery mechanism (static config, manual
   spec). The assumption is that nodes participating in cross-node composition
   support service discovery.

3. **The `from_call` handler is transparent to composition.** A handler that
   calls `env.invoke("worker", "exec", ...)` doesn't know it's a remote call.
   If the remote node is unreachable or the connection drops, the handler gets
   a `call.error` (same as a local handler error). The assumption is that
   remote call failures are handled the same as local handler failures.

4. **`from_call`-registered operations mirror the remote spec.** The imported
   `OperationSpec` has the same name, namespace, type, schemas, and access
   control as the remote operation. If the remote operation changes (new
   schema, renamed), the imported spec is stale until re-import. The
   assumption is that re-import happens on reconnection or is triggered
   explicitly. Hot-swapping imported specs is a two-way door.

5. **The `to_*` adapters are projections, not live bridges.** `to_openapi`
   generates a spec; it doesn't proxy HTTP requests. An external HTTP client
   calling the generated OpenAPI endpoints needs an HTTP handler (alknet-http)
   that translates HTTP requests into call protocol operations. The assumption
   is that `to_*` generates specs/tools, and a separate HTTP/MCP handler
   bridges the actual traffic.

## References

- ADR-005: irpc as call protocol foundation
- ADR-012: Call protocol stream model (bidirectional streams)
- ADR-013: Rust as canonical implementation language (adapter traits in Rust)
- ADR-014: Secret material flow (credential sources, not static tokens)
- ADR-015: Privilege model (adapter ops are Internal by default)
- ADR-016: Abort cascade (cross-node abort propagation)
- OQ-15: Call protocol client and adapter contract (resolved by this ADR)
- [call-protocol.md](../crates/call/call-protocol.md)
- [operation-registry.md](../crates/call/operation-registry.md)
- TypeScript `@alkdev/operations` — `from_openapi`, `from_mcp`, `buildEnv`
  prior art
- POC at `/workspace/@alkdev/dispatch` — head/worker dispatch over SSH+axum