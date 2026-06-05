---
id: server/control-channel
name: Implement alknet-control reserved channel for pubsub event bus bridging (ADR-018)
status: pending
depends_on:
  - server/handler
  - auth/error-types
scope: narrow
risk: medium
impact: component
level: implementation
---

## Description

Implement the control channel routing per ADR-018. When the server receives a `channel_open_direct_tcpip` request for `alknet-control:0`:

1. The handler detects the reserved `alknet-` prefix destination
2. Instead of making a TCP connection, it bridges the SSH channel to an internal event bus handle
3. `EventEnvelope` JSON flows bidirectionally over the SSH channel

The entire `alknet-` prefix is reserved — no TCP connections should be attempted for `alknet-*` destinations. The control channel is optional; servers without pubsub configured should accept the channel and provide a configurable behavior (reject or provide a loopback pipe).

At this stage, implement the routing logic and a `ControlChannel` trait that consumers can implement. The actual pubsub bridge implementation would be in a separate crate or behind a feature flag.

## Acceptance Criteria

- [ ] `crates/alknet-core/src/server/control_channel.rs` exports `ControlChannelHandler` trait and routing logic
- [ ] `ALKNET_CONTROL_DESTINATION` constant defined as `"alknet-control"` (ADR-018)
- [ ] `ALKNET_PREFIX` constant defined as `"alknet-"` for namespace reservation
- [ ] `ControlChannelHandler` trait: `async fn handle_channel(stream: Box<dyn AsyncRead + AsyncWrite + Unpin + Send>)`
- [ ] Server handler detects `alknet-*` prefix and routes to `ControlChannelHandler` instead of TCP proxy
- [ ] If no `ControlChannelHandler` configured, reject the channel open request (SSH channel open failure)
- [ ] Non-reserved destinations continue through normal TCP proxy path
- [ ] Server constraint enforced: no TCP connections to `alknet-*` destinations
- [ ] Unit tests: reserved destination detected, non-reserved passes through, prefix matching works

## References

- docs/architecture/server.md — Channel Handling section (reserved destinations), Constraints section
- docs/architecture/decisions/018-control-channel-for-pubsub.md — control channel rationale
- docs/architecture/napi-and-pubsub.md — server-side control channel behavior

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion