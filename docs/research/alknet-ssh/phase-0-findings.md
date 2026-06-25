---
status: draft
last_updated: 2026-06-25
---

# alknet-ssh — Phase 0 Research Findings

This document captures Phase 0 (Exploration) findings for the `alknet-ssh`
crate. The objective of Phase 0 per `docs/sdd_process.md` is: *"Capture vision
and guiding principles; research options; validate approaches; converge on a
recommended approach."* It is the input to Phase 1 (Architecture), where the
Architect will produce `docs/architecture/crates/ssh/*.md` specs, ADRs, and open
questions.

## Vision Recap

`alknet-ssh` is the SSH protocol handler for the ALPN-as-service architecture
(ADR-001). It registers the `alknet/ssh` ALPN on the shared `AlknetEndpoint`
and implements the `ProtocolHandler` trait (ADR-002, ADR-007).

The guiding insight, carried over from the reference implementation at
`/workspace/@alkdev/alknet-main/`, is:

> **SSH does not care where its underlying byte stream comes from.**

The reference implementation built on this — it ran the russh SSH-2 state
machine over a `Transport`-produced duplex stream (`AsyncRead + AsyncWrite +
Unpin + Send`) rather than over its own TCP sockets. The greenfield rebuild
keeps the insight and drops the messy transport-abstraction layer that grew
around it: in the new model the `AlknetEndpoint` hands the handler a `Connection`
(quinn/iroh QUIC), and the handler is responsible for opening/accepting the
bidirectional QUIC stream that carries the SSH-2 protocol.

The reference implementation reportedly has 3.5k clones in the past 14 days, so
there is real-world demand for the "SSH-over-arbitrary-stream" capability. The
greenfield rewrite is a total rewrite except most of the vault was initially
copied (also since rewritten).

## Sources Investigated

| Source | Path | Note |
|--------|------|------|
| Existing arch docs (core) | `docs/architecture/crates/core/*` | ProtocolHandler, Connection, BiStream, AuthContext, IdentityProvider, Endpoint |
| Existing ADRs 001–027 | `docs/architecture/decisions/*` | All Accepted; ADR-002/007/010/004/011 most relevant to SSH |
| russh reference deep-dives | `docs/research/references/ssh/russh/01-06` | Already authored; covered overview, keys, protocol, crypto, internals, usage |
| russh source (authoritative) | `/workspace/russh/` | Checked out at `Cargo.toml` version `0.60.2`. The cargo registry cache only contains `russh-0.49.2` — older and NOT the intended version. **Use `/workspace/russh/` as the canonical 0.60.2 reference.** |
| alknet Cargo.lock | `Cargo.lock` | Does **not** yet contain a russh entry — russh is not wired into the workspace dependency graph yet |
| Reference implementation | `/workspace/@alkdev/alknet-main/` | `crates/alknet-core/src/{interface/ssh.rs, server/handler.rs, server/serve.rs, transport/*, client/*}` |

> **Note on the russh clone**: the `/workspace/russh` checkout was inspected and
> its `russh/Cargo.toml` declares `version = "0.60.2"` with `edition = "2024"`
> and MSRV 1.85 — matching the research references. The agent flagged the
> cargo-cache mismatch; verifying against the checkout rather than the cache is
> the safe choice since 0.49.2 → 0.60.2 spans major API changes
> (`server::run_stream` generic signature, `Auth` enum shape, `server::Handler`
> method set all differ). When alknet-ssh's `Cargo.toml` pins `russh = "0.60"`,
> Cargo will fetch the matching 0.60.x into the cache, at which point the cache
> becomes authoritative for *future* investigations.

## Straightforward Parts

These are settled by existing ADRs and the reference implementation; Phase 1
should document them as spec rather than re-litigate them.

### 1. SSH is a `ProtocolHandler` on `alknet/ssh`

Confirmed by overview.md's ALPN Registry and core-types.md. `SshAdapter`
implements `ProtocolHandler::handle(&self, connection: Connection, auth:
&AuthContext) -> Result<(), HandlerError>` with `alpn() = b"alknet/ssh"`. The
handler owns the entire `Connection` lifecycle (ADR-006: one ALPN, one
connection, one handler) and may open/accept multiple QUIC streams because it
multiplexes SSH channels.

### 2. SSH runs over a single QUIC bidirectional stream

