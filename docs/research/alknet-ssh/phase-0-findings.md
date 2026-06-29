---
status: draft
last_updated: 2026-06-29
---

# alknet-ssh — Phase 0 Research Findings

This document captures Phase 0 (Exploration) findings for the `alknet-ssh`
crate. The objective of Phase 0 per `docs/sdd_process.md` is: *"Capture vision
and guiding principles; research options; validate approaches; converge on a
recommended approach."* It is the input to Phase 1 (Architecture), where the
Architect will produce `docs/architecture/crates/ssh/*.md` specs, ADRs, and
open questions.

This document was initially drafted 2026-06-25 and **revised 2026-06-29** to
reflect two developments that changed the framing: (1) the WebTransport
architecture landed as ADRs 038/040/043, grounding the SSH-over-WebTransport
path that was previously speculative; (2) the recognition that SSH's channel
multiplexer is the natural decomposition point, dissolving the "massive v1
scope" problem into a stack of independently functional layers.

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
around it: in the new model the `AlknetEndpoint` hands the handler a
`Connection` (quinn/iroh QUIC), and the handler is responsible for
opening/accepting the bidirectional QUIC stream that carries the SSH-2
protocol. The same handler can equally be reached via a WebTransport stream
proxied through the `h3` ALPN-stream-proxy (ADR-040) — the handler sees a
`Connection` either way, and SSH doesn't care.

The reference implementation reportedly has ~3.5k clones in 14 days on the
GitHub push mirror (30-60 unique clones/day, a mix of bots and humans/LLMs
inspecting it). There is real-world demand for the "SSH-over-arbitrary-stream"
capability. The greenfield rewrite is a total rewrite; the vault was initially
copied and also since rewritten.

## Sources Investigated

| Source | Path | Note |
|--------|------|------|
| Existing arch docs (core) | `docs/architecture/crates/core/*` | ProtocolHandler, Connection, BiStream, AuthContext, IdentityProvider, Endpoint |
| Existing arch docs (http) | `docs/architecture/crates/http/*` | WebTransport substrate, ALPN-stream-proxy — **new since initial research** |
| Existing ADRs 001–043 | `docs/architecture/decisions/*` | ADR-002/007/010/004/011 (core); **ADR-038/040/043 (WebTransport, new)** |
| russh reference deep-dives | `docs/research/references/ssh/russh/01-06` | Overview, keys, protocol, crypto, internals, usage |
| russh-sftp reference deep-dives | `docs/research/references/ssh/russh-sftp/01-07` | SFTP protocol, client/server API, data flow |
| russh source (authoritative) | `/workspace/russh/` | `Cargo.toml` version `0.60.2`, edition 2024, MSRV 1.85. The cargo registry cache only contains `russh-0.49.2`; **use `/workspace/russh/` as canonical.** |
| russh-sftp source | `/workspace/russh-sftp/` | SFTP subsystem implementation, WASM-targeted protocol parsing |
| alknet Cargo.lock | `Cargo.lock` | Does not yet contain a russh entry |
| Reference implementation | `/workspace/@alkdev/alknet-main/` | `crates/alknet-core/src/{interface/ssh.rs, server/*, client/*, socks5/*}` |
| Concrete consumer | `/workspace/@alkdev/dispatch/` | axum + `russh = "0.60"` SSH **client** for "reverse git runner" over Docker/vast.ai. Textbook consumer of the SSH client + forwarding primitives. |

> **Note on the russh clone**: `/workspace/russh` declares `version = "0.60.2"`
> with `edition = "2024"` and MSRV 1.85 — matching the research references.
> The cargo-cache mismatch (0.49.2 only) matters because 0.49.2 → 0.60.2 spans
> major API changes (`server::run_stream` generic signature, `Auth` enum
> shape, `server::Handler` method set all differ). When alknet-ssh's
> `Cargo.toml` pins `russh = "0.60"`, Cargo will fetch the matching 0.60.x.

## The Channel Decomposition (Core Insight)

The initial research framed alknet-ssh's scope as a single massive v1: server
+ client + SOCKS5 + bidirectional port forwarding, all at once. That framing
made the crate feel unmanageably large and produced hedging language
("v1 default," "can be revisited later," "two-way door, decide later") that
proposed shipping non-functional or half-built versions. This revision
dissolves that problem by recognizing that **SSH's channel multiplexer is the
natural decomposition point**, and the features that felt like a massive scope
are layers that stack on top of it — each functional on its own.

### How SSH channels work

SSH multiplexes multiple logical channels over a single encrypted transport
stream (RFC 4254). `ChannelId(u32)` identifies channels; all channel traffic
(`CHANNEL_OPEN`/`DATA`/`EOF`/`CLOSE`/...) is interleaved on the single
underlying SSH transport. This is **independent of QUIC's own stream
multiplexing** — one QUIC bistream (or one WebTransport stream, or one TCP
connection) ↔ one SSH connection ↔ many SSH channels riding inside it.

The crucial property: **channel types are negotiated.** If one side requests a
channel type the other doesn't implement, the request is rejected with an
error. This means a partial channel implementation is not "broken" — it
correctly negotiates the types it supports and rejects the ones it doesn't.
This is the opposite of a half-built protocol; it's a layered protocol where
each layer stands on its own.

### The layer stack

```
Layer 7: SFTP subsystem          (channel type: "subsystem", name: "sftp")
Layer 6: SOCKS5 server            (consumer of Layer 5 — opens direct-tcpip channels)
Layer 5: Port forwarding          (channel types: "direct-tcpip", "forwarded-tcpip")
Layer 4: Session / exec           (channel type: "session"; exec/shell/pty requests)
Layer 3: Channel multiplexer      (russh internal — CHANNEL_OPEN/DATA/CLOSE)
Layer 2: SSH connection           (key exchange, auth, encrypted session)
Layer 1: Stream transport         (QUIC bistream / WebTransport stream / TCP)
```

Each layer is functional when built:

- **Layers 1-4** (stream + SSH connection + channels + session/exec): a working
  SSH server that authenticates and runs commands. This is immediately useful
  — it's the dispatch "reverse git runner" primitive (`exec` on a session
  channel) and the foundation everything else builds on.
- **+ Layer 5** (port forwarding): add `direct-tcpip` (local→remote) and
  `forwarded-tcpip`/`tcpip_forward` (remote→local) channel types. Now the SSH
  connection can forward ports in both directions. Each forwarded connection is
  a channel, not a separate transport stream. This unlocks the VPN-like
  topology (WireGuard + Postgres + Redis over SSH forwarding) that the reference
  implementation was built for.
- **+ Layer 6** (SOCKS5): a SOCKS5 server that accepts local connections and
  opens `direct-tcpip` channels to forward them. It's a *consumer* of the
  forwarding API, not a new channel type — SOCKS5 is a protocol spoken on the
  *client side* (the entity that wants to proxy), and the forwarding channel
  is what carries the bytes. This is where the "maybe a separate crate"
  question lives: SOCKS5 is a consumer of Layer 5's API, so if that API is
  clean, SOCKS5 can be in alknet-ssh or extracted — a two-way door.
- **+ Layer 7** (SFTP): a subsystem channel ("subsystem", name "sftp") that
  runs the SFTP protocol. `russh-sftp::server::run` takes the channel's stream
  (`channel.into_stream()` → `AsyncRead + AsyncWrite + Unpin + Send`) and a
  handler. It's another channel-layer consumer, stacking on Layer 3/4.

**No layer ships broken.** You build 1-4, ship a working SSH+exec appliance.
You add 5, ship a working SSH+forwarding appliance. You add 6, ship a working
  SSH+SOCKS5 proxy. You add 7, ship SFTP. Each increment is a complete,
functional SSH server for the channel types it supports — and a clean
rejection for the ones it doesn't. This is decomposition, not phasing: there
is no "phase 1 ships something that can't be used."

### What this means for the crate boundary

The decomposition clarifies which pieces are "foundational to SSH" vs
"consumers of SSH":

- **Foundational (in alknet-ssh)**: Layers 1-5. The stream transport, SSH
  connection, channel multiplexer, session/exec, and port forwarding are the
  SSH protocol itself. Forwarding (`direct-tcpip`/`forwarded-tcpip`) is
  defined by RFC 4254 §7; it's not an add-on, it's part of the protocol.
- **Consumer (in alknet-ssh or extractable)**: Layers 6-7. SOCKS5 and SFTP are
  *consumers* of the channel API. SOCKS5 is a proxy protocol that opens
  forwarding channels; SFTP is a file protocol that runs over a subsystem
  channel. Both could live in alknet-ssh or in separate crates — the decision
  is a two-way door because they consume a clean interface (the channel/stream
  API), so extraction is cheap if a second consumer appears.

The "maybe a separate socks proxy crate, and maybe not" question is answered
by this framing: **start with SOCKS5 in alknet-ssh** (the VPN-like use case
needs it there), and extract only if a second consumer of the forwarding API
appears — the stream-agnostic philosophy makes extraction cheap. SFTP is the
same: start with it as a subsystem the SSH handler can serve, extract only if
warranted. Neither is deferred; both are built as stacking layers.

## What's Changed Since Initial Research

Three things changed between the initial 2026-06-25 research and this
revision:

### 1. WebTransport is now architecturally grounded

ADRs 038 (HTTP/3 + WebTransport as first-class), 040 (WebTransport
ALPN-stream-proxy), and 043 (WebTransport as a bidirectional ALPN transport
substrate) now exist. The path "a browser opens a WebTransport session to
`/alknet/ssh`, the `h3` handler proxies the stream to `SshAdapter::handle()`,
the browser runs a WASM SSH client over the stream" is no longer speculative
— the substrate is specified. ADR-040 Assumption 2 states the constraint
explicitly: *the target ALPN handler accepts a proxied `Connection`; if a
handler assumes its `Connection` came from a specific QUIC source, it breaks
the proxy.* alknet-ssh must not assume its stream came from `accept_bi()` on a
native QUIC connection — it could be a WebTransport stream wrapped as a
`Connection`.

This is a **constraint on alknet-ssh's design**, not a feature to add later:
the handler's stream-acquisition path must be source-agnostic from the start.
The `tokio::io::join(recv, send)` adapter works identically whether the halves
came from a QUIC bistream or a WebTransport stream — both produce
`AsyncRead + AsyncWrite + Unpin + Send`. The constraint is satisfied by
construction if alknet-ssh uses the `BiStream`/`Connection` abstraction rather
than reaching for concrete quinn types.

### 2. The SSH client can run in WASM

The initial research (DP-7) framed tokio as a hard transitive dependency and
treated WASM as a one-way-door closure on the server side (OQ-09). That's
correct for the *server* dispatch path (the accept loop uses `tokio::spawn`,
the endpoint is quinn-bound), but **incorrect for the client side.**
Verifying against `/workspace/russh/russh-util/src/runtime.rs`:

