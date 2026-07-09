# ADR-017: Call Protocol Client and Adapter Contract

## Status

Accepted (amended 2026-06-26 — see "Amendments" below)

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
) -> Vec<HandlerRegistration>
```

The adapter:
1. Calls `services/list` on the remote node → gets the list of `External`
   operations
2. Calls `services/schema` for each → gets the input/output JSON Schemas and
   declared error_schemas (ADR-023)
3. For each discovered operation, constructs a `HandlerRegistration` bundle:
   - The spec mirrors the remote operation's name, namespace, type, schemas
     (input, output, and error_schemas — ADR-023), and access control
   - The handler sends `call.requested` through the `CallConnection` and awaits
     `call.responded` (or streams for subscriptions)
   - `provenance: FromCall`, `composition_authority: None`, `scoped_env: None`
     (leaves — ADR-022)
4. The caller registers these bundles in their local registry (into the
   connection's overlay — ADR-024)

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

The adapter patterns share a common shape: they produce
`HandlerRegistration` bundles that register in the local registry. The
trait:

```rust
#[async_trait]
pub trait OperationAdapter: Send + Sync {
    async fn import(&self) -> Vec<HandlerRegistration>;
}
```

The return type is `Vec<HandlerRegistration>` (not `(OperationSpec,
Handler)` pairs) — ADR-022 changed the registration API to the bundle
shape, and adapters must produce bundles. Adapter convenience methods
construct bundles with `composition_authority: None` and `scoped_env: None`
for the leaf ops they produce.

The trait is **async** because `from_call` requires async discovery
(`services/list` + `services/schema` over a QUIC connection). A synchronous
trait cannot accommodate `from_call` without a separate async pre-step that
populates a cache. The sync adapters (`from_openapi`, `from_mcp` reading a
static spec) trivially satisfy an async trait — their `import()` bodies
contain no `.await` points. The async/sync question is decided: the trait
is async.

Implementations:
- `FromOpenAPI` — imports from an OpenAPI spec (HTTP-backed handlers)
- `FromMCP` — imports from an MCP server (MCP-backed handlers)
- `FromCall` — imports from a remote call protocol endpoint
  (call-protocol-backed handlers)
- ~~`FromJsonSchema` — imports from a JSON Schema definition (schema-only, no
  handler — used for validation or client generation)~~ — **superseded by
  [ADR-066](066-from-jsonschema-as-http-adapter.md)**: `from_jsonschema` is
  now an HTTP-backed single-endpoint adapter in `alknet-http` (reqwest
  forwarding handler, not a schema-only placeholder); `FromJsonSchema`
  provenance stays in `alknet-call` as a handler-bearing leaf.

The `to_*` adapters are outbound projections, not `OperationAdapter`
implementations — they consume the registry, they don't produce entries for it.

The specific trait signatures (error types, configuration parameters) are
two-way doors for implementation. The one-way doors are the architectural
commitments: adapters produce `HandlerRegistration` bundles (ADR-022), the
trait is async (required by `from_call`), and the adapter *trait* lives in
`alknet-call` while adapter *implementations* live with their transport
(HTTP-backed adapters in `alknet-http` per ADR-066; QUIC-backed `from_call`
in `alknet-call`). See `client-and-adapters.md` §"Adapter Location Map."

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
- **Published `to_*` specs are compatibility contracts.** The "best-effort"
  mapping label is internal framing. Once a generated spec is published and
  external clients build against it, the mapping semantics (e.g.,
  subscriptions → SSE long-poll) become a de facto contract. Changing the
  mapping later breaks every client. `to_*` mapping choices are two-way
  *before* first publication but one-way *after*. Version the generated
  specs (e.g., OpenAPI spec version tied to the registry's External
  operation set version) and emit a spec version marker so consumers can
  detect mapping changes. This is the "published artifact is a contract"
  blind spot in ADR-009's framework: it classifies doors by reversal cost
  in the codebase, not by compatibility cost for external consumers.
- **Sharing the global registry with a `CallClient` exposes local
  capabilities to the remote peer.** Each `HandlerRegistration` carries
  `Capabilities` with secret material. If the `CallClient` shares the
  global registry, a remote peer calling an External operation triggers
  dispatch that populates `OperationContext.capabilities` from the local
  registration bundle — meaning the local node's API keys and signing keys
  are used for the remote peer's call. A peer-scoped subset must filter by
  capability remote-safety (is this operation's capability safe to expose
  to this peer?), not just operation name. The registry-mechanism choice
  (share global vs subset vs separate) is two-way mechanically but has a
  security dimension post-ADR-022: the "share global" option is a
  capability-exposure decision, not just a dispatch decision.
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
   `OperationSpec` has the same name, namespace, type, schemas (input, output,
   and error_schemas per ADR-023), and access control as the remote operation. If the remote operation changes (new
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
- ADR-028: Peer-Scoped Registry Filtering for CallClient Inbound Dispatch
  (resolves the §1 Consequences security dimension flagged as a two-way door)
- OQ-15: Call protocol client and adapter contract (resolved by this ADR)
- OQ-25..28: Two-way-door remainders from the call-completion gap analysis
  (DC-1 shape, DC-4 error type, DC-2 re-import trigger, DC-3 namespace
  collision — see [open-questions.md](../open-questions.md))
- [call-protocol.md](../crates/call/call-protocol.md)
- [operation-registry.md](../crates/call/operation-registry.md)
- [client-and-adapters.md](../crates/call/client-and-adapters.md) — the spec
  that operationally fills the gap this ADR left to implementation
- `docs/research/alknet-call-completion/gap-analysis.md` — DC-1..4, the
  decisions that needed resolution before implementation
- TypeScript `@alkdev/operations` — `from_openapi`, `from_mcp`, `buildEnv`
  prior art
- POC at `/workspace/@alkdev/dispatch` — head/worker dispatch over SSH+axum

## Amendments (2026-06-26)

This ADR left four decisions as two-way doors (§1 Consequences flagged DC-1's
security dimension; §5 noted trait signatures are two-way doors; Assumption 4
noted re-import hot-swap is a two-way door; §3 mentioned the namespace prefix).
The call-completion gap analysis (`docs/research/alknet-call-completion/gap-analysis.md`
DC-1..4) resolved them. The resolutions:

### DC-1 — CallClient registry scope: resolved by ADR-028, superseded by ADR-029

The §1 Consequences security dimension was originally resolved by ADR-028
(default-deny `remote_safe: bool` + `trusted_peer` opt-in). **ADR-028 is now
superseded by [ADR-029](029-peer-graph-routing-model.md)** (2026-06-27):
the flat-namespace single-peer model ADR-028 built on cannot express the
head→N-workers pattern, and the `remote_safe`/`trusted_peer` gate duplicates
the existing `AccessControl`/`Identity` machinery while reintroducing the
blanket-bypass anti-pattern ADR-015 killed. ADR-029 replaces the flat overlay
with peer-keyed overlays + `PeerRef` routing, and retires `remote_safe`/
`trusted_peer` in favor of `AccessControl::check(peer_identity)` — the
existing authorization path that was already in the dispatch path. The peer-
scoping question this section flagged is now answered structurally (peer-keyed
overlays), not by a parallel boolean gate.

### DC-4 — OperationAdapter trait error type: resolved

§5 showed `async fn import(&self) -> Vec<HandlerRegistration>` with no error
type. The trait returns `Result<Vec<HandlerRegistration>, AdapterError>`
where `AdapterError` is a crate-level enum. The *presence* of the error type
is recorded in [client-and-adapters.md](../crates/call/client-and-adapters.md);
the exact variants are the two-way-door remainder, tracked as OQ-26.

### DC-2 — from_call re-import on reconnection: default set

Assumption 4 noted re-import "happens on reconnection or is triggered
explicitly." The v1 default is **auto-re-import on connection establishment**.
The overlay is per-connection (Layer 2, ADR-024), so re-import is naturally
scoped; a stale overlay dies with the connection. Explicit re-import via a
future `CallConnection::refresh()` is additive. Two-way door; recorded in
[client-and-adapters.md](../crates/call/client-and-adapters.md); tracked as
OQ-27.

### DC-3 — from_call namespace collision: default set

§3's `FromCallConfig` namespace prefix is **optional, default no prefix,
collision = error**. A node importing from two remotes that both expose the
same unprefixed op name should fail loudly. The operator adds prefixes when
importing from multiple sources. Two-way door; recorded in
[client-and-adapters.md](../crates/call/client-and-adapters.md); tracked as
OQ-28.

### Operational spec

The gap this ADR left to implementation — the `CallClient` API, the
`from_call` flow, the trait signature, the adapter location map, the
no-env-vars invariant, and the exchange-of-operations pattern — is
specified in
[client-and-adapters.md](../crates/call/client-and-adapters.md). That document
is the operational complement to this ADR; this ADR remains the architectural
authority.

## Amendments (2026-07-09)

### `from_jsonschema` clause superseded by ADR-066

The §5 `FromJsonSchema` implementation listing ("schema-only, no handler")
is **superseded by [ADR-066](066-from-jsonschema-as-http-adapter.md)**.
`from_jsonschema` is now an HTTP-backed single-endpoint adapter in
`alknet-http` (reqwest forwarding handler, same shape as `from_openapi`),
not a schema-only placeholder in `alknet-call`. The `FromJsonSchema`
provenance variant stays in `alknet-call` (`OperationProvenance`) but is
now a handler-bearing leaf, not a "no handler" entry. The "schema-only,
no handler" concept is removed — schema validation without a handler is
served by consuming `OperationSpec` directly. The §5 "adapters live in
alknet-call" one-way-door statement is corrected above to "the adapter
trait lives in `alknet-call`; implementations live with their transport."
See [ADR-066](066-from-jsonschema-as-http-adapter.md) and
[client-and-adapters.md](../crates/call/client-and-adapters.md) §"from_jsonschema".