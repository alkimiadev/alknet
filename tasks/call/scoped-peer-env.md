---
id: call/scoped-peer-env
name: Add ScopedPeerEnv peer-pinned reachability (ADR-029 §4)
status: pending
depends_on: [call/operation-env-invoke-peer]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Extend the per-handler reachability set with peer-pinned entries, so a handler
can declare that an op is reachable *only* when routed to a specific peer
(`PeerRef::Specific`). This is the structural disambiguation mechanism that
replaces `FromCallConfig::namespace_prefix` as a reachability concern (the
prefix stays as optional local-naming sugar, ADR-029 §5). Per ADR-029 §4.

Today the reachability set is `ScopedOperationEnv { allowed: HashSet<String> }`
(`registry/context.rs:78`). It is peer-agnostic — a name in `allowed` is
reachable via any routing path (`PeerRef::Any` or `PeerRef::Specific(any)`).
The head→N-workers disambiguation case (head imports `/container/exec` from
worker A and worker B; a handler wants to *pin* its `container/exec` call to
worker A specifically) is not expressible in the reachability set. It can only
be expressed at the call site by passing `PeerRef::Specific("worker-a")` to
`invoke_peer`, which is a routing decision, not a declaration — the handler's
scoped env permits `container/exec` regardless of which peer serves it.

ADR-029 §4 specifies the rename-and-extend:

```rust
pub struct ScopedPeerEnv {
    pub allowed_ops: HashSet<String>,    // peer-agnostic — reachable via PeerRef::Any
    pub peer_pinned: HashSet<String>,     // "peer-id/op-name" — reachable only via PeerRef::Specific(that peer)
}
```

> The existing `ScopedOperationEnv.allowed` becomes the `allowed_ops` field;
> peer-pinning is additive. Unqualified reachability (peer-agnostic
> composition) stays the common case; peer-pinning is opt-in for the
> disambiguation case. — ADR-029 §4

### Rename + extend

`ScopedOperationEnv` is renamed to `ScopedPeerEnv` and gains the `peer_pinned`
field. All call sites that construct `ScopedOperationEnv::new(["op"])` /
`ScopedOperationEnv::empty()` migrate to `ScopedPeerEnv::new(["op"])` /
`ScopedPeerEnv::empty()` (the `new`/`empty` constructors populate
`allowed_ops` and leave `peer_pinned` empty — the common case stays a
one-liner). A new constructor adds peer-pinned entries:

```rust
impl ScopedPeerEnv {
    pub fn empty() -> Self { Self { allowed_ops: HashSet::new(), peer_pinned: HashSet::new() } }

    pub fn new(ops: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self { allowed_ops: ops.into_iter().map(|s| s.into()).collect(), peer_pinned: HashSet::new() }
    }

    /// Peer-pinned reachability: "peer-id/op-name". Reachable only via
    /// PeerRef::Specific(that peer). Additive to `new` — call `new` for the
    /// peer-agnostic set, then `with_pinned` for the pinned set.
    pub fn with_pinned(mut self, pinned: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.peer_pinned = pinned.into_iter().map(|s| s.into()).collect();
        self
    }

    /// Peer-agnostic reachability — unchanged from ScopedOperationEnv::allows.
    /// A name here is reachable via any routing path (PeerRef::Any or Specific).
    pub fn allows(&self, name: &str) -> bool { self.allowed_ops.contains(name) }

    /// Peer-pinned reachability — reachable only via PeerRef::Specific(peer).
    /// The entry shape is "peer-id/op-name" (ADR-029 §4, OQ-33).
    pub fn allows_pinned(&self, peer: &PeerId, name: &str) -> bool {
        self.peer_pinned.contains(&format!("{peer}/{name}"))
    }

    /// Does this scoped env permit `name` via `peer`? Used by the reachability
    /// gate in invoke_peer / invoke_with_policy.
    /// - PeerRef::Any  → allows(name)
    /// - PeerRef::Specific(peer) → allows(name) || allows_pinned(peer, name)
    pub fn allows_via(&self, peer: &PeerRef, name: &str) -> bool {
        match peer {
            PeerRef::Any => self.allows(name),
            PeerRef::Specific(p) => self.allows(name) || self.allows_pinned(p, name),
        }
    }
}
```