The reference implementation's `transport/iroh_transport.rs` proves the
approach: open a QUIC bistream, then **join the two halves into a single duplex
type with `tokio::io::join(recv, send)`** and feed that to russh. This is the
key adapter — it is already a one-liner in tokio:

```rust
// from alknet-main/.../iroh_transport.rs:94
let conn = self.endpoint.connect(self.node_id, ALPN).await?;
let (send, recv) = conn.open_bi().await?;
Ok(io::join(recv, send))   // produces: AsyncRead + AsyncWrite + Unpin + Send
```

The Phase 0 research subagent initially speculated a custom `QuicSshStream`
adapter struct would be needed. Verifying against the reference implementation
revealed that `tokio::io::join` already produces the `AsyncRead + AsyncWrite`
combo russh requires (russh internally re-splits via `tokio::io::split`). **No
custom adapter struct is required** — the `Connection::accept_bi()` /
`open_bi()` pair plus `tokio::io::join` is sufficient. This is a meaningful
simplification over the speculative approach.

### 3. russh accepts a generic stream on both client and server side

Verified from `/workspace/russh/russh/src/`:

- `server::run_stream<H, R>(config: Arc<Config>, stream: R, handler: H)` where
  `R: AsyncRead + AsyncWrite + Unpin + Send + 'static` — `server/mod.rs:997`.
- `client::connect_stream<H, R>(config: Arc<Config>, stream: R, handler: H)`
  with the same bound — `client/mod.rs:982`.

Neither path assumes TCP — TCP-specific code (`set_nodelay`, `TcpListener`) is
confined to `run_on_socket` / `connect` / `run_on_address`. The generic stream
path is clean of TCP assumptions. russh writes its own SSH identification banner
first, then reads the peer's — no caller-side banner pre-work is needed.

### 4. SSH channels multiplex *inside* the QUIC bistream

`ChannelId(u32)` identifies channels; all channel traffic
(`CHANNEL_OPEN`/`DATA`/`EOF`/`CLOSE`/...) is interleaved on the single
underlying SSH transport stream that russh owns. **This is independent of
QUIC's own stream multiplexing** — one QUIC bistream ↔ one SSH connection ↔ many
SSH channels riding inside it. Port forwarding (`direct-tcpip`,
`forwarded-tcpip`) is ordinary channel traffic — each forwarded TCP connection
is a channel, not a separate QUIC stream.

This is the cleanest mapping and the right default: alknet-ssh does not try to
map SSH channels onto QUIC streams (which would require bypassing russh's own
multiplexer). It hands russh one bistream and lets russh multiplex inside it.

### 5. Auth routes through the shared `IdentityProvider`

ADR-004 establishes the hybrid auth model: the endpoint resolves what it can
(TLS client cert → fingerprint), the handler resolves what it must (SSH key
fingerprint). `auth.md` shows the `SshAdapter` pattern exactly — constructor-
inject `Arc<dyn IdentityProvider>`, call `resolve_from_fingerprint()` inside
`handle()` when `auth.identity` is `None`, store the resolved `Identity` on the
`Connection` via `set_identity()` for observability (OQ-11). The
`ConfigIdentityProvider` already resolves SSH key fingerprints against
`DynamicConfig::auth::authorized_keys_fingerprints`. No new auth machinery is
needed for SSH.

### 6. Outbound credentials (if any) come from `Capabilities`

ADR-014 / ADR-022 establish that handlers get outbound credentials through the
registration bundle's `capabilities` field, populated by the assembly layer
from the vault. SSH itself typically needs no outbound credentials (the SSH host
key is a network-identity concern, the SSH *client* key for auth comes from the
peer), but if alknet-ssh ever needs an outbound secret (e.g., to dial an upstream
SOCKS proxy), it comes from `Capabilities`, not from env vars or vault-on-wire.

### 7. TCP SSH is a handler concern, not an endpoint concern

ADR-010 is explicit: "TCP is NOT an endpoint concern... the SSH handler can
listen on a TCP socket independently." This means alknet-ssh may optionally bind
a plain TCP listener (port 22-style) and accept raw SSH connections *outside*
the ALPN endpoint. The `alknet/ssh` ALPN path and the bare-TCP path can coexist;
they share the same `russh::server::Config` and the same `server::Handler`
implementation, differing only in how the stream is obtained. This is a
two-way-door additive capability — the TCP listener can be added later without
touching the ALPN path.

## Less Straightforward Parts (Decision Points)

