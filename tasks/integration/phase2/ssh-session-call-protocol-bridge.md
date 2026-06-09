---
id: ssh-session-call-protocol-bridge
name: Bridge SshSession recv/send to call protocol via alknet-control:0 channel
status: completed
depends_on: [stream-interface-message-interface-split]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement `SshSession::recv()` and `SshSession::send()` to bridge SSH channel data to and from the call protocol's `InterfaceEvent`/`EventEnvelope` frames. Currently both methods are stubs: `recv()` always returns `None` and `send()` silently discards.

Per the integration plan section 2.1 and interface.md (OQ-IF-01, resolved):

The bridge works as follows:
- When `SshHandler::channel_open_direct_tcpip` detects a destination starting with `alknet-`, it currently accepts the channel but doesn't bridge the data. The `ControlChannelRouter` exists in `control_channel.rs` but has no handler wired.
- `SshSession::recv()` should read `EventEnvelope` frames from the `alknet-control:0` channel stream (using the 4-byte length prefix + JSON wire format from `call::frame::{encode, decode}`), wrap them in `InterfaceEvent` with the session's `Identity` (obtained during SSH auth).
- `SshSession::send()` should write `EventEnvelope` frames to the `alknet-control:0` channel stream using the same framing format.
- The `ControlChannelRouter` should be wired to bridge incoming channel data to the call protocol handler.

**Current state of the code**:
- `SshSession::recv()` returns `None` (stub)
- `SshSession::send()` discards silently (stub)
- `ControlChannelRouter` in `control_channel.rs` has `route()` and `has_handler()` but no handler is registered
- `call::frame::{encode, decode}` functions exist and are well-tested (4-byte BE length prefix + JSON)
- `SshHandler` detects `alknet-*` destinations in `channel_open_direct_tcpip` but doesn't bridge data
- `SshHandler` stores `authenticated_identity: Option<Identity>` from SSH auth
- `InterfaceEvent` struct carries `EventEnvelope` + `Option<Identity>` — already defined

**Key design considerations**:
- The `SshSession` needs access to the SSH channel's data stream to read/write `EventEnvelope` frames. This requires getting the `russh::Channel` data stream and framing it.
- The `ControlChannelRouter` currently uses `Box<dyn DuplexStream>` — it can be wired as a `ControlChannelHandler` that reads frames from the stream and produces `InterfaceEvent`s.
- The `alknet-control:0` channel is the first SSH direct-tcpip channel with the `alknet-control` destination. Additional `alknet-*` channels may follow.
- The session's `Identity` (from SSH auth) must be attached to every `InterfaceEvent` produced by `recv()`.

## Acceptance Criteria

- [ ] `SshSession::recv()` reads `EventEnvelope` frames from the SSH channel data stream and produces `InterfaceEvent` with the session's `Identity`
- [ ] `SshSession::send()` writes `EventEnvelope` frames to the SSH channel data stream using `call::frame::encode`
- [ ] `ControlChannelRouter` is wired as the handler for `alknet-control:0` channels, bridging SSH channel data to the call protocol
- [ ] Frame encoding matches `call::frame::{encode, decode}` — 4-byte big-endian length prefix + UTF-8 JSON body
- [ ] The session's `Identity` (from `SshHandler::authenticated_identity`) is attached to every `InterfaceEvent` produced by `recv()`
- [ ] `SshHandler::channel_open_direct_tcpip` correctly routes `alknet-control:0` channels to the `ControlChannelRouter` handler
- [ ] Unit test: `SshSession` can round-trip an `EventEnvelope` through `send()` and `recv()` (using a mock channel stream)
- [ ] Unit test: `ControlChannelRouter.with_handler()` successfully routes channel data
- [ ] All existing server/auth/transport tests continue to pass
- [ ] No behavioral changes for non-`alknet-*` channel forwarding (port proxy logic unchanged)

## References

- docs/research/integration-plan.md — Phase 2.1
- docs/architecture/interface.md — OQ-IF-01 resolution, InterfaceEvent model
- docs/architecture/call-protocol.md — EventEnvelope, frame encoding
- crates/alknet-core/src/interface/ssh.rs — SshSession stubs (recv/send)
- crates/alknet-core/src/server/control_channel.rs — ControlChannelRouter
- crates/alknet-core/src/call/frame.rs — frame encode/decode
- crates/alknet-core/src/interface/session.rs — InterfaceEvent, InterfaceSession traits

## Notes

> This is the highest-risk task in Phase 2. The `russh` channel data stream API needs careful handling — getting a `Channel`'s data stream for async reading/writing is non-trivial and may require understanding russh's `data()` callback pattern vs. the `Channel::into_stream()` method.

> Consider implementing incrementally: first wire the `ControlChannelRouter` handler to produce `InterfaceEvent`s from raw channel data, then connect that to `SshSession::recv()`/`send()`. Each step should have passing tests before proceeding.

> The `SshSession` struct currently holds a `server::Handle` and a `JoinHandle`. It may need additional fields to track the control channel stream and the authenticated identity for producing `InterfaceEvent`s with identity attached.

## Summary

> Implemented SshSession recv/send bridge to call protocol via alknet-control:0 channel. Added FrameFramedReader/FrameFramedWriter for async length-prefixed EventEnvelope I/O. SshSession::recv() reads InterfaceEvents from mpsc channel bridged from SSH channel. SshSession::send() writes EventEnvelopes to mpsc channel bridged to SSH channel. ControlChannelBridge implements ControlChannelHandler. SshHandler routes alknet-control:0 channels to bridge task using tokio::select!. Session Identity attached to every InterfaceEvent.