```rust
#[cfg(target_arch = "wasm32")]
macro_rules! spawn_impl { ($fn:expr) => { wasm_bindgen_futures::spawn_local($fn) }; }
#[cfg(not(target_arch = "wasm32"))]
macro_rules! spawn_impl { ($fn:expr) => { tokio::spawn($fn) }; }
```

russh's `spawn` swaps to `wasm_bindgen_futures::spawn_local` on `wasm32`, and
`russh-util/src/time.rs` swaps to a chrono-based `Instant` on WASM. The client
`connect_stream<H, R>(config, stream, handler)` path takes a generic
`R: AsyncRead + AsyncWrite + Unpin + Send + 'static` — if the stream is
provided externally (a WebTransport `BiStream` implemented in WASM), the
client state machine runs in WASM. The `russh-sftp` protocol parsing already
targets WASM, confirming the pattern.

**The browser case is real:** a browser connects via WebTransport to
`/alknet/ssh`, the hub's `h3` handler proxies the stream to `SshAdapter`, and
the browser runs a WASM build of the alknet-ssh **client** (russh client +
`connect_stream` over a WebTransport `BiStream`) to speak SSH over the proxied
stream. The browser doesn't open native ports — it sends packets over the SSH
protocol, which forwards them as channels. The server side stays tokio-native
(the accept loop, the endpoint); the client side is the WASM target.

This reframes DP-7: tokio is a hard dependency for the **server** path, but
the **client** path is WASM-compatible because russh already abstracted its
runtime. alknet-ssh's client API must not reach for tokio-specific types
(`TcpStream`, `tokio::net`) in its public surface — the client should take a
stream, like russh's `connect_stream` does, so a WASM build can feed it a
WebTransport `BiStream`.

### 3. The http crate intersection is now visible

The alknet-http specs are drafted (ADR-036 through ADR-043). The
ALPN-stream-proxy (ADR-040) means `alknet-http`'s `h3` handler holds a
`HandlerRegistry` reference and routes WebTransport streams to ALPN handlers by
CONNECT path. alknet-ssh is one of those handlers. This is a structural
relationship: alknet-ssh doesn't depend on alknet-http, but alknet-http's
WebTransport path depends on alknet-ssh (and every other ALPN handler) being
source-agnostic about its `Connection`. The specs must be consistent on this
point — ADR-040 Assumption 2 is the contract both crates must honor.

## Straightforward Parts

These are settled by existing ADRs, the reference implementation, and the
channel decomposition. Phase 1 should document them as spec rather than
re-litigate them.

### 1. SSH is a `ProtocolHandler` on `alknet/ssh`

Confirmed by overview.md's ALPN Registry and core-types.md. `SshAdapter`
implements `ProtocolHandler::handle(&self, connection: Connection, auth:
&AuthContext) -> Result<(), HandlerError>` with `alpn() = b"alknet/ssh"`. The
handler owns the entire `Connection` lifecycle (ADR-006: one ALPN, one
connection, one handler) and may open/accept multiple QUIC streams because it
multiplexes SSH channels inside a single bistream.

### 2. SSH runs over a single bidirectional stream — source-agnostic

The reference implementation's `transport/iroh_transport.rs` proves the
approach: open a QUIC bistream, **join the two halves into a single duplex
type with `tokio::io::join(recv, send)`** and feed that to russh. This is a
one-liner:

```rust
// from alknet-main/.../iroh_transport.rs:94
let conn = self.endpoint.connect(self.node_id, ALPN).await?;
let (send, recv) = conn.open_bi().await?;
Ok(io::join(recv, send))   // produces: AsyncRead + AsyncWrite + Unpin + Send
```

`tokio::io::join` already produces the `AsyncRead + AsyncWrite` combo russh
requires (russh internally re-splits via `tokio::io::split`). **No custom
adapter struct is required** — `Connection::accept_bi()` / `open_bi()` plus
`tokio::io::join` is sufficient for the QUIC path, and the same `join` pattern
works for a WebTransport stream wrapped as a `Connection` (ADR-040).

This is now a **constraint**, not just a finding: per ADR-040 Assumption 2,
the handler must accept a `Connection` that came from a WebTransport stream,
not assume it came from a native QUIC `accept_bi()`. The `BiStream`/`Connection`
abstraction (ADR-007) is what makes this work — alknet-ssh must use it, not
reach for concrete quinn types.

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

### 4. SSH channels multiplex *inside* the stream — this is the decomposition axis

`ChannelId(u32)` identifies channels; all channel traffic is interleaved on
the single underlying SSH transport stream that russh owns. Port forwarding
(`direct-tcpip`, `forwarded-tcpip`) is ordinary channel traffic — each
forwarded TCP connection is a channel, not a separate stream. SFTP is a
subsystem channel. SOCKS5 is a consumer of forwarding channels.

This is the cleanest mapping and the right default: alknet-ssh does not try to
map SSH channels onto QUIC streams (which would require bypassing russh's own
multiplexer). It hands russh one bistream and lets russh multiplex inside it.
**The channel multiplexer is the decomposition point** — each feature
(forwarding, SOCKS5, SFTP) is a channel type or a consumer of channel types,
stacking on Layer 3. See "The Channel Decomposition" above.

### 5. Auth routes through the shared `IdentityProvider`