These are the points where Phase 0 surfaced genuine choices that affect the
architecture. Each is tagged with a recommended door type per ADR-009. The
Architect should turn the *accepted* recommendations into ADRs, and the
*deferred* ones into open questions.

### DP-1: Host key sourcing — vault-derived vs config-loaded vs both
*(Recommended: one-way door — needs an ADR)*

russh's `server::Config.keys: Vec<PrivateKey>` holds the SSH host keys the
server presents during key exchange. The host key is the SSH layer's analogue
of the TLS layer's network identity — it is what the *SSH client* verifies
against `known_hosts`. Three sourcing paths exist:

- **(a) Vault-derived**: derive an Ed25519 key from the alknet-vault seed (HD
  path) and use it as the SSH host key. Aligns with the project's "everything
  keys-from-seed" philosophy (ADR-020, ADR-026) and means the SSH host key is
  deterministic from the mnemonic — a node restored from mnemonic gets the same
  SSH host key fingerprint.
- **(b) Config-loaded**: operator provides SSH host key file path(s) in
  `StaticConfig`/`DynamicConfig`. Matches how OpenSSH works
  (`/etc/ssh/ssh_host_ed25519_key`). Simplest, decoupled from the vault.
- **(c) Both**: vault-derived by default, config override for operators who
  bring their own keys. Mirrors the TLS identity model (ADR-027's
  `TlsIdentity::RawKey` default + `X509`/`Acme` for domain-hosted).

**Recommendation**: **(c) both**, with vault-derived as the default. This
matches the symmetry with `TlsIdentity` in endpoint.md and respects the
"fingerprint-based, keys-from-seed" identity model. The vault is local-only by
construction (ADR-025) and assembly-layer-only access (ADR-019), so the SSH host
key is derived at startup and injected into `SshAdapter::Config` the same way
TLS RawKey identity is. Operators who want stable host keys independent of the
mnemonic can supply a key file. Phase 1 should write an ADR for this (likely
ADR-028) and a corresponding OQ if the exact config-field shape is unresolved.

### DP-2: Per-connection host key selection
*(Recommended: one-way door — needs an ADR, ties to DP-1)*

When supporting multiple host keys (e.g., an Ed25519 default + an RSA key for
legacy clients), russh's `server::Config.keys` is a `Vec` and russh negotiates
which to use based on the client's offered algorithms. The selection is
deterministic per-russh-version but not configurable per-connection. Question:
do we need per-peer host key selection (e.g., present different host keys to
different peer networks)? Almost certainly **no** for v1 — one host key set per
node, advertised uniformly. Phase 1 should record this as the simple model and
leave per-connection selection as a future two-way-door if a use case arises.

### DP-3: Crypto backend — `aws-lc-rs` (default) vs `ring`
*(Recommended: two-way door — decide at implementation time, but pin the choice
in an ADR if it has cross-crate consequences)*

russh 0.60.2 requires exactly one of `aws-lc-rs` (default) or `ring` enabled;
enabling both silently picks `aws-lc-rs`. Both produce AES-GCM / ChaCha20-Poly1305.
Considerations:

- `aws-lc-rs` is the russh default, has broader algorithm coverage, but brings
  NIST build machinery (a heavier build, requires a C compiler + cmake for the
  AWSLC build).
- `ring` is lighter-weight, smaller binary, simpler build.
- **Cross-crate consequence**: alknet-core already depends on `rustls-acme =
    "0.12"` with `features = ["aws-lc-rs"]` (see `crates/alknet-core/Cargo.toml`),
  so `aws-lc-rs` is already in the workspace's build. Choosing `ring` for russh
  while alknet-core uses `aws-lc-rs` would put *both* crypto backends in the
  final binary — wasteful but not incorrect.

**Recommendation**: **default to `aws-lc-rs`** (aligns with the rest of the
workspace and avoids a duplicate crypto backend), but treat the choice as a
two-way door — it can be flipped by changing `default-features = false` on
russh. Phase 1 should note this and *not* spend an ADR on it unless the
duplicate-backend concern turns out to matter for binary size.

### DP-4: Client side — full `russh::client` vs SSH-only-server
*(Recommended: one-way door — needs an ADR)*

