# ADR-085: Workspace Scope — Core vs. Consumer Repos

## Status

Accepted

## Context

The original crate decomposition (ADR-003, 2026-06) listed a flat
workspace of ~12 crates: `alknet-core`, `alknet-vault`, `alknet-ssh`,
`alknet-call`, `alknet-agent`, `alknet-git`, `alknet-sftp`, `alknet-msg`,
`alknet-http`, `alknet-dns`, `alknet-napi`, and a CLI binary. This was
written before two things were clear:

1. **Channels** (ADR-071) — a multiplexing substrate that carries
   `alknet/call` on channel 0 and any ALPN on data channels. Channels
   did not exist when ADR-003 was written; it was created to solve a
   pain point that surfaced writing the first consumers. Channels
   changed the shape: a "hub" is a channels hub, a "worker" is a
   channels worker, and most protocol handlers (TTY, tunnels, SFTP,
   SOCKS5, git, SSH) ride inside channels as data-channel ALPNs, not as
   top-level endpoint ALPNs.

2. **The hub/worker topology** (ADR-029, ADR-034, ADR-079) — the
   deployment shape where a central hub accepts worker connections,
   relays channels between legs, and aggregates operations. This is
   the "vpn-like without being a vpn" target. A hub is one who
   initiates an `AlknetEndpoint` serving many transports; a worker
   dials out to a hub. A hub-worker does both.

Because ADR-003's flat list was never revised, the overview's crate
graph has been describing the wrong scope the entire time. It lists
crates that should be consumers in separate repos (`alknet-agent`,
`alknet-docker`) and omits crates that are core to the actual target
(`alknet-tls`, `alknet-channels`, `alknet-hub`, `alknet-worker`,
`alknet-tty`). It also lists crates that were never specced and are not
part of the current scope (`alknet-dns`, `alknet-msg`, `alknet-napi`).

This stale scope is a causal factor in the "assembly layer" hedging
pattern. When the overview implies everything lives in one repo, but
the actual architecture requires a hub/worker composition layer that
isn't in the graph, agents fill the gap with "assembly layer" as an
escape hatch — a path-of-least-resistance solution to an impossible
bind. The fix is not to suppress the hedging; it's to make the scope
clear so the bind doesn't arise.

### What "core" means now

The alknet mono-repo is the **core networking toolkit** — the crates
that a hub, a worker, or a hub-worker are built from, plus the protocol
handlers that are foundational to the "p2p-capable vpn-like without
being a vpn" target. Crates that build *on top of* a hub or worker
(docker operations, agent, future applications) are consumers in
their own repos — they depend on `alknet-call` / `alknet-channels` /
`alknet-hub` / `alknet-worker`, not on `alknet-core` directly.

The distinction is:
- **Core mono-repo**: the substrate (core, tls, call, channels), the
  deployment shapes (hub, worker), and the foundational protocol
  handlers that every hub/worker needs (tty, tunnels, fs, sftp, ssh,
  http). Vault is core because it's foundational to ACL (key derivation,
  identity).
- **Consumer repos**: crates that build on top of a hub or worker
  (docker operations, agent, future applications). These are
  independent repos that depend on the published core crates.

### The foundational handlers

The protocol handlers that are foundational to the vpn-like target
ride inside channels as data-channel ALPNs:

| ALPN (inside channels) | Crate | Status |
|------------------------|-------|--------|
| `alknet/tty` | `alknet-tty` | specced (ADR-052–057), implemented |
| `alknet/tunnel` | (in `alknet-channels` or a sibling) | POC-validated, not yet specced — minimal (the channels POC covers this use case) |
| `alknet/socks5` | (TBD) | not yet discussed — SOCKS5 proxy over channels |
| `alknet/fs` | (TBD) | not yet specced — filesystem access over channels |
| `alknet/sftp` | (TBD) | not yet specced — SFTP protocol core over channels |

Additionally, `alknet-http` (the HTTP edge case — registration endpoint,
browser access, MCP/OpenAPI adapters) and a future `alknet-ssh` (russh
server channels wrapper as an option for hubs, for git/sftp
compatibility) are core mono-repo concerns because they are part of the
hub's inbound surface.

### What leaves the mono-repo

