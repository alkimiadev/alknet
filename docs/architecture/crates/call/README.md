---
status: draft
last_updated: 2026-06-21
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
| [008](../../decisions/008-secret-service-integration.md) | Vault Integration Point | Vault accessed at assembly layer, not on the wire |
| [010](../../decisions/010-alpn-router-and-endpoint.md) | ALPN Router and Endpoint | Static handler registration |
| [012](../../decisions/012-call-protocol-stream-model.md) | Call Protocol Stream Model | Bidirectional streams, EventEnvelope, ID-based correlation |
| [014](../../decisions/014-secret-material-flow-and-capability-injection.md) | Secret Material Flow and Capability Injection | Call protocol carries no secret material; capabilities injected at assembly layer |
| [015](../../decisions/015-privilege-model-and-authority-context.md) | Privilege Model and Authority Context | `internal` = authority switch not ACL skip; External/Internal visibility; handler identity + scoped env |
| [016](../../decisions/016-abort-cascade-for-nested-calls.md) | Abort Cascade for Nested Calls | `call.aborted` cascades to descendants; default `abort-dependents`, `continue-running` opt-in |
| [017](../../decisions/017-call-protocol-client-and-adapter-contract.md) | Call Protocol Client and Adapter Contract | `CallClient` opens connections; `from_call` imports remote ops; connection direction independent of call direction |

## Relevant Open Questions

| OQ | Title | Status | Relevance |
|----|-------|--------|-----------|
| OQ-07 | Call protocol scope within a connection | resolved (ADR-012) | Stream model, multiplexing, scope |
| OQ-13 | Operation path format and routing scope | resolved | `/{service}/{op}` is the correct design; remote dispatch is a separate layer |
| OQ-14 | Batch operation semantics | resolved | Correlated `call.requested` events is the correct protocol design |
| OQ-16 | Safe vault operations for call protocol exposure | resolved (ADR-014) | None exposed for now |
| OQ-19 | Session-scoped operation registries | open | Agent-written operations overlaid on global registry via `OperationEnv` trait layering. Protocol doesn't need changes; one-way door is not closing the trait-based composition point |

## Key Design Principles

1. **One connection, full access**: An `alknet/call` connection gives access to the entire operation registry — calls, subscriptions, batch, schema.
2. **Protocol is symmetric**: Both sides can initiate calls. The server calling a client uses the same EventEnvelope format and correlation.
3. **Stream-agnostic correlation**: PendingRequestMap correlates by request ID, not by stream. The protocol works with any stream arrangement.
4. **Operation registry is static**: Operations are registered at startup by the CLI binary. The registry supports JSON Schema discovery.
5. **irpc is one dispatch backend**: Local operations dispatch directly. irpc service calls (in-process, type-safe) are internal. The call protocol is the external interface.
6. **Local dispatch only**: The operation registry dispatches to local handlers. Remote dispatch (federation, head/worker routing) would be a separate mechanism at a different layer, not a modification to alknet-call's path format.
7. **No secret material on the wire**: The call protocol carries no private keys, API keys, mnemonics, or decrypted credentials. Handlers receive outbound credentials through `OperationContext.capabilities`, injected at the assembly layer. See ADR-014.
8. **Abort cascades to descendants**: `call.aborted` for a parent request cascades to all non-terminal descendants. Default `abort-dependents`; `continue-running` opt-in. See ADR-016.
9. **Internal calls switch authority context, not skip ACL**: The `internal` flag marks composition-originated calls. ACL runs against the handler's identity, not the caller's and not as a blanket skip. Operations have External/Internal visibility. Scoped composition env bounds reachability. See ADR-015.