alknet-ssh as described in the README is the *SSH handler* (server side of the
`alknet/ssh` ALPN). But the reference implementation also ships a substantial
**client** (`crates/alknet-core/src/client/*`: SOCKS5 client, connect logic,
channel manager, ~1900 lines) and a **SOCKS5** implementation
(`src/socks5/*`, ~800 lines) that turns the SSH server into a SOCKS5 *proxy
endpoint* clients can dial. The README lists alknet-ssh's purpose as "SSH
handler (russh), SOCKS5, port forwarding" — so the client/proxy functionality is
intended.

Questions:
- Does alknet-ssh own *both* the SSH server (handling `alknet/ssh` connections)
  *and* the SSH/SOCKS5 *client* (for the node to dial *out* via SSH to other
  hosts)? Or does the client live elsewhere?
- Is the SOCKS5 server a feature of alknet-ssh, or a separate crate? The SOCKS5
  protocol itself is independent of SSH (it just needs a byte stream), so it
  could be its own reusable crate that alknet-ssh composes with.

**Recommendation**: Phase 1 should clarify scope with an ADR. My tentative
recommendation: alknet-ssh owns the SSH *server* (the `ProtocolHandler`) plus
the SSH *client* (for outbound SSH dialing, needed for port forwarding and
SOCKS-via-SSH). SOCKS5 itself becomes a small, self-contained, reusable crate
(e.g., `alknet-socks5`) that consumes a byte stream — keeping it decoupled from
SSH matches the "stream-agnostic" philosophy and unlocks SOCKS5 reuse over
non-SSH transports. This is a real architectural choice that deserves an ADR
rather than an implicit decision.

### DP-5: Channel-policy surface — which SSH services does alknet-ssh expose?
*(Recommended: one-way door — needs an ADR, at least the default policy)*

russh's `server::Handler` defaults every channel-request method to reject/no-op
(or, for `auth_publickey_offered`, accept the offer through to signature
verification). alknet-ssh must decide its default channel policy:

