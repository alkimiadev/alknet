# ADR-024: Bidirectional Call Protocol

## Status
Accepted

## Context

The alknet control channel (ADR-018) routes from client → server's event bus.
This is unidirectional: clients can send events to the server, but the server
cannot call operations on the client. In the hub/spoke model, spokes (dev env
containers) connect to a hub and expose operations (fs, bash, search) that the
hub invokes. The hub needs to call *spoke* operations.

Additionally, the current control channel provides no request/response semantics.
Every consumer that needs call/response reinvents the pending-request correlation.

## Decision

The call protocol is bidirectional. Both sides can send `call.requested` and
receive `call.responded`. The protocol uses `EventEnvelope` wire format (4-byte
BE length prefix + JSON) — the same as `@alkdev/pubsub`.

Five event types: `call.requested`, `call.responded`, `call.completed`,
`call.aborted`, `call.error`.

A call is a subscribe that resolves after one event. Both use `call.requested`
with correlated `requestId`. `PendingRequestMap` in core provides correlation.

Operation names use slash-based paths: `/{spoke}/{service}/{op}`. The first
path segment routes the call to the correct connected node. The hub's registry
maps spoke prefixes to connections. This mirrors iroh's ALPN dispatch: the
first segment is the routing key, remaining path dispatches within the node.

Core-provided operations use short paths without a spoke prefix
(`/services/list`, `/services/schema`). Spoke operations are prefixed
(`/dev1/fs/readFile`).

This generalizes ADR-018's control channel: the `alknet-*` destination becomes
a transport for `EventEnvelope` frames with call protocol semantics, instead of
raw pubsub dispatch.

## Consequences

- **Positive**: Hub can invoke operations on spokes. Dev env containers
  expose fs, bash, search — the hub calls them as needed.
- **Positive**: Browser clients can expose custom UDFs. Any connected participant
  can both call and serve operations.
- **Positive**: Built-in request/response correlation. One `PendingRequestMap`
  in core serves all consumers.
- **Positive**: Slash-based paths align with URL routing, OpenAPI, MCP, and
  iroh's ALPN dispatch. First segment = routing key.
- **Positive**: Multiple spokes exposing the same service (two dev envs both
  exposing `/fs/*`) are naturally differentiated by the spoke prefix.
- **Negative**: The `PendingRequestMap` adds in-memory state. Entries must be
  cleaned up on timeout or connection close.
- **Negative**: The hub must maintain a routing table mapping spoke identities
  to connections, with registration on connect and cleanup on disconnect.

## References

- [call-protocol.md](../call-protocol.md) — Full call protocol spec
- [ADR-018](018-control-channel-for-pubsub.md) — Control channel (generalized)
- [napi-and-pubsub.md](../napi-and-pubsub.md) — NAPI wrapper and pubsub adapter