| Crate | Destination | Why it's a consumer, not core |
|-------|-------------|-------------------------------|
| `alknet-docker` | own repo | Docker operations build on top of a hub/worker — a docker host is a worker, not a substrate concern. Depends on `alknet-call` + `alknet-tty`, not on core transport. |
| `alknet-agent` | own repo | The agent builds on `alknet-call` for tool dispatch — it's an application, not networking substrate. |

These specs are kept in `docs/architecture/crates/docker/` for
reference (the work is not lost), but the crates move to their own repos
as consumers. The overview's crate graph no longer lists them as
mono-repo members.

### What was never in scope (and is removed from the graph)

`alknet-dns`, `alknet-msg`, `alknet-napi` were listed in ADR-003's flat
decomposition but were never specced, never implemented, and are not
part of the current target. They are removed from the overview's crate
graph. If a DNS or messaging handler becomes needed, it will be a
consumer repo (a handler that rides inside channels or registers on the
endpoint), not a mono-repo member. NAPI projection, if needed, lives
with whatever consumer needs the Node.js bridge.

## Decision

### The alknet mono-repo scope is the core networking toolkit

The mono-repo contains the substrate, the deployment shapes, and the
foundational protocol handlers. Everything else is a consumer in its
own repo.

```
alknet mono-repo (the core networking toolkit)
│
├── Substrate
│   ├── alknet-core      (ProtocolHandler, endpoint, auth, config, Connection)
│   ├── alknet-tls       (shared TLS config — ADR-082)
│   ├── alknet-call      (call protocol on alknet/call)
│   └── alknet-channels  (multiplexing substrate on alknet/channels — ADR-071)
│       ├── alknet-channels-core  (pure multiplexer — ADR-081)
│       └── alknet-channels-call   (channel 0 pre-negotiation — ADR-081)
│
├── Deployment shapes
│   ├── alknet-hub       (channels hub — accepts workers, relays, aggregates)
│   └── alknet-worker    (channels worker — dials out to a hub)
│
├── Foundational handlers (inside channels as data-channel ALPNs, or on the endpoint)
│   ├── alknet-tty       (alknet/tty — specced, implemented)
│   ├── alknet-http      (h2/http1.1 + WebSocket — the HTTP edge case)
│   ├── alknet-tty-local (PTY/pipe backend — sibling crate)
│   ├── alknet-ssh       (russh server channels wrapper — for git/sftp compat) [not yet specced]
│   ├── alknet-tunnel    (alknet/tunnel — POC-validated, minimal spec needed) [not yet specced]
│   ├── alknet-socks5    (SOCKS5 proxy over channels) [not yet specced]
│   ├── alknet-fs        (filesystem access over channels) [not yet specced]
│   └── alknet-sftp      (SFTP over channels) [not yet specced]
│
└── alknet-vault         (standalone — foundational to ACL: key derivation, identity)
```

### Dependency rules

- The substrate crates (`core`, `tls`, `call`, `channels`) depend on
  each other in a clean DAG: `channels` → `call` → `core`; `tls` →
  `core`. No cycles.
- `alknet-hub` and `alknet-worker` depend on the substrate (channels,
  call, core) and on the handlers they wire. They are consumers of the
  substrate, not part of it.
- Foundational handlers depend on `alknet-core` (for
  `ProtocolHandler`, `Connection`) and/or `alknet-channels` (for
  `ChannelBidiStreamSource`, `into_sub_streams`). No handler depends
  on another handler — cross-handler communication goes through
  `alknet/call` on channel 0.
- `alknet-vault` is standalone (zero alknet crate dependencies — ADR-018).
  It is foundational to ACL: the hub/worker identity model
  (`IdentityProvider`, `PeerEntry`, fingerprint resolution) derives
  from vault-managed keys. Vault is accessed only at the assembly layer
  (ADR-019); handlers receive derived credentials via capabilities
  (ADR-014), never a vault reference.
- Consumer repos (docker, agent, future applications) depend on the
  published core crates (`alknet-call`, `alknet-channels`,
  `alknet-hub`, `alknet-worker`), not on `alknet-core` directly.

### The hub/worker model

A **hub** is a channels hub — it accepts inbound connections (over
quinn, iroh, TCP+TLS — ADR-083), runs `ChannelsAdapter` on
`alknet/channels`, relays data channels between legs (ADR-079),
aggregates workers' operations into a shared env, and serves the
discovery API. A hub may also serve HTTP (`h2`/`http/1.1` for
registration and browser access) and, optionally, an SSH server (russh
channels wrapper for git/sftp compatibility).

