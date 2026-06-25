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
*(Recommended: one-way door — needs an ADR; user-clarified)*

alknet-ssh as described in the README is the *SSH handler* (server side of the
`alknet/ssh` ALPN). But the reference implementation also ships a substantial
**client** (`crates/alknet-core/src/client/*`: SOCKS5 client, connect logic,
channel manager, ~1900 lines) and a **SOCKS5** implementation
(`src/socks5/*`, ~800 lines) that turns the SSH server into a SOCKS5 *proxy
endpoint* clients can dial. The README lists alknet-ssh's purpose as "SSH
handler (russh), SOCKS5, port forwarding" — so the client/proxy functionality is
intended.

**User clarification (necessary context)**: SOCKS5 and port forwarding in
*both* directions are **core, non-negotiable features** for v1 — they are "the
basic features that made the first version gain interest" (3.5k clones/14 days).
The user runs an actual VPN-like topology (WireGuard + Postgres + Redis today)
over this, and explicitly wants the port-forwarding-in-both-directions
capability to unlock the VPN-like functionality in the new stack. The growing
world-wide trend of banning/blocking "VPNs" (most users use it as a proxy /
location-hiding tool) makes a self-hostable, stream-agnostic SSH-with-forwarding
stack strategically valuable beyond alknet itself.

A concrete downstream consumer that the user wants to *replace* with this stack
is `/workspace/@alkdev/dispatch` — a single-crate axum service that uses
`russh = "0.60"` as an SSH **client** to act as a "reverse git runner" for
Docker containers and remote GPU instances (vast.ai, and eventually runpod /
ubicloud / others). Dispatch's `src/ssh.rs` is a textbook russh client wrapper
(connect + auth + `channel_open_session().exec()` + `disconnect`), and its
`src/handlers.rs::start_forward` does `channel_open_direct_tcpip` local→remote
forwarding (the VPN-like pattern). Dispatch has no SOCKS5 — that's the
alknet-original feature the user wants preserved. Dispatch also factors into a
future "abstract container service" — both it and alknet-ssh share the SSH
client + forwarding primitives, which argues strongly for those primitives living
in alknet-ssh (not duplicated in each consumer).

This reframes the questions:
- Does alknet-ssh own *both* the SSH server (handling `alknet/ssh` connections)
  *and* the SSH *client* (for outbound SSH dialing)? — **Yes** (recommended
  strongly; dispatch and the VPN-like use case both need it, and factoring it
  into alknet-ssh avoids primitive duplication).
- Is the SOCKS5 *server* (what an SSH connection's client dials *through* the
  alknet node) a feature of alknet-ssh, or a separate crate? The SOCKS5 protocol
  itself is transport-independent (it just needs a byte stream), so it *could*
  factor out — but it's tightly coupled to the SSH-forwarding feature and to the
  VPN-like use case. The user explicitly abstracts *some* things out to optional
  crates but stresses that "some is pretty foundational stuff to ssh."

**Recommendation**: alknet-ssh owns **both** the SSH server (`ProtocolHandler`
for `alknet/ssh`) **and** the SSH client (outbound dialing, the primitives
dispatch and the VPN-like topology both consume). Port forwarding in both
directions (`direct-tcpip` local→remote, `forwarded-tcpip`/`tcpip_forward`
remote→local) is **in v1 scope**, not deferred. SOCKS5 is **in v1 scope within
alknet-ssh** (the VPN-like use case needs the node to expose a SOCKS5 *server*
that forwards over the SSH connection); the question of whether the SOCKS5
*protocol codec* factors into a tiny reusable `alknet-socks5` crate (consuming a
byte stream, reusable over other transports) is left as a two-way-door
implementation detail — recommend starting with the codec inside alknet-ssh and
extracting only if a second consumer appears (the "stream-agnostic" philosophy
says this extraction, if done, is cheap). Phase 1 writes an ADR recording this
scope: server + client + bidirectional forwarding + SOCKS5-server-all-in-v1.

### DP-5: Channel-policy surface — which SSH services does alknet-ssh expose?
*(Recommended: one-way door — needs an ADR, at least the default policy;
user-clarified)*

russh's `server::Handler` defaults every channel-request method to reject/no-op
(or, for `auth_publickey_offered`, accept the offer through to signature
verification). alknet-ssh must decide its default channel policy. The user's
clarification sharpens this:

- **session channels**: the dispatch use case uses `channel_open_session().exec()`
  heavily — that's the "reverse git runner" pattern (run a command on the remote
  instance, capture stdout/stderr/exit). For the **server side** of
  `alknet/ssh`, though, the question is whether alknet-ssh *runs a real shell*
  on its own node. Given the VPN-like / forwarding use case is primary and the
  "shell server" use case is secondary, the default should be **exec-only**:
  `shell_request` and `pty_request` default-reject; `exec_request` permitted
  (gated by ACL — see forwarding below). This keeps alknet-ssh a focused
  forwarding/exec appliance rather than a general-purpose interactive login
  server. Interactive shell can be an explicit opt-in later (two-way door).
- **port forwarding in both directions** (`direct-tcpip` in, `tcpip_forward` /
  `forwarded-tcpip` out): **in v1 scope, both directions**, per user
  clarification. The *policy* (which destinations are allowed, whether to
  restrict by ACL/scope) still needs specifying.
- **PTY/X11/agent forwarding**: default-reject for security; explicit opt-in.
  (Consistent with the exec-only session stance.)

**Default-deny baseline**: the user explicitly called out that "the configuration
needs to be such that it's kind of 'default deny', which russh does by default."
russh's `server::Handler` already defaults every channel/auth/forwarding callback
to reject or no-op — so alknet-ssh gets default-deny for free by overriding
only the methods it wants to enable. Phase 1 must record this as the explicit
baseline: every forwarding destination, every exec command, every channel type
must be *explicitly permitted* by config + ACL, never implicitly allowed.

**ACL gating**: forwarding destinations and exec commands are gated by scopes on
the resolved `Identity`. The exact scope vocabulary (e.g., `ssh:forward:*`,
`ssh:forward:127.0.0.1:5432`, `ssh:exec:git-upload-pack`) is a design choice the
Architect makes — likely a small, capability-shaped scope set with wildcards,
consistent with `Identity.scopes` / `Identity.resources` (auth.md). The
"resources" field on `Identity` (populated only by composition per
`CompositionAuthority::as_identity`, ADR-022) is *not* available to
fingerprint/token-resolved external identities, so per-destination ACLs for
inbound SSH must live in `scopes`, not `resources`.

**Recommendation**: Phase 1 writes an ADR defining the v1 channel-policy
surface: exec (gated) + bidirectional port forwarding (gated), with
shell/PTY/X11/agent forwarding default-rejected. Default-deny baseline is
inherited from russh. Forwarding destinations + exec commands gated by ACL
scopes. The exact scope vocabulary is an OQ for Phase 1 (it interacts with how
operators express "allow forwarding to 127.0.0.1:5432" in `DynamicConfig`).

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

### DP-10: Bare-TCP SSH listener — in-v1 for git-over-SSH forward-compat
*(Recommended: one-way door on the *config shape*, two-way door on the *listener
itself* — user-clarified)*

ADR-010 already establishes that bare-TCP SSH is a handler concern, not an
endpoint concern — the SSH handler can listen on a TCP socket independently of
the `alknet/ssh` ALPN path. The user added a forward-looking constraint: **"We
need to be able to have that TCP handler so we can later support git over ssh."**

Standard git-over-SSH (`ssh git@host ...`) runs on TCP port 22, not over QUIC,
not over the `alknet/ssh` ALPN — git clients (`git`, libgit2, `gix`) dial a TCP
socket and expect the SSH-2 protocol directly. To make alknet-ssh a viable
git-over-SSH target, the bare-TCP listener must be a first-class path, not just
a future two-way-door add-on.

The two paths (ALPN/QUIC vs bare-TCP) share the same `russh::server::Config` and
the same `server::Handler` implementation; they differ only in how the duplex
stream is obtained:

- **ALPN path**: `handle()` receives the QUIC `Connection`, calls
  `accept_bi()`, `tokio::io::join`s the halves, hands to `run_stream`.
- **TCP path**: a `tokio::net::TcpListener` accept loop hands each accepted
  `TcpStream` directly to `run_stream` (russh accepts `TcpStream` natively via
  `run_on_socket`, or we use `run_stream` with the raw stream to keep config/
  handler identical across both paths).

