---
id: core/ssh-interface-extraction
name: Extract SshInterface from ServerHandler — refactor SSH into Layer 2
status: completed
depends_on:
  - core/interface-trait-definition
  - core/config-identity-provider-into-handler
scope: broad
risk: high
impact: project
level: implementation
---

## Description

This is the most invasive change in Phase 1. Extract SSH session handling from `ServerHandler` into `SshInterface`, making it implement the `Interface` trait defined in the previous task. Per interface.md and ADR-026, SshInterface becomes one implementation of the Interface trait alongside RawFramingInterface.

**Current state**: `ServerHandler` is a `russh::server::Handler` that owns auth, channel management, and proxy logic — all tangled together.

**Target state**:
- `SshInterface` implements `Interface` and wraps the SSH handshake and session management
- Auth delegation goes through `IdentityProvider` (already wired from task 1.5)
- Channel open requests (including forwarding policy checks) are Layer 3 concerns routed through the Interface session
- Port forwarding proxy logic is per-connection state managed by the session

**Key changes**:
- `SshInterface` wraps `russh` server config and session handling
- `SshInterface::accept()` takes a `TransportStream` and `SshInterfaceConfig`, performs SSH handshake, returns a session
- The session produces call protocol events (for `alknet-control:0` channels) and handles channel routing
- Forwarding policy check is invoked from Layer 3 (call protocol handler), not embedded in the interface
- `RawFramingInterface` stub: just the type definition, no implementation (Phase 4+ for DNS and WebTransport)
- The server accept loop uses `(Transport, Interface)` pairs instead of directly spawning SSH handlers

**This is explicitly flagged as high-risk** in the integration plan. The refactoring must preserve all existing behavior. Strategy: implement `SshInterface` as a wrapper around the existing `ServerHandler` logic initially, then progressively move concerns to the right layer.

## Acceptance Criteria

- [ ] `SshInterface` struct implements `Interface` trait with `accept()` method
- [ ] `SshInterface::accept()` performs SSH handshake over `TransportStream`
- [ ] Auth is delegated through `IdentityProvider` (not embedded in SshInterface)
- [ ] Channel open requests route through the session to Layer 3 (forwarding policy, call protocol)
- [ ] `alknet-control:0` channels produce call protocol events via the session
- [ ] Port forwarding proxy logic is per-connection state, not embedded in the interface
- [ ] `RawFramingInterface` type exists but is stub-only (no implementation beyond type definition)
- [ ] Server accept loop uses `(Transport, Interface)` pairs to spawn sessions
- [ ] All existing server/auth/transport tests pass unchanged
- [ ] New tests: `SshInterface` session over TCP and TLS transports
- [ ] New tests: forwarding policy applied when channels are opened (Layer 3, not Layer 2)

## References

- docs/architecture/interface.md — SshInterface, what stays in Layer 2 vs Layer 3, OQ-IF-02
- docs/architecture/decisions/026-transport-interface-separation.md — ADR-026
- crates/alknet-core/src/server/handler.rs — current ServerHandler to be extracted

## Notes

> This is the highest-risk task in Phase 1. Consider implementing incrementally: first wrap existing ServerHandler in SshInterface, then progressively move auth to IdentityProvider, channel routing to call protocol, etc. Each step should have passing tests before proceeding.

## Summary

> To be filled on completion