- **session channels** (`shell`, `exec`, `subsystem`): does alknet-ssh run a
  real shell? A restricted command set? Nothing (exec-only)? This is a major
  behavioral choice. The reference implementation (per overview.md's "what
  stays") had a 974-line `server/handler.rs` and a 555-line
  `server/channel_proxy.rs` — it clearly did substantial channel work
  (proxying channels to upstream connections).
- **port forwarding** (`direct-tcpip` in, `tcpip-forward` / `forwarded-tcpip`
  out): the README explicitly lists "port forwarding" as an alknet-ssh feature,
  so this is in scope. But the *policy* (which destinations are allowed, whether
  to restrict by ACL/scope) needs specifying.
- **PTY/X11/agent forwarding**: almost certainly disabled by default for
  security; explicit opt-in.

**Recommendation**: Phase 1 should write an ADR defining the v1 channel-policy
surface — likely "exec + port-forwarding in scope; shell/PTY/X11/agent
deferred; channel destinations gated by ACL scopes." The exact scope set is a
design choice the Architect makes with the user.

### DP-6: Auth method coverage — publickey-only vs password/kbdint too
*(Recommended: two-way door — start publickey-only, extend later)*

russh supports `none`, `password`, `publickey`, `keyboard-interactive`, and
OpenSSH certificate auth server-side. alknet's identity model (auth.md) is
*fingerprint-based* — SSH key fingerprint → `IdentityProvider` → `Identity`.
This maps naturally onto **publickey** (the fingerprint is the SHA-256 of the
presented public key) and **OpenSSH certificate** auth (cert fingerprint).
Password / keyboard-interactive don't fit the fingerprint model as cleanly
(there's no `resolve_from_password` on `IdentityProvider`).

**Recommendation**: **start publickey-only** (and certificate auth, which is a
superset of publickey from the fingerprint POV). Treat password /
keyboard-interactive as a two-way door — can be added later if a use case
arises, but the natural alknet identity story is key-based. Phase 1 should note
this; likely not a full ADR (it's a default, not a structural decision) but at
least a documented design choice in the ssh spec.

### DP-7: tokio as a hard transitive dependency
*(Recommended: acknowledged constraint, not a decision)*

russh 0.60.2 transitively requires tokio (no "no-tokio" feature; only WASM swaps
the spawner). The server loop uses `tokio::time::sleep` for keepalive/inactivity
timers, so the tokio runtime must have its time driver enabled. **alknet-ssh
must run inside a tokio runtime** — which it will, because alknet-core's endpoint
already runs on tokio (`tokio = { version = "1", features = ["full"] }`). This
is consistent with the rest of the workspace and not a constraint to fight.
Phase 1 should record it as a known constraint; OQ-09 (WASM boundaries) already
documents that the *server-side* dispatch path is a one-way door away from WASM
— alknet-ssh inherits that.

### DP-8: The `ssh-key` crate is forked
*(Recommended: acknowledged constraint — use the russh re-export)*

russh 0.60.2 depends on `internal-russh-forked-ssh-key = "0.6.18"` (a renamed
fork), **not** upstream `ssh-key`. alknet-ssh must not add upstream `ssh-key`
directly — that would put two `ssh-key` versions in the tree and the
`PublicKey`/`PrivateKey` types wouldn't unify. The fork is re-exported through
`russh::keys::ssh_key`, so alknet-ssh should always reach key types via
`russh::keys::*` (or `russh::keys::ssh_key::*`) to stay on the same fork. Phase
1 should note this as an implementation constraint; it's not architecturally
interesting but a real footgun if missed.

### DP-9: End-to-end over a non-TCP stream is untested upstream
*(Recommended: de-risk early with a POC test)*

russh's own test suite (`/workspace/russh/russh/src/tests.rs` and
`client/test.rs`) only exercises the client↔server round trip over real TCP
loopback. There is **no** test connecting `connect_stream` ↔ `run_stream` over
`tokio::io::duplex()` or any other in-memory pipe. The `SshRead::read_ssh_id`
unit tests feed `&[u8]` directly, proving the banner parser works on
non-socket streams — but a full client↔server round trip over a non-TCP stream
is unverified upstream.

The reference implementation uses this path in production (per
`transport/iroh_transport.rs` using `tokio::io::join`), which is strong
empirical evidence it works. But the alknet greenfield rewrite should **close
this gap early** with an integration test using `tokio::io::duplex()` connecting
`connect_stream` ↔ `run_stream` *before* going near real QUIC.

**Recommendation**: per `sdd_process.md` Phase 0, this is a candidate for a POC
Specialist task (`.worktrees/research/ssh-stream-poc/`). Phase 1's
architecture docs should reference the POC's outcome. If the POC surfaces
issues (half-open stream handling, `poll_shutdown` semantics, etc.), they feed
back into the spec as constraints.

## Tentative Recommended Approach (Convergence)

Based on the above, the recommended approach to take into Phase 1:

1. **Crate**: `alknet-ssh`, depends on `alknet-core` and `russh = "0.60"`
   (default features, i.e. `aws-lc-rs`). Implements `ProtocolHandler` for
   `b"alknet/ssh"`.

2. **Stream wiring**: `handle()` accepts the QUIC `Connection`, calls
   `connection.accept_bi()` once to get `(SendStream, RecvStream)`, joins them
   with `tokio::io::join(recv, send)`, and hands the resulting duplex stream to
   `russh::server::run_stream(Arc::clone(&config), stream, handler)`. One QUIC
   bistream ↔ one SSH connection; russh multiplexes SSH channels inside it.

3. **Auth**: constructor-injected `Arc<dyn IdentityProvider>` (per auth.md's
   `SshAdapter` example). Inside `handle()`, if `auth.identity` is `None`,
   russh's `server::Handler::auth_publickey` resolves the offered key's
   fingerprint through the provider; on success, store the resolved `Identity`
   on the `Connection` via `set_identity()` (OQ-11). Start **publickey-only**
   (plus OpenSSH cert, which rides the same fingerprint path).

4. **Host keys** (DP-1): vault-derived Ed25519 by default (derived from the
   seed at startup by the assembly layer and injected into `SshAdapter`'s
   config), with an optional config-supplied key file override. Symmetric with
   `TlsIdentity::RawKey` (ADR-027). Needs an ADR.

5. **Channel policy** (DP-5): v1 supports `exec` + port forwarding
   (`direct-tcpip` / `forwarded-tcpip`); `shell`/PTY/X11/agent forwarding
   deferred (default-reject). Forwarding destinations gated by ACL scopes on the
   resolved `Identity`. Needs an ADR defining the v1 surface.

6. **Client + SOCKS5** (DP-4): alknet-ssh also owns the SSH *client* (outbound
   dialing, needed for forwarding). SOCKS5 protocol factors out into a small
   reusable `alknet-socks5` crate that consumes a byte stream — decoupled from
   SSH, reusable over other transports. Needs an ADR confirming the scope
   split.

7. **De-risk POC** (DP-9): a Phase 0 POC validating `connect_stream` ↔
   `run_stream` over `tokio::io::duplex()` before Phase 1 finalizes the stream
   wiring spec. Strong empirical evidence from the reference implementation
   suggests it will pass, but the upstream test gap is real.

8. **TCP listener** (DP-7/ADR-010): optional, additive, deferred past v1 — the
   `alknet/ssh` ALPN path is the primary surface; a bare-TCP SSH listener can be
   added later sharing the same `server::Config` and `Handler`.

## Open Questions to Carry into Phase 1

The following should become OQs in `docs/architecture/open-questions.md`
(numbering will be assigned by the Architect — likely OQ-25 onwards, since
OQ-01–OQ-24 exist):

- **OQ-SSH-01 (host key sourcing)**: vault-derived default + config override —
  resolved by the DP-1 ADR.
- **OQ-SSH-02 (channel policy v1 surface)**: the exact set of allowed channel
  types / request types — resolved by the DP-5 ADR; some sub-questions (e.g.,
  default forwarding ACL) may stay open.
- **OQ-SSH-03 (client + SOCKS5 split)**: confirm alknet-ssh owns the client and
  `alknet-socks5` is a separate crate — resolved by the DP-4 ADR.
- **OQ-SSH-04 (POC outcome)**: did the `duplex()`-based round-trip POC pass, and
  did it surface any stream-handling constraints (half-open, `poll_shutdown`,
  maximum packet size) that constrain the spec? Resolved by POC Specialist
  results.
- **OQ-SSH-05 (crypto backend)**: confirm `aws-lc-rs` default aligns with the
  rest of the workspace; defer flipping to `ring` unless binary-size pressure
  arises. Two-way door.

## Next Steps (Phase 0 → Phase 1)

1. **You decide** on the DP-1, DP-4, DP-5 recommendations (or amend them) —
   these are the load-bearing architectural choices. DP-3, DP-6, DP-7, DP-8 are
   defaults I recommend accepting as-is; DP-9 is a POC task.
2. **Optional POC** (DP-9): spawn a POC Specialist to validate
   `connect_stream` ↔ `run_stream` over `tokio::io::duplex()`. Timeboxed; if it
   passes, the stream-wiring spec is straightforward; if it surfaces
   constraints, they fold into the spec.
3. **Phase 1 (Architect)**: produce `docs/architecture/crates/ssh/README.md` +
   component specs (e.g., `ssh-handler.md`, `ssh-stream.md`, `ssh-channels.md`,
   `ssh-auth.md`), ADRs for the accepted DPs (likely ADR-028 host-key sourcing,
   ADR-029 channel policy, ADR-030 ssh client + socks5 split), and the OQs above
   in `open-questions.md`. Update `docs/architecture/README.md` index and
   ADR table.

## References

- `docs/sdd_process.md` — Phase 0 process definition
- `docs/architecture/overview.md` — ALPN-as-service, crate graph, ProtocolHandler
- `docs/architecture/crates/core/core-types.md` — ProtocolHandler, Connection, BiStream
- `docs/architecture/crates/core/auth.md` — AuthContext, IdentityProvider, SshAdapter example
- `docs/architecture/decisions/001-alpn-protocol-dispatch.md` — ALPN dispatch
- `docs/architecture/decisions/002-protocol-handler-trait.md` — ProtocolHandler trait
- `docs/architecture/decisions/004-auth-as-shared-core.md` — hybrid auth
- `docs/architecture/decisions/007-bistream-type-definition.md` — BiStream trait
- `docs/architecture/decisions/010-alpn-router-and-endpoint.md` — endpoint, TCP-is-handler-concern
- `docs/architecture/decisions/014-secret-material-flow-and-capability-injection.md` — Capabilities
- `docs/architecture/decisions/022-handler-registration-provenance-and-composition-authority.md` — registration bundle
- `docs/architecture/decisions/025-vault-local-only-dispatch.md` — vault local-only
- `docs/architecture/decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md` — TLS identity model (symmetry reference for DP-1)
- `docs/research/references/ssh/russh/01-06` — existing russh deep-dives
- `/workspace/russh/` — russh 0.60.2 source (authoritative; cargo cache has 0.49.2 only)
- `/workspace/@alkdev/alknet-main/crates/alknet-core/src/` — reference implementation
  (`transport/iroh_transport.rs:94` shows the `tokio::io::join` adapter; `server/`,
  `interface/ssh.rs`, `client/`, `socks5/` for prior art)