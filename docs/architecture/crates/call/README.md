---
status: draft
last_updated: 2026-06-17
---

# alknet-call

Structured RPC over QUIC: operations, request/response, streaming subscriptions, and service discovery. Implements `ProtocolHandler` on ALPN `alknet/call`.

## Documents

| Document | Status | Description |
|----------|--------|-------------|
| [call-protocol.md](call-protocol.md) | draft | CallAdapter, EventEnvelope framing, stream model, PendingRequestMap, bidirectional calls |
| [operation-registry.md](operation-registry.md) | draft | OperationSpec, Handler, OperationRegistry, AccessControl, service discovery, irpc integration |

## Applicable ADRs

| ADR | Title | Relevance |
|-----|-------|-----------|
| [001](../../decisions/001-alpn-protocol-dispatch.md) | ALPN-Based Protocol Dispatch | CallAdapter registers on ALPN `alknet/call` |
| [002](../../decisions/002-protocol-handler-trait.md) | ProtocolHandler Trait | CallAdapter implements ProtocolHandler |
| [003](../../decisions/003-crate-decomposition.md) | Crate Decomposition | alknet-call depends on alknet-core and irpc |
| [004](../../decisions/004-auth-as-shared-core.md) | Auth as Shared Core | AuthContext passed to call handlers |
| [005](../../decisions/005-irpc-as-call-protocol-foundation.md) | irpc as Call Protocol Foundation | irpc provides framing and service dispatch |
| [006](../../decisions/006-alpn-convention-and-connection-model.md) | ALPN String Convention | `alknet/call` ALPN, one ALPN per connection |
| [007](../../decisions/007-bistream-type-definition.md) | BiStream Type Definition | CallAdapter receives Connection, not BiStream |
| [008](../../decisions/008-secret-service-integration.md) | Vault Integration Point | Vault operations exposed via call protocol |
| [012](../../decisions/012-call-protocol-stream-model.md) | Call Protocol Stream Model | Bidirectional streams, EventEnvelope, ID-based correlation |

## Relevant Open Questions

| OQ | Title | Status | Relevance |
|----|-------|--------|-----------|
| OQ-07 | Call protocol scope within a connection | resolved (ADR-012) | Stream model, multiplexing, scope |
| OQ-13 | Operation path format and routing scope | open | Namespace paths: `/{service}/{op}` vs `/{node}/{service}/{op}` |
| OQ-14 | Batch operation semantics | open | Whether batch is a protocol primitive or client-side pattern |

## Key Design Principles

1. **One connection, full access**: An `alknet/call` connection gives access to the entire operation registry — calls, subscriptions, batch, schema.
2. **Protocol is symmetric**: Both sides can initiate calls. The server calling a client uses the same EventEnvelope format and correlation.
3. **Stream-agnostic correlation**: PendingRequestMap correlates by request ID, not by stream. The protocol works with any stream arrangement.
4. **Operation registry is dynamic**: Operations are registered at startup by the CLI binary. The registry supports JSON Schema discovery.
5. **irpc is one dispatch backend**: Local operations dispatch directly. irpc service calls (vault, auth) are internal. The call protocol is the external interface.
6. **Phase 1 is local dispatch only**: The operation registry dispatches to local handlers. Remote dispatch (head/worker routing) and irpc service dispatch are contracted but not built yet.