# ADR-018: Control Channel for PubSub over SSH

## Status
Accepted

## Context
The NAPI wrapper and pubsub integration need a way to use alknet's SSH channel as a data plane for event routing. When a `alknet connect` client opens an SSH session to a server, the `direct_tcpip` channel type is used to reach specific TCP targets (host:port).

For the pubsub use case, the client needs a dedicated bidirectional stream to the server's event bus — not a TCP connection to a random host. There are several approaches:

1. **Special destination**: Use `direct_tcpip` with a reserved destination (e.g., `alknet-control:0`) that the server recognizes and routes internally instead of connecting to a TCP target.
2. **Port forwarding**: The server runs a pubsub hub on a specific port (e.g., 9736) and the client uses normal port forwarding (`-L 9736:hub:9736`).
3. **Custom channel type**: Define a new SSH channel type beyond `direct_tcpip` and `forwarded_tcpip`.

## Decision
Use approach 1: a reserved `direct_tcpip` destination string. When the server receives a `channel_open_direct_tcpip` request for `alknet-control:0`:

1. The `channel_open_direct_tcpip` handler detects the special target via string matching
2. Instead of connecting to a TCP target, it bridges the channel to the local pubsub event bus
3. `EventEnvelope` JSON flows bidirectionally over the SSH channel

The destination string `alknet-control` is reserved. Regular TCP targets are hostnames or IP addresses, so there is no collision risk.

Approach 2 (port forwarding to a specific port) is still supported as an alternative — the client can use `--forward 9736:localhost:9736` if the server runs a pubsub hub on that port. But the control channel approach is simpler and doesn't require a separate listening port.

Approach 3 (custom channel type) was rejected because russh's `direct_tcpip` handler is well-understood and adding custom channel types requires modifying russh.

## Consequences
- **Positive**: Simple implementation — just string matching in the server's `channel_open_direct_tcpip` handler.
- **Positive**: No separate port or service needs to run on the server. The control channel is built into alknet.
- **Positive**: Compatible with the NAPI wrapper's single-duplex-stream model.
- **Positive**: Port forwarding to a specific port is still available as an alternative.
- **Negative**: The string `alknet-control` is a magic constant. It should be defined as a constant in the crate.
- **Negative**: Regular TCP destinations accidentally matching `alknet-control` would be misrouted. Mitigated by reserving the entire `alknet-` prefix namespace.

## References
- [napi-and-pubsub.md](../napi-and-pubsub.md)
- [server.md](../server.md)