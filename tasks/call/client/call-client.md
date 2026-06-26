---
id: call/client/call-client
name: Implement CallClient (outbound connection opener) with peer-scoped default-deny dispatch (ADR-017, ADR-028)
status: completed
depends_on: [call/protocol/call-connection, call/registry/remote-safe-marking]
scope: moderate
risk: high
impact: phase
level: implementation
---

## Description

Implement `CallClient` in `src/client/mod.rs` (new `client` module). This is
the #1 gap in alknet-call — the outbound connection opener. Every downstream
consumer (runner pattern, container service, bilateral exchange, NAPI
projection, agent cross-node tool dispatch) is blocked on it. It opens a QUIC
connection to a remote node on ALPN `alknet/call`, performs credential setup,
and produces a `CallConnection` running the **shared** dispatch loop (ADR-017
§1). `CallClient` is the connection-establishment half; `CallAdapter`'s
accept path is the inbound half. Both produce the same `CallConnection`.

### CallClient struct

```rust
pub struct CallClient {
    /// The operation registry. The peer-scoped view is a dispatch-time read
    /// over this registry, not a copy (ADR-028 §5).
    registry: Arc<OperationRegistry>,
    identity_provider: Arc<dyn IdentityProvider>,
    /// Trusted-peer mode (ADR-028 §3): when true, the dispatch path exposes
    /// all External ops to the remote peer and services/list lists all
    /// External ops, ignoring the remote_safe marking. When false (default),
    /// only registrations with remote_safe: true dispatch, and services/list
    /// hides non-remote-safe ops (ADR-028 Assumption 2).
    trusted_peer: bool,
}

impl CallClient {
    /// Default: peer-scoped (default-deny). Filters dispatch + services/list
    /// by remote_safe == true.
    pub fn new(registry: Arc<OperationRegistry>, idp: Arc<dyn IdentityProvider>) -> Self;

    /// Trusted-peer mode: expose all External ops, ignore remote_safe.
    /// Explicit opt-in per ADR-028 §3.
    pub fn trusted_peer(registry: Arc<OperationRegistry>, idp: Arc<dyn IdentityProvider>) -> Self;

    /// Open a QUIC connection to `addr` on ALPN `alknet/call`, perform
    /// credential handshake, and return a CallConnection running the shared
    /// dispatch loop. Credentials come from capabilities (ADR-014), not env
    /// vars — see client-and-adapters.md "No-Env-Vars Invariant".
    pub async fn connect(
        &self,
        addr: SocketAddr,
        credentials: CallCredentials,
    ) -> Result<CallConnection, ClientError>;
}
```

### Shared dispatch loop

The dispatch loop is **shared** with `CallAdapter`. Once a connection is
established (whether accepted or opened), the same logic applies: read
`EventEnvelope` frames, dispatch to the operation registry, write responses,
send outgoing `call.requested` for calls initiated on this side. Refactor the
existing accept-path dispatch out of `CallAdapter` into a shared function
(likely in `src/protocol/connection.rs` or a new `src/protocol/dispatch.rs`)
that both `CallAdapter::handle` and `CallClient::connect` call. Do not
duplicate the dispatch loop — ADR-017 §1 is explicit that the client is the
connection-establishment half, not a parallel protocol implementation.

The `CallConnection` type already exists (`protocol/connection.rs`) and
holds the Layer 2 overlay + call/subscribe/abort API. `CallClient::connect`
constructs it from the opened connection (vs `CallAdapter` constructing it
from the accepted connection).

### Peer-scoped dispatch (ADR-028 — default-deny)

The incoming-call dispatch path in the `CallClient` must filter by
`remote_safe`:

- **Default mode** (`trusted_peer: false`): an incoming `call.requested` for
  an op name resolves to the registration; if `registration.remote_safe ==
  false`, return `NOT_FOUND` (not `FORBIDDEN` — same posture as
  `Visibility::Internal` per ADR-015). If `true`, dispatch normally.
  `OperationContext.capabilities` is populated from the registration bundle
  only for remote-safe ops — this is the security argument for default-deny
  (ADR-028 Context): a remote peer's call must not trigger dispatch that
  populates capabilities from the local node's registration bundle unless the
  op is explicitly exposed.
- **Trusted-peer mode** (`trusted_peer: true`): bypass the `remote_safe`
  filter; expose all `External` ops. The operator has made the trust decision
  explicitly.

This is a dispatch-time read over the single Layer-0 registry (ADR-028 §5) —
not a copied subset, not a third registry instance. The `OperationRegistry`
from `remote-safe-marking` is the single source.

### services/list hide behavior (ADR-028 Assumption 2)

When the `CallClient` serves `services/list` to the remote peer:

- **Default mode**: hide ops where `remote_safe == false` (in addition to
  the existing `Visibility::External` filter). A peer should not see ops it
  cannot call.
- **Trusted-peer mode**: list all `External` ops regardless of
  `remote_safe`.

The existing `services_list_handler` in `registry/discovery.rs` filters by
`Visibility::External` only. Wire the additional `remote_safe` filter for the
`CallClient`'s serving path. (The `CallAdapter`'s serving path — local
accept — is unchanged; it continues to list all `External` ops, since a
direct QUIC client is not a `CallClient` peer in the filtered sense. Clarify
this split in code comments and a test.)

### Credentials

`connect()` takes a `CallCredentials` bundle. Credentials come from
`Capabilities` (ADR-014), never env vars. The three dimensions (ADR-017 §7):
TLS identity (RFC 7250 raw key or X.509, ADR-027), auth token (opaque,
vault-decrypted), remote identity verification (expected fingerprint/cert).
Populated by the assembly layer at `CallClient` construction time from
vault-derived `Capabilities`. The concrete `TlsIdentity` / `AuthToken` /
`RemoteIdentity` shapes are implementation-detail two-way doors (recorded in
client-and-adapters.md); the one-way constraint is they come from
capabilities, not env vars.

### Connection symmetry

After establishment, the connection is symmetric (ADR-017 §2): both sides
can send and receive `call.requested`. Connection direction is independent of
call direction. The `CallClient` is both a caller (initiates outgoing calls
via `CallConnection::call()`/`subscribe()`/`abort()`) and a callee
(dispatches incoming calls against its peer-scoped view).

## Acceptance Criteria

- [ ] `src/client/mod.rs` exists with `CallClient` struct (registry, idp, trusted_peer)
- [ ] `CallClient::new` constructs default-deny (trusted_peer: false)
- [ ] `CallClient::trusted_peer` constructs trusted-peer mode
- [ ] `connect()` opens a QUIC connection on ALPN `alknet/call`
- [ ] `connect()` returns a `CallConnection` running the shared dispatch loop
- [ ] Dispatch loop is shared with `CallAdapter` (refactored, not duplicated)
- [ ] Default mode: incoming call to op with `remote_safe == false` returns NOT_FOUND
- [ ] Default mode: incoming call to op with `remote_safe == true` dispatches
- [ ] Default mode: capabilities populated only for remote-safe dispatched ops
- [ ] Trusted-peer mode: all External ops dispatch regardless of remote_safe
- [ ] Default mode: services/list hides non-remote-safe ops from the peer
- [ ] Trusted-peer mode: services/list lists all External ops
- [ ] Outgoing call()/subscribe()/abort() work through the returned CallConnection
- [ ] Connection symmetry: remote peer can call back into the CallClient
- [ ] `CallCredentials` carries TLS identity / auth token / remote identity (from capabilities)
- [ ] No env-var reads in the credential path (no-env-vars invariant, ADR-014)
- [ ] Integration test: two-node call (CallClient connects to CallAdapter, both call each other)
- [ ] Integration test: default-deny op returns NOT_FOUND to remote peer
- [ ] Integration test: remote_safe op dispatches to remote peer
- [ ] Integration test: trusted-peer mode exposes all External ops
- [ ] Integration test: services/list hides non-remote-safe in default mode
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call --all-targets` succeeds with no warnings

## References

- docs/architecture/crates/call/client-and-adapters.md — CallClient §, credential sources §
- docs/architecture/decisions/017-call-protocol-client-and-adapter-contract.md — ADR-017 §1 (shared loop), §2 (symmetry), §7 (credentials)
- docs/architecture/decisions/028-callclient-peer-scoped-registry-filtering.md — ADR-028 (default-deny, trusted-peer, Assumption 2 services/list hide)
- docs/architecture/crates/call/call-protocol.md — CallConnection (the type connect() produces)
- tasks/call/protocol/call-connection.md — completed CallConnection task
- tasks/call/registry/remote-safe-marking.md — prerequisite (adds remote_safe field)
- docs/research/alknet-call-completion/gap-analysis.md — DC-1, implementation priority #1

## Notes

> This is the single highest-value piece of work in the alknet-call
> completion — every downstream consumer is blocked on it. The dispatch loop
> is shared with CallAdapter (refactor, don't duplicate — ADR-017 §1 is
> explicit). The peer-scoped default-deny (ADR-028) is the one-way-door
> security dimension: a remote peer's call must not populate
> OperationContext.capabilities from the local bundle unless the op is
> explicitly remote-safe. The v1 shape is `trusted_peer: bool` + the
> `remote_safe: bool` field from `remote-safe-marking`; per-peer allowlists
> are OQ-25 and explicitly out of scope. Credentials come from capabilities,
> never env vars (no-env-vars invariant).