A **worker** is a channels worker — it dials out to a hub via
`ChannelClient`, runs `from_call` to discover the hub's (and other
workers') operations, and exposes its own operations on channel 0. A
worker has no inbound endpoints unless it is also a hub (hub-worker).

A **hub-worker** does both — accepts inbound and dials out. This is a
valid deployment shape; the topology is not strictly hierarchical.

The bidirectionality of call and channels means both sides can be both
hub and worker within a connection. A hub (A) that uses a client to
connect to another hub (B) is, from B's perspective, a worker. This
does not require a separate "hub-as-client" abstraction — the
`ChannelClient` / `CallClient` take-over APIs (`from_connection`,
`spawn_dispatch`) are transport-agnostic and work regardless of
whether the dialer is a hub, a worker, or a hub-worker.

### ADR-003 is amended

ADR-003's flat crate table is superseded for scoping purposes. The
"one crate per protocol handler, core provides shared infra" principle
survives; the specific crate list does not. The crate list is now this
ADR's scope table. ADR-003's amendments (the `alknet-call` as
protocol-foundation clarification, the `alknet-tty` no-`alknet-call`
clarification, the irpc removal) survive — they are about dependency
edges, not about which crates are in the mono-repo.

## Consequences

**Positive:**
- The overview's crate graph will match reality for the first time.
  Future sessions start with an accurate scope, not a stale flat list
  that implies the wrong boundary.
- The "assembly layer" escape hatch has a bounded home: the hub and
  worker crates. "Assembly layer" = the deployment binary (hub, worker,
  or hub-worker), not a dump for unknowns. This is the fix for the
  hedging pattern's root cause.
- Consumer repos (docker, agent) are unblocked — they can be developed
  independently against the published core crates, without waiting for
  the mono-repo to accommodate their concerns.
- The scope is narrow enough to finish. Six substrate + deployment
  crates (core, tls, call, channels, hub, worker) plus the foundational
  handlers (tty, http, ssh, tunnel, socks5, fs, sftp) plus vault is a
  bounded surface. The previous scope (~12 flat crates including DNS,
  messaging, NAPI) was never the target.
- The foundational handlers that ride inside channels (tunnel, socks5,
  fs, sftp) are correctly scoped as channels data-channel ALPNs, not
  top-level endpoint handlers. This is the "p2p-capable vpn-like"
  shape — these services are available over channels, with the ACL and
  bidirectionality that channels + call provide.

**Negative:**
- `alknet-docker` and `alknet-agent` leave the mono-repo. Their specs
  stay in `docs/architecture/crates/docker/` (and a future
  `crates/agent/` if specced) for reference, but the crate code moves to
  consumer repos. This is a repository boundary change, not a loss of
  work — the specs and ADRs (058–063 for docker) remain valid as the
  consumer's architecture.
- Several foundational handlers (ssh, tunnel, socks5, fs, sftp) are
  named in the scope but not yet specced. The scope table makes this
  visible — it is a backlog, not a hidden gap.
- `alknet-worker` has no spec yet. The worker pattern is described
  inside the hub README (as the inverse of hub), but a dedicated
  `crates/worker/README.md` or a combined hub/worker doc is needed.

## Door type

**One-way.** The repo boundary (core mono-repo vs. consumer repos) is
structural — once docker and agent are in their own repos with their
own release cycles, reversing means merging them back and breaking
downstream consumers that depend on the published crates. The scope
table (which crates are core) is one-way for the same reason: the
dep graph and the overview orient around it.

## References

- ADR-003: Crate Decomposition (amended — the flat crate list is
  superseded; the decomposition principle survives)
- ADR-029: Peer-Graph Routing Model (the hub/worker topology)
- ADR-034: Three Peer Roles (hub = role-3, worker = role-1/2)
- ADR-071: alknet-channels Wire Format (the multiplexing substrate)
- ADR-079: Hub Relay (translate, not transparently forward)
- ADR-080: ChannelClient (the worker's dial path)
- ADR-081: channels sub-crate decomposition (channels-core / channels-call)
- ADR-082: alknet-tls extraction
- ADR-083: Endpoint as multi-transport accept-loop runner
- `docs/architecture/crates/hub/README.md` (the hub pattern — the
  deployment shape this scope is built around)