A name in `peer_pinned` but not in `allowed_ops` is reachable *only* via
`PeerRef::Specific(that peer)` — `PeerRef::Any` does NOT pick it up (the
pin is a restriction of the routing paths that satisfy reachability, not a
permission for any-path routing). This is the disambiguation guarantee: the
handler declared "this op is reachable only on worker A," so `Any` must not
silently route it to worker B.

### Reachability gate

The existing reachability gate (`scoped_env.allows(&name)`) at
`registry/env.rs:114` (`invoke_with_policy`) and `registry/env.rs:225,266`
(`invoke_peer` for `Specific` and `Any`) is extended to consult
`peer_pinned`:

- `invoke_with_policy` (the `PeerRef::Any` path, `env.rs:114`): unchanged —
  `scoped_env.allows(&name)`. Peer-pinned-only ops are NOT reachable via this
  path. This preserves the existing peer-agnostic behavior: a handler that
  pins `container/exec` to worker A in its scoped env cannot reach it via
  `invoke_with_policy` (the `Any` fan-out); it must use `invoke_peer` with
  `PeerRef::Specific("worker-a")`.
- `invoke_peer` (`env.rs:225` for `Specific`, `env.rs:266` for `Any`):
  - `PeerRef::Specific(peer)`: `scoped_env.allows_via(&PeerRef::Specific(peer), &name)`
    — true if the name is in `allowed_ops` (peer-agnostic) OR in `peer_pinned`
    for this specific peer. False otherwise → NOT_FOUND.
  - `PeerRef::Any`: `scoped_env.allows(&name)` — peer-pinned-only ops are NOT
    reachable via `Any` (the pin restricts to `Specific`). Unchanged from
    today for the peer-agnostic set.

`connection.rs:266` (`CallConnection::invoke` — the client-side
`PeerRef::Any`-equivalent path) uses `scoped_env.allows(&name)` — unchanged
(it's the `Any` path; pinned-only ops aren't reachable there).

### HandlerRegistration field type

`HandlerRegistration.scoped_env: Option<ScopedOperationEnv>`
(`registry/registration.rs:34,44,149,235`) becomes
`Option<ScopedPeerEnv>`. The `FromCallConfig` path
(`client/from_call.rs`) populates `allowed_ops` with the imported op names
(unchanged from today) and leaves `peer_pinned` empty by default; a
`with_pinned` call is the opt-in for pinning an imported op to a specific peer.

### What this task does NOT do

- Does NOT change `from_call` collision behavior — that's already done
  (`call/from-call-forwarded-for`: cross-peer collision dissolves,
  same-peer collision is `SamePeerCollision`). `namespace_prefix` stays as
  local-naming sugar (ADR-029 §5); it is not removed.
- Does NOT change `services/list` filtering — that's `AccessControl`-based
  (`call/services-list-accesscontrol-filtered`). The scoped env is the
  *composition reachability* set (which ops this handler may call), not the
  *listing* set (which ops a peer sees).
- Does NOT introduce a `RoutingPolicy` (round-robin / least-loaded) — that's
  OQ-30, out of scope. `PeerRef::Any` stays insertion-order first-match.

## Acceptance Criteria