**Default-deny baseline (user-stated)**: "the configuration needs to be consider
such that it's kind of 'default deny', which russh does by default." This
applies to *both* paths — the same ACL gating, the same channel policy, the
same default-reject for forwarding destinations. A TCP-listener client gets
*exactly* the same policy treatment as an ALPN client; the only difference is
the transport. The TCP listener is **off by default** (must be explicitly
configured to bind), consistent with the default-deny posture — an operator
who doesn't configure a TCP bind address gets no TCP listener, only the ALPN
path.

**Recommendation**: Phase 1 records the dual-path model in the ssh spec —
ALPN/QUIC primary, bare-TCP as a co-equal first-class path (off by default,
explicit config to enable) — so that the **configuration shape** accommodates
both from v1 even if the TCP listener implementation lands slightly later.
Crucially, the **config schema** should reserve the TCP-listener fields now
(one-way door — adding a config field later is non-breaking but designing the
config *around* only-ALPN-then-retrofitting-TCP is messier than reserving the
shape up front). The listener implementation itself is a two-way door. This
avoids the trap where git-over-SSH becomes a painful retrofit because the
config only modeled the ALPN path.

## Tentative Recommended Approach (Convergence)

Based on the above, the recommended approach to take into Phase 1:

1. **Crate**: `alknet-ssh`, depends on `alknet-core` and `russh = "0.60"`
   (default features, i.e. `aws-lc-rs`). Implements `ProtocolHandler` for
   `b"alknet/ssh"`. **Owns both the SSH server and the SSH client** (the client
   is the shared primitive dispatch and the VPN-like topology both consume).

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

5. **Channel policy — default-deny, exec + bidirectional forwarding in v1**
   (DP-5): v1 supports `exec` (gated) + port forwarding in **both** directions
   (`direct-tcpip` local→remote, `forwarded-tcpip`/`tcpip_forward`
   remote→local, both gated). `shell`/PTY/X11/agent forwarding default-reject
   (opt-in later, two-way door). **Default-deny baseline inherited from
   russh** — every channel type, every forwarding destination, every exec
   command must be explicitly permitted by config + ACL scopes; never
   implicitly allowed. Forwarding destinations + exec commands gated by scopes
   on the resolved `Identity` (the `resources` field is composition-only per
   ADR-022, so inbound-SSH per-destination ACLs live in `scopes`). Needs an ADR
   defining the v1 surface + the scope vocabulary (latter likely stays an OQ).

6. **Client + SOCKS5 — in v1, both in alknet-ssh** (DP-4): alknet-ssh owns the
   SSH *server* (the `ProtocolHandler`) **and** the SSH *client* (outbound
   dialing, the primitives dispatch and the VPN-like topology both consume).
   Port forwarding in both directions is a *client-side* feature too (the
   client opens `direct-tcpip` channels; dispatch does exactly this). SOCKS5
   *server* (what an SSH connection's client dials *through* the alknet node)
   is **in v1 within alknet-ssh** — the VPN-like use case requires it. The
   SOCKS5 protocol codec may or may not factor into a tiny reusable
   `alknet-socks5` crate (consuming a byte stream); recommend starting with the
   codec inside alknet-ssh and extracting only if a second consumer appears
   (two-way door — the stream-agnostic philosophy makes extraction cheap).
   Needs an ADR confirming this scope.

7. **De-risk POC** (DP-9): a Phase 0 POC validating `connect_stream` ↔
   `run_stream` over `tokio::io::duplex()` before Phase 1 finalizes the stream
   wiring spec. Strong empirical evidence from the reference implementation
   suggests it will pass, but the upstream test gap is real.

8. **Bare-TCP SSH listener — first-class path, config shape reserved in v1,
   listener off-by-default** (DP-10): the `alknet/ssh` ALPN/QUIC path is
   primary; a bare-TCP listener is a co-equal first-class path needed for
   future git-over-SSH support. **Reserve the TCP-listener config fields in v1**
   (one-way door on the config schema — retrofitting is messier than reserving
   the shape up front). The listener is **off by default** (explicit config to
   bind), consistent with the default-deny posture. Both paths share the same
   `server::Config` + `Handler` + ACL policy — only the stream source differs.
   The listener implementation itself is a two-way door, but the config shape is
   locked in v1.

## Open Questions to Carry into Phase 1

The following should become OQs in `docs/architecture/open-questions.md`
(numbering will be assigned by the Architect — likely OQ-25 onwards, since
OQ-01–OQ-24 exist):

- **OQ-SSH-01 (host key sourcing)**: vault-derived default + config override —
  resolved by the DP-1 ADR.
- **OQ-SSH-02 (channel policy v1 surface + default-deny scope vocabulary)**:
  the set of allowed channel types / request types is resolved by the DP-5
  ADR; the exact scope vocabulary for forwarding destinations + exec commands
  (e.g., `ssh:forward:127.0.0.1:5432` vs a resources-style shape) stays open —
  it interacts with how operators express allow-lists in `DynamicConfig` and
  with the fact that `Identity.resources` is composition-only (ADR-022).
- **OQ-SSH-03 (client + SOCKS5 scope)**: confirm alknet-ssh owns both server +
  client + SOCKS5-server in v1, and whether the SOCKS5 codec extracts to a
  separate crate now or later — resolved (in favor of in-alknet-ssh-now,
  extract-later) by the DP-4 ADR.
- **OQ-SSH-04 (POC outcome)**: did the `duplex()`-based round-trip POC pass, and
  did it surface any stream-handling constraints (half-open, `poll_shutdown`,
  maximum packet size) that constrain the spec? Resolved by POC Specialist
  results.
- **OQ-SSH-05 (crypto backend)**: confirm `aws-lc-rs` default aligns with the
  rest of the workspace; defer flipping to `ring` unless binary-size pressure
  arises. Two-way door.
- **OQ-SSH-06 (bare-TCP listener enablement timeline)**: the config shape is
  reserved in v1 (DP-10); whether the TCP listener *implementation* lands in v1
  or as a fast-follow is a two-way door. Git-over-SSH is the forcing function —
  decide based on whether v1 needs to be a git-over-SSH target out of the box.

## Next Steps (Phase 0 → Phase 1)

1. **You decide** on the DP-1, DP-4, DP-5, DP-10 recommendations (or amend
   them) — these are the load-bearing architectural choices, and DP-4/DP-5/DP-10
   now reflect your clarifications (SOCKS5 + bidirectional forwarding + TCP
   listener for git-over-SSH are all in-scope; default-deny baseline). DP-2,
   DP-3, DP-6, DP-7, DP-8 are defaults I recommend accepting as-is; DP-9 is a
   POC task.
2. **Optional POC** (DP-9): spawn a POC Specialist to validate
   `connect_stream` ↔ `run_stream` over `tokio::io::duplex()`. Timeboxed; if it
   passes, the stream-wiring spec is straightforward; if it surfaces
   constraints, they fold into the spec.
3. **Phase 1 (Architect)**: produce `docs/architecture/crates/ssh/README.md` +
   component specs (e.g., `ssh-handler.md`, `ssh-stream.md`, `ssh-channels.md`,
   `ssh-auth.md`, `ssh-forwarding.md`, `ssh-socks5.md`, `ssh-client.md`,
   `ssh-tcp-listener.md`), ADRs for the accepted DPs (likely ADR-028 host-key
   sourcing, ADR-029 channel policy + default-deny, ADR-030 ssh server+client+
   socks5+forwarding scope, ADR-031 bare-TCP listener config shape), and the
   OQs above in `open-questions.md`. Update `docs/architecture/README.md` index
   and ADR table.

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
- `/workspace/@alkdev/dispatch/` — concrete downstream consumer the user wants to
  replace with this stack: axum + `russh = "0.60"` SSH **client** for "reverse git
  runner" over Docker/vast.ai. `src/ssh.rs` (russh client wrapper, 143 lines),
  `src/handlers.rs::start_forward` (`channel_open_direct_tcpip` local→remote
  forwarding), `src/sftp.rs` (russh-sftp client). AGENTS.md and
  `docs/architecture.md` describe the architecture. No SOCKS5 — that's the
  alknet-original feature preserved here. Dispatch is a textbook consumer of the
  alknet-ssh **client** + **forwarding** primitives, which is why those live in
  alknet-ssh rather than being duplicated per-consumer.