ADR-004 establishes the hybrid auth model: the endpoint resolves what it can
(TLS client cert → fingerprint), the handler resolves what it must (SSH key
fingerprint). `auth.md` shows the `SshAdapter` pattern exactly — constructor-
inject `Arc<dyn IdentityProvider>`, call `resolve_from_fingerprint()` inside
`handle()` when `auth.identity` is `None`, store the resolved `Identity` on the
`Connection` via `set_identity()` for observability (OQ-11). The
`ConfigIdentityProvider` already resolves SSH key fingerprints against
`DynamicConfig::auth::authorized_keys_fingerprints`. No new auth machinery
is needed for SSH.

### 6. Outbound credentials (if any) come from `Capabilities`

ADR-014 / ADR-022 establish that handlers get outbound credentials through the
registration bundle's `capabilities` field, populated by the assembly layer
from the vault. SSH itself typically needs no outbound credentials (the SSH
host key is a network-identity concern, the SSH *client* key for auth comes from
the peer), but if alknet-ssh ever needs an outbound secret (e.g., to dial an
upstream SOCKS proxy), it comes from `Capabilities`, not from env vars or
vault-on-wire.

### 7. TCP SSH is a handler concern, not an endpoint concern

ADR-010 is explicit: "TCP is NOT an endpoint concern... the SSH handler can
listen on a TCP socket independently." This means alknet-ssh may optionally bind
a plain TCP listener (port 22-style) and accept raw SSH connections *outside*
the ALPN endpoint. The `alknet/ssh` ALPN path and the bare-TCP path can coexist;
they share the same `russh::server::Config` and the same `server::Handler`
implementation, differing only in how the stream is obtained. This is a
two-way-door additive capability — the TCP listener can be added without
touching the ALPN path.

### 8. The WebTransport path is grounded — SSH-over-WebTransport is a constraint

Per ADR-040/043, the `h3` handler proxies WebTransport streams to ALPN
handlers. A browser opening a WebTransport session to `/alknet/ssh` gets its
stream handed to `SshAdapter::handle()` as a `Connection`. The browser runs a
WASM SSH client (the alknet-ssh client, built for `wasm32`) over the stream.
The handler must be source-agnostic about its `Connection` — this is a
constraint on the design, satisfied by using the `BiStream`/`Connection`
abstraction rather than concrete quinn types. **This is no longer an open
question; it's a requirement.**

## Less Straightforward Parts (Decision Points)

These are the points where Phase 0 surfaced genuine choices that affect the
architecture. Each is tagged with a door type per ADR-009. The Architect
should turn the *accepted* recommendations into ADRs, and the genuinely
unresolved ones into open questions. **Door type classifies reversal cost, not
urgency — a two-way door is a decision made now that can be reverted later,
not a decision to defer** (ADR-009 §"What this framework is NOT").

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
mnemonic can supply a key file. Phase 1 should write an ADR for this and a
corresponding OQ if the exact config-field shape is unresolved.

### DP-2: Per-connection host key selection
*(Recommended: one-way door — needs an ADR, ties to DP-1)*

When supporting multiple host keys (e.g., an Ed25519 default + an RSA key for
legacy clients), russh's `server::Config.keys` is a `Vec` and russh negotiates
which to use based on the client's offered algorithms. The selection is
deterministic per-russh-version but not configurable per-connection. Question:
do we need per-peer host key selection (e.g., present different host keys to
different peer networks)? **No** — one host key set per node, advertised
uniformly. Per-connection selection is not needed; if a use case arises, it's
an additive two-way-door. Phase 1 records the simple model.

### DP-3: Crypto backend — `aws-lc-rs` (default) vs `ring`
*(Recommended: two-way door — decided: `aws-lc-rs`, can flip later)*

russh 0.60.2 requires exactly one of `aws-lc-rs` (default) or `ring` enabled;
enabling both silently picks `aws-lc-rs`. Both produce AES-GCM / ChaCha20-Poly1305.

- `aws-lc-rs` is the russh default, has broader algorithm coverage, but brings
  NIST build machinery (a heavier build, requires a C compiler + cmake).
- `ring` is lighter-weight, smaller binary, simpler build.
- **Cross-crate consequence**: alknet-core already depends on `rustls-acme =
  "0.12"` with `features = ["aws-lc-rs"]`, so `aws-lc-rs` is already in the
  workspace's build. Choosing `ring` for russh while alknet-core uses
  `aws-lc-rs` would put *both* crypto backends in the final binary — wasteful
  but not incorrect.