- [ ] `ScopedPeerEnv` struct with `allowed_ops: HashSet<String>` and `peer_pinned: HashSet<String>`
- [ ] `ScopedOperationEnv` removed (renamed; all call sites migrated — `context.rs`, `registration.rs`, `adapter.rs`, `connection.rs`, `dispatch.rs`, `env.rs`, `from_call.rs`, `from_jsonschema.rs`, and all tests)
- [ ] `ScopedPeerEnv::empty()`, `new(ops)`, `with_pinned(pinned)` constructors
- [ ] `allows(name)` — peer-agnostic reachability (unchanged behavior)
- [ ] `allows_pinned(peer, name)` — peer-pinned reachability ("peer-id/op-name")
- [ ] `allows_via(peer, name)` — combined reachability check for the gate
- [ ] `invoke_with_policy` (`env.rs:114`) gate unchanged: `scoped_env.allows(&name)` (pinned-only ops NOT reachable via `Any`)
- [ ] `invoke_peer` `PeerRef::Specific` (`env.rs:225`) gate: `scoped_env.allows_via(&PeerRef::Specific(peer), &name)`
- [ ] `invoke_peer` `PeerRef::Any` (`env.rs:266`) gate: `scoped_env.allows(&name)` (pinned-only NOT reachable via `Any`)
- [ ] `CallConnection::invoke` (`connection.rs:266`) gate unchanged (`Any`-equivalent path)
- [ ] `HandlerRegistration.scoped_env: Option<ScopedPeerEnv>` (field type migrated)
- [ ] Name in `peer_pinned` but not `allowed_ops` → reachable ONLY via `PeerRef::Specific(that peer)`; `Any` returns NOT_FOUND
- [ ] Name in both `allowed_ops` and `peer_pinned` for a peer → reachable via both `Any` and `Specific(peer)` (allowed_ops is the permissive set; pin is additive restriction for the Specific case, not a narrowing of Any)
- [ ] `FromCallConfig` populates `allowed_ops` (default), `peer_pinned` empty by default
- [ ] Unit test: `ScopedPeerEnv::new` + `with_pinned` populates both fields
- [ ] Unit test: `allows` checks `allowed_ops` only (peer-agnostic, unchanged)
- [ ] Unit test: `allows_pinned` checks `peer_pinned` ("peer-id/op-name" shape)
- [ ] Unit test: `allows_via` — `Any` uses `allowed_ops`; `Specific(peer)` uses `allowed_ops || peer_pinned`
- [ ] Unit test: pinned-only op reachable via `PeerRef::Specific(peer)`, NOT reachable via `PeerRef::Any` (NOT_FOUND)
- [ ] Unit test: op in both `allowed_ops` and `peer_pinned` reachable via both `Any` and `Specific`
- [ ] Unit test: `invoke_peer` `Specific` with wrong peer for a pinned-only op → NOT_FOUND (pinned to peer A, routed to peer B)
- [ ] Unit test: `invoke_with_policy` (`Any` path) does not pick up pinned-only ops
- [ ] `cargo test --workspace` succeeds
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` succeeds
- [ ] `cargo fmt --all -- --check` succeeds

## References

- docs/architecture/decisions/029-peer-graph-routing-model.md — ADR-029 §4 (ScopedPeerEnv), §5 (namespace_prefix becomes local-naming sugar)
- docs/architecture/decisions/030-peerentry-and-identity-id-decoupling.md — ADR-030 (PeerId source — peer_pinned entry shape uses PeerId = Identity.id)
- docs/architecture/crates/call/operation-registry.md — HandlerRegistration.scoped_env (field type migrated)
- docs/architecture/crates/call/client-and-adapters.md — invoke_peer signature and ScopedPeerEnv peer-qualified reachability (line 183)
- docs/architecture/open-questions.md — OQ-33 (PeerId logical id — the peer_pinned entry "peer-id/op-name" shape)

## Notes

> The §4 gap surfaced during a verification pass (not the original
> review-sync): ADR-029 §1-3, §5-6 were decomposed into tasks and completed,
> but §4 was never taskified — the specs reference `ScopedPeerEnv` (e.g.
> `client-and-adapters.md:183`) while the code still uses `ScopedOperationEnv`.
> This task closes that drift. The change is a rename + extend (not a redesign):
> `allowed_ops` is the existing `allowed` set, `peer_pinned` is the additive
> new field, and the reachability gate gains a `PeerRef`-aware check. The
> `PeerRef::Any` path is unchanged (pinned-only ops are NOT reachable via
> `Any` — the pin is a restriction, not a permission). The prefix-as-sugar
> remainder (ADR-029 §5) is untouched. Risk is medium, not high: the rename
> is mechanical (compiler-checked), the new gate logic is small, and the
> behavior of the common case (no `peer_pinned` entries) is identical to
> today.