**Recommendation**: **`aws-lc-rs`** (aligns with the rest of the workspace and
avoids a duplicate crypto backend). This is a decision, not a deferral — it's
a two-way door that can be flipped by changing `default-features = false` on
russh if binary-size pressure arises later. Phase 1 notes this; likely not a
full ADR (it's a default, not a structural decision) but a documented design
choice in the ssh spec.

### DP-4: Client + forwarding + SOCKS5 + SFTP scope — reframed as layer order
*(Recommended: one-way door on "all in alknet-ssh"; two-way door on extraction)*

The initial research framed this as "is all of this in v1?" — a massive scope
question. The channel decomposition dissolves it. The question is not "do we
ship it all at once" but "what's the build order, and are all the layers in
alknet-ssh?"

**Server side** (the `ProtocolHandler` for `alknet/ssh`): owns Layers 1-5
(stream transport, SSH connection, channels, session/exec, port forwarding).
These are the SSH protocol itself. Forwarding is defined by RFC 4254 §7 — it's
not an add-on. The server also serves SFTP (Layer 7) as a subsystem channel
when configured.

**Client side** (outbound SSH dialing): owns the same layers, as a client. The
client opens session channels for `exec` (the dispatch "reverse git runner"
pattern), opens `direct-tcpip` channels for local→remote forwarding, and
requests `tcpip_forward` for remote→local forwarding. **The client is the WASM
target** — russh's `connect_stream` runs in WASM when fed a WebTransport
`BiStream`. This is why the client lives in alknet-ssh, not in each consumer:
dispatch and the VPN-like topology both consume the same client + forwarding
primitives, and the browser case needs the client in WASM.

**SOCKS5** (Layer 6): a consumer of the forwarding API. The SOCKS5 server
accepts local connections and opens `direct-tcpip` channels to forward them.
It lives in alknet-ssh because the VPN-like use case needs it there; if a
second consumer of the forwarding API appears, the SOCKS5 codec can extract to
a tiny `alknet-socks5` crate (consuming a byte stream) — a two-way door, cheap
because the interface (the forwarding channel API) is clean.

**SFTP** (Layer 7): a subsystem channel. `russh-sftp::server::run` takes the
channel's stream and a handler. It's in alknet-ssh as a subsystem the server
can serve; the client side uses `russh-sftp::client::SftpSession` over a
channel stream. Same extraction logic as SOCKS5 — start in alknet-ssh, extract
only if warranted.

**Recommendation**: alknet-ssh owns **all layers** (server + client +
forwarding + SOCKS5 + SFTP). The build order is 1-4 first (functional SSH+exec),
then 5 (forwarding), then 6 (SOCKS5) and 7 (SFTP) — each layer functional when
built, none shipped broken. Phase 1 writes an ADR confirming this scope and the
layered build order. The extraction question (SOCKS5/SFTP to separate crates)
is a two-way door, decided as "in alknet-ssh, extract if a second consumer
appears" — a decision, not a deferral.

### DP-5: Channel-policy surface — which SSH services does alknet-ssh expose?
*(Recommended: one-way door — needs an ADR; the default-deny baseline is
non-negotiable)*

russh's `server::Handler` defaults every channel-request method to reject/no-op
(or, for `auth_publickey_offered`, accept the offer through to signature
verification). alknet-ssh must decide its default channel policy:

- **session channels**: the dispatch use case uses `channel_open_session().exec()`
  heavily — the "reverse git runner" pattern (run a command on the remote
  instance, capture stdout/stderr/exit). For the **server side** of
  `alknet/ssh`, the question is whether alknet-ssh *runs a real shell* on its own
  node. Given the VPN-like / forwarding use case is primary and the "shell
  server" use case is secondary, the default is **exec-only**:
  `shell_request` and `pty_request` default-reject; `exec_request` permitted
  (gated by ACL). This keeps alknet-ssh a focused forwarding/exec appliance
  rather than a general-purpose interactive login server. Interactive shell is
  an explicit opt-in (two-way door).
- **port forwarding in both directions** (`direct-tcpip` in, `tcpip_forward` /
  `forwarded-tcpip` out): in scope (Layer 5). The *policy* (which destinations
  are allowed, whether to restrict by ACL/scope) needs specifying.
- **SFTP subsystem**: in scope (Layer 7), gated by ACL.
- **PTY/X11/agent forwarding**: default-reject for security; explicit opt-in.
  (Consistent with the exec-only session stance.)

**Default-deny baseline**: russh's `server::Handler` already defaults every
channel/auth/forwarding callback to reject or no-op — so alknet-ssh gets
default-deny for free by overriding only the methods it wants to enable. This
is the explicit baseline: every forwarding destination, every exec command,
every channel type must be *explicitly permitted* by config + ACL, never
implicitly allowed. This applies to **both** the ALPN/QUIC path and the
bare-TCP path (DP-10) — a TCP-listener client gets exactly the same policy
treatment; only the transport differs.

**ACL gating**: forwarding destinations and exec commands are gated by scopes on
the resolved `Identity`. The exact scope vocabulary (e.g., `ssh:forward:*`,
`ssh:forward:127.0.0.1:5432`, `ssh:exec:git-upload-pack`) is a design choice the
Architect makes — likely a small, capability-shaped scope set with wildcards,
consistent with `Identity.scopes` / `Identity.resources` (auth.md). The
"resources" field on `Identity` (populated only by composition per
`CompositionAuthority::as_identity`, ADR-022) is *not* available to
fingerprint/token-resolved external identities, so per-destination ACLs for
inbound SSH must live in `scopes`, not `resources`.

**Recommendation**: Phase 1 writes an ADR defining the channel-policy surface:
exec (gated) + bidirectional port forwarding (gated) + SFTP (gated), with
shell/PTY/X11/agent forwarding default-rejected. Default-deny baseline
inherited from russh. Forwarding destinations + exec commands gated by ACL
scopes. The exact scope vocabulary is an OQ for Phase 1 (it interacts with how
operators express "allow forwarding to 127.0.0.1:5432" in `DynamicConfig`).

### DP-6: Auth method coverage — publickey-only vs password/kbdint too
*(Recommended: two-way door — decided: publickey-only, extend later if needed)*

russh supports `none`, `password`, `publickey`, `keyboard-interactive`, and
OpenSSH certificate auth server-side. alknet's identity model (auth.md) is
*fingerprint-based* — SSH key fingerprint → `IdentityProvider` → `Identity`.
This maps naturally onto **publickey** (the fingerprint is the SHA-256 of the
presented public key) and **OpenSSH certificate** auth (cert fingerprint).
Password / keyboard-interactive don't fit the fingerprint model as cleanly
(there's no `resolve_from_password` on `IdentityProvider`).

**Recommendation**: **publickey-only** (and certificate auth, which is a
superset of publickey from the fingerprint POV). Password /
keyboard-interactive are a two-way door — can be added later if a use case
arises. Phase 1 notes this as a documented design choice in the ssh spec,
likely not a full ADR (it's a default, not a structural decision).

### DP-7: Runtime — tokio (server) vs WASM-compatible (client)
*(Recommended: acknowledged constraint — server needs tokio, client is
WASM-compatible)*

russh 0.60.2 uses `russh-util::runtime::spawn`, which swaps to
`wasm_bindgen_futures::spawn_local` on `wasm32` and `tokio::spawn` otherwise.
`russh-util::time::Instant` swaps to a chrono-based implementation on WASM.
This means:

- **Server side** (the `ProtocolHandler` accept path): requires tokio. The
  endpoint's accept loop uses `tokio::spawn`, the `Connection` is quinn-bound,
  and the dispatch path is a one-way door away from WASM (OQ-09). alknet-ssh's
  server inherits this — it runs inside the tokio runtime that alknet-core's
  endpoint already provides (`tokio = { version = "1", features = ["full"] }`).
- **Client side** (outbound dialing / the WASM target): WASM-compatible. The
  client `connect_stream` path takes a generic stream; if the stream is a
  WebTransport `BiStream` implemented in WASM, the client state machine runs in
  WASM. **alknet-ssh's client API must not reach for tokio-specific types**
  (`TcpStream`, `tokio::net`) in its public surface — it should take a stream,
  like russh's `connect_stream` does, so a WASM build can feed it a
  WebTransport `BiStream`. The browser runs the alknet-ssh client in WASM to
  speak SSH over the proxied WebTransport stream (ADR-040/043).

**Recommendation**: Phase 1 records the split: server = tokio (hard
constraint, consistent with workspace), client = WASM-compatible (russh
already abstracted its runtime; alknet-ssh's client API preserves this by
taking a stream, not a socket). This is a known constraint, not a decision to
fight. OQ-09 (WASM boundaries) documents the server-side closure; the
client-side WASM compatibility is a new finding that keeps the browser door
open.

### DP-8: The `ssh-key` crate is forked
*(Recommended: acknowledged constraint — use the russh re-export)*

russh 0.60.2 depends on `internal-russh-forked-ssh-key = "0.6.18"` (a renamed
fork), **not** upstream `ssh-key`. alknet-ssh must not add upstream `ssh-key`
directly — that would put two `ssh-key` versions in the tree and the
`PublicKey`/`PrivateKey` types wouldn't unify. The fork is re-exported through
`russh::keys::ssh_key`, so alknet-ssh should always reach key types via
`russh::keys::*` (or `russh::keys::ssh_key::*`) to stay on the same fork. Phase
1 notes this as an implementation constraint; it's a real footgun if missed.

### DP-9: End-to-end over a non-TCP stream is untested upstream
*(Recommended: de-risk early with a POC test)*

russh's own test suite only exercises the client↔server round trip over real
TCP loopback. There is **no** test connecting `connect_stream` ↔ `run_stream`
over `tokio::io::duplex()` or any other in-memory pipe. The `SshRead::read_ssh_id`
unit tests feed `&[u8]` directly, proving the banner parser works on
non-socket streams — but a full client↔server round trip over a non-TCP stream
is unverified upstream.

The reference implementation uses this path in production (`transport/iroh_transport.rs`
using `tokio::io::join`), which is strong empirical evidence it works. But the
greenfield rewrite should **close this gap early** with an integration test
using `tokio::io::duplex()` connecting `connect_stream` ↔ `run_stream` *before*
going near real QUIC. **The WebTransport path adds a second POC target**: a
WebTransport stream wrapped as a `BiStream`/`Connection` fed to `run_stream`,
validating the ADR-040 Assumption 2 contract (the handler accepts a proxied
`Connection`).

**Recommendation**: per `sdd_process.md` Phase 0, this is a candidate for a
POC Specialist task (`.worktrees/research/ssh-stream-poc/`). Two POC scenarios:
(1) `duplex()`-based round trip, (2) WebTransport-stream-as-`Connection` →
`run_stream`. Phase 1's architecture docs reference the POC outcomes. If the
POC surfaces issues (half-open stream handling, `poll_shutdown` semantics,
maximum packet size), they feed back into the spec as constraints.

### DP-10: Bare-TCP SSH listener — first-class path for git-over-SSH
*(Recommended: one-way door on the config shape, two-way door on the listener
itself)*

ADR-010 establishes that bare-TCP SSH is a handler concern — the SSH handler
can listen on a TCP socket independently of the `alknet/ssh` ALPN path.
Git-over-SSH (`ssh git@host ...`) runs on TCP port 22, not over QUIC — git
clients (`git`, libgit2, `gix`) dial a TCP socket and expect the SSH-2 protocol
directly. To make alknet-ssh a viable git-over-SSH target, the bare-TCP listener
must be a first-class path.

The two paths (ALPN/QUIC vs bare-TCP) share the same `russh::server::Config` and
the same `server::Handler` implementation; they differ only in how the duplex
stream is obtained:

- **ALPN path**: `handle()` receives the QUIC `Connection`, calls
  `accept_bi()`, `tokio::io::join`s the halves, hands to `run_stream`.
- **TCP path**: a `tokio::net::TcpListener` accept loop hands each accepted
  `TcpStream` directly to `run_stream` (or `run_on_socket`, keeping config/
  handler identical across both paths).
- **WebTransport path** (new): `handle()` receives a `Connection` wrapped from
  a WebTransport stream (ADR-040); same `run_stream` call, same config/handler.

All three paths share the same `server::Config` + `Handler` + ACL policy —
only the stream source differs. The TCP listener is **off by default** (must
be explicitly configured to bind), consistent with the default-deny posture.

**Recommendation**: Phase 1 records the three-path model in the ssh spec —
ALPN/QUIC primary, bare-TCP as a co-equal first-class path (off by default),
WebTransport as the browser path (via ADR-040). **Reserve the TCP-listener
config fields** (one-way door on the config schema — retrofitting is messier
than reserving the shape up front). The listener implementation is a two-way
door; the config shape is locked.

## Recommended Approach: Layered Build Order

Based on the channel decomposition and the decision points above, the
recommended approach to take into Phase 1:

### Crate

`alknet-ssh`, depends on `alknet-core` and `russh = "0.60"` (default features,
i.e. `aws-lc-rs`). Implements `ProtocolHandler` for `b"alknet/ssh"`. **Owns
both the SSH server and the SSH client** — the server is the `ProtocolHandler`;
the client is the shared primitive dispatch, the VPN-like topology, and the
browser-WASM case all consume. Owns all channel layers (1-7): stream
transport, SSH connection, channel multiplexer, session/exec, port
forwarding, SOCKS5, SFTP.

### Build order (each layer functional when built)

**Layer 1-4: SSH connection + channels + session/exec**
- Stream wiring: `handle()` accepts the `Connection`, calls `accept_bi()` (or
  receives a WebTransport-proxied stream), `tokio::io::join`s the halves, hands
  to `russh::server::run_stream`. Source-agnostic (ADR-040 constraint).
- Auth: constructor-injected `Arc<dyn IdentityProvider>`. Inside `handle()`, if
  `auth.identity` is `None`, russh's `server::Handler::auth_publickey` resolves
  the offered key's fingerprint through the provider; on success, store the
  resolved `Identity` on the `Connection` via `set_identity()` (OQ-11).
  Publickey-only (plus OpenSSH cert).
- Host keys (DP-1): vault-derived Ed25519 by default, optional config override.
- Channel policy: exec (gated) only; shell/PTY/X11/agent default-reject.
- Client: `connect_stream` over a provided stream (WASM-compatible); session
  channel `exec` for the dispatch "reverse git runner" pattern.
- **Result**: a working SSH+exec appliance (server + client). Immediately useful.

**Layer 5: Port forwarding (bidirectional)**
- `direct-tcpip` (local→remote) and `forwarded-tcpip`/`tcpip_forward`
  (remote→local) channel types, both gated by ACL scopes.
- Client-side: opens `direct-tcpip` channels (dispatch's `start_forward`
  pattern); requests `tcpip_forward` for remote→local.
- **Result**: a working SSH+forwarding appliance. The VPN-like topology
  (WireGuard + Postgres + Redis over SSH forwarding) works.

**Layer 6: SOCKS5 server**
- A SOCKS5 server that accepts local connections and opens `direct-tcpip`
  channels to forward them. Consumer of Layer 5's API.
- In alknet-ssh (the VPN-like use case needs it there). Extractable to
  `alknet-socks5` if a second consumer appears (two-way door).
- **Result**: a working SSH+SOCKS5 proxy. The reference implementation's
  SOCKS5 feature is preserved.

**Layer 7: SFTP subsystem**
- Server: `russh-sftp::server::run` over a subsystem channel's stream.
- Client: `russh-sftp::client::SftpSession` over a channel stream.
- In alknet-ssh; extractable if warranted (two-way door).
- **Result**: SFTP file transfer over SSH.

### De-risk POC (DP-9)

A Phase 0 POC validating `connect_stream` ↔ `run_stream` over
`tokio::io::duplex()`, plus a WebTransport-stream-as-`Connection` →
`run_stream` POC validating the ADR-040 contract. Timeboxed; if they pass, the
stream-wiring spec is straightforward; if they surface constraints, they fold
into the spec.

### Three-path model (DP-10)

ALPN/QUIC primary, bare-TCP co-equal (off by default, config reserved in the
schema for git-over-SSH), WebTransport as the browser path (ADR-040). All three
share `server::Config` + `Handler` + ACL; only the stream source differs.

## Open Questions to Carry into Phase 1

The following should become OQs in `docs/architecture/open-questions.md`
(numbering assigned by the Architect — likely OQ-41 onwards, since OQ-01–OQ-40
exist):

- **OQ-SSH-01 (host key sourcing)**: vault-derived default + config override —
  resolved by the DP-1 ADR. The exact config-field shape may stay open.
- **OQ-SSH-02 (channel policy v1 surface + default-deny scope vocabulary)**:
  the set of allowed channel types / request types is resolved by the DP-5
  ADR; the exact scope vocabulary for forwarding destinations + exec commands
  (e.g., `ssh:forward:127.0.0.1:5432` vs a resources-style shape) stays open —
  it interacts with how operators express allow-lists in `DynamicConfig` and
  with the fact that `Identity.resources` is composition-only (ADR-022).
- **OQ-SSH-03 (SOCKS5/SFTP extraction)**: confirm SOCKS5 and SFTP start in
  alknet-ssh and extract only if a second consumer of the forwarding/channel
  API appears — resolved (in favor of in-alknet-ssh-now, extract-later) by
  the DP-4 ADR. Two-way door.
- **OQ-SSH-04 (POC outcome)**: did the `duplex()`-based round-trip POC pass, and
  did the WebTransport-stream POC validate the ADR-040 contract? Resolved by
  POC Specialist results.
- **OQ-SSH-05 (client WASM surface)**: confirm alknet-ssh's client API takes a
  stream (not a socket), preserving the WASM door russh's runtime abstraction
  opened. This is a design constraint, not a deferral — the client must not
  reach for `tokio::net` types in its public surface.
- **OQ-SSH-06 (bare-TCP listener)**: config shape reserved; listener
  implementation is a two-way door. Git-over-SSH is the forcing function —
  decide based on whether the build needs to be a git-over-SSH target.

## Next Steps (Phase 0 → Phase 1)

1. **You decide** on the DP recommendations (or amend them). DP-1, DP-4, DP-5,
   DP-10 are the load-bearing architectural choices. DP-2, DP-3, DP-6, DP-7,
   DP-8 are defaults recommended as-is; DP-9 is a POC task.
2. **POC** (DP-9): spawn a POC Specialist to validate `connect_stream` ↔
   `run_stream` over `tokio::io::duplex()` and the WebTransport-stream path.
   Timeboxed; if it passes, the stream-wiring spec is straightforward; if it
   surfaces constraints, they fold into the spec.
3. **Phase 1 (Architect)**: produce `docs/architecture/crates/ssh/README.md` +
   component specs organized by channel layer (e.g., `ssh-stream.md` for
   Layer 1, `ssh-connection.md` for Layer 2, `ssh-channels.md` for Layer 3,
   `ssh-exec.md` for Layer 4, `ssh-forwarding.md` for Layer 5, `ssh-socks5.md`
   for Layer 6, `ssh-sftp.md` for Layer 7, `ssh-client.md` for the client/WASM
   path, `ssh-tcp-listener.md` for the bare-TCP path), ADRs for the accepted DPs
   (host-key sourcing, channel policy + default-deny, ssh server+client+
   forwarding+socks5+sftp scope + layered build order, bare-TCP config shape),
   and the OQs above in `open-questions.md`. Update `docs/architecture/README.md`
   index and ADR table.

## References

- `docs/sdd_process.md` — Phase 0 process definition
- `docs/architecture/overview.md` — ALPN-as-service, crate graph, ProtocolHandler
- `docs/architecture/crates/core/core-types.md` — ProtocolHandler, Connection, BiStream
- `docs/architecture/crates/core/auth.md` — AuthContext, IdentityProvider, SshAdapter example
- `docs/architecture/crates/http/webtransport.md` — WebTransport substrate spec
- `docs/architecture/decisions/001-alpn-protocol-dispatch.md` — ALPN dispatch
- `docs/architecture/decisions/002-protocol-handler-trait.md` — ProtocolHandler trait
- `docs/architecture/decisions/004-auth-as-shared-core.md` — hybrid auth
- `docs/architecture/decisions/007-bistream-type-definition.md` — BiStream trait
- `docs/architecture/decisions/010-alpn-router-and-endpoint.md` — endpoint, TCP-is-handler-concern
- `docs/architecture/decisions/014-secret-material-flow-and-capability-injection.md` — Capabilities
- `docs/architecture/decisions/022-handler-registration-provenance-and-composition-authority.md` — registration bundle
- `docs/architecture/decisions/025-vault-local-only-dispatch.md` — vault local-only
- `docs/architecture/decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md` — TLS identity model (symmetry reference for DP-1)
- `docs/architecture/decisions/038-http3-and-webtransport-as-first-class.md` — h3/WebTransport first-class
- `docs/architecture/decisions/040-webtransport-alpn-stream-proxy.md` — ALPN-stream-proxy (SSH-over-WebTransport path)
- `docs/architecture/decisions/043-webtransport-bidirectional-alpn-substrate.md` — WebTransport as bidirectional ALPN substrate
- `docs/research/references/ssh/russh/01-06` — russh deep-dives (overview, keys, protocol, crypto, internals, usage)
- `docs/research/references/ssh/russh-sftp/01-07` — russh-sftp deep-dives (overview, wire protocol, key types, client/server API, data flow, quick reference)
- `/workspace/russh/` — russh 0.60.2 source (authoritative; `russh-util/src/runtime.rs` shows the WASM runtime swap)
- `/workspace/russh-sftp/` — russh-sftp source (WASM-targeted protocol parsing)
- `/workspace/@alkdev/alknet-main/crates/alknet-core/src/` — reference implementation
  (`transport/iroh_transport.rs:94` shows the `tokio::io::join` adapter; `server/`,
  `interface/ssh.rs`, `client/`, `socks5/` for prior art)
- `/workspace/@alkdev/dispatch/` — concrete downstream consumer the user wants to
  replace with this stack: axum + `russh = "0.60"` SSH **client** for "reverse git
  runner" over Docker/vast.ai. `src/ssh.rs` (russh client wrapper, 143 lines),
  `src/handlers.rs::start_forward` (`channel_open_direct_tcpip` local→remote
  forwarding), `src/sftp.rs` (russh-sftp client). No SOCKS5 — that's the
  alknet-original feature preserved here. Dispatch is a textbook consumer of the
  alknet-ssh **client** + **forwarding** primitives, which is why those live in
  alknet-ssh rather than being duplicated per-consumer.