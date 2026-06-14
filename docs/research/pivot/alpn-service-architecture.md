# ALPN-as-Service Architecture

> Status: Research / Pivot Proposal
> Last updated: 2026-06-14

## Core Insight

**A service IS an ALPN.** Every protocol handler registers an ALPN string on a shared QUIC+TLS endpoint. The ALPN negotiation during the TLS/QUIC handshake is the dispatch mechanism — it routes the connection to the correct protocol handler before any application bytes are read.

This insight comes from observing how iroh already works: `Router` dispatches incoming QUIC connections to `ProtocolHandler` implementations based on the ALPN string. The reverse-proxy project at `/workspace/@alkdev/reverse-proxy` uses the same pattern for TLS (ALPN → h2 or http/1.1). Hickory DNS registers ALPN protocols (`dot`, `doq`, `h2`, `h3`) in its TLS/QUIC server configs. The pattern is universal.

## The ProtocolHandler Trait

A single trait replaces the current `StreamInterface` + `MessageInterface` split, the `ListenerConfig` enum, and the three-layer dispatch model:

```rust
#[async_trait]
pub trait ProtocolHandler: Send + Sync + 'static {
    /// The ALPN string this handler claims (e.g. b"alknet/ssh")
    fn alpn(&self) -> &'static [u8];

    /// Handle an incoming bidirectional QUIC stream
    async fn handle(&self, stream: BiStream, auth: &AuthContext) -> Result<()>;
}
```

`BiStream` is a joined `(SendStream, RecvStream)` implementing `AsyncRead + AsyncWrite`. `AuthContext` carries the authenticated identity (resolved during the TLS/QUIC handshake or from the first protocol frames, depending on the handler).

Every handler receives a byte stream and manages its own wire format. HTTP is just a handler on ALPN `h2`. DNS is just a handler on its own ALPN. SSH is a handler on `alknet/ssh`. The call protocol is a handler on `alknet/call`.

## Handler Registry

```
alknet Endpoint (QUIC + TLS on a single port)
  │
  ├── ALPN "alknet/ssh"     → SshAdapter
  │     • russh doing SSH-2 handshake, auth, channel multiplexing
  │     • direct-tcpip, forwarded-tcpip, streamlocal-forward
  │     • SOCKS5 server on the client side
  │     • This is the "gateway" — richest tunneling primitives
  │
  ├── ALPN "alknet/call"    → CallAdapter
  │     • JSON-RPC: call/request + streaming (subscribe)
  │     • EventEnvelope framing (length-prefixed JSON)
  │     • Operation registry, access control, pending request map
  │     • Cross-language: consumable from JS, Python, WASM, anything
  │
  ├── ALPN "alknet/git"     → GitAdapter
  │     • gix (Apache-2.0/MIT) for pack generation, ref resolution, object store
  │     • Custom pkt-line protocol adapter (~1000 lines)
  │     • Capability advertisement v2, ls-refs, fetch, receive-pack
  │     • No HTTP layer — git protocol directly over QUIC streams
  │
  ├── ALPN "alknet/sftp"    → SftpAdapter
  │     • russh-sftp protocol core (WASM-ready, transport-agnostic)
  │     • 26 packet types, custom serde codec, pure data transformation
  │     • Only read_packet() couples to I/O — easily adapted
  │     • Can compile to WASM for browser SFTP clients
  │
  ├── ALPN "alknet/msg"     → MessageAdapter
  │     • E2E encrypted direct messages (encrypt with recipient's public key)
  │     • Mixnet support (Chaum 1981): nested encryption, batch-and-reorder
  │     • Return addresses as digital pseudonyms
  │     • Replicators are naturally mixes — they already relay data
  │
  ├── ALPN "alknet/http"    → HttpAdapter
  │     • axum router with auth middleware
  │     • REST API, dashboard, MCP endpoint
  │     • Maps HTTP requests to call protocol operations
  │
  ├── ALPN "alknet/dns"     → DnsAdapter
  │     • hickory-proto for DNS wire format (#![no_std], WASM-compatible)
  │     • pkarr::SignedPacket for self-sovereign DNS (iroh-dns pattern)
  │     • Service discovery: _alknet.<z32-node-id>.alk.dev TXT
  │     • Control channel: AuthToken in query labels (censorship fallback)
  │     • Encrypted transports: DoT, DoQ, DoH3 via ALPN dispatch
  │     • Mainline DHT fallback for fully decentralized resolution
  │
  ├── ALPN "h3"             → WebTransportAdapter (wtransport)
  │     • Browser-compatible WebTransport (W3C standard)
  │     • Bidirectional streams + datagrams
  │     • Enables browser-to-head without SSH key exchange
  │     • AuthToken in CONNECT request headers
  │
  ├── ALPN "h2" / "http/1.1" → Standard HTTP
  │     • For browsers, curl, standard HTTP clients
  │     • Same axum router as alknet/http but on standard ALPNs
  │
  └── custom ALPNs          → third-party adapters
```

## What This Simplifies

### 1. The Interface Layer Collapses

No more `StreamInterface` vs `MessageInterface` split. No more `ListenerConfig` enum with three variants (`Stream`, `Http`, `Dns`). No more server accept loop handling three different listener types. Every handler receives a stream and manages its own protocol. HTTP is just a handler on ALPN `h2`. DNS is just a handler on its own ALPN. SSH is a handler on `alknet/ssh`.

### 2. The Call Protocol Becomes One ALPN Among Many

It's a generic JSON-RPC protocol that supports call/request and streaming. Services that need structured RPC register operations under `alknet/call`. Services with their own wire format (Git, SFTP, SSH) get their own ALPN. Cross-node calls go through `alknet/call`. In-process calls are direct.

### 3. OperationEnv's Three Dispatch Paths Collapse

Dispatch is "which ALPN does this belong to?" — not local/irpc/remote path selection. A handler that needs to call another service opens a stream on `alknet/call` and sends a `call.requested` envelope. The handler doesn't need to know whether the target is local, in-cluster, or cross-node.

### 4. Crate Decomposition Gets Simpler

A core crate provides the endpoint/router + auth/identity. Protocol crates register handlers. No need for trait interop without crate dependencies (the current alknet-storage-implements-IdentityProvider-without-depending-on-core pattern).

### 5. The WASM Story Gets Clean

If we assume a byte stream, protocol parsers compile to WASM:
- russh-sftp's protocol core is already WASM-ready (pure data transformation, no I/O)
- hickory-proto is `#![no_std]` with a `wasm-bindgen` feature
- The call protocol's JSON framing is inherently cross-language
- Git's pkt-line is simple enough to implement anywhere
- A browser gets a WebTransport stream and speaks SFTP, Git, or call protocol directly

### 6. The Reverse-Proxy ALPN Pattern Applies Directly

The reverse-proxy at `/workspace/@alkdev/reverse-proxy` does: TLS handshake → read ALPN → dispatch to h2 or http/1.1 handler. Alknet does the same, just with more ALPNs. The `ArcSwap<DynamicConfig>` pattern for hot-reloading handler routing works identically.

### 7. SSH Is the Tunneling Gateway, Not the Only Interface

SSH provides `direct-tcpip`, `forwarded-tcpip`, `streamlocal-forward` — rich tunneling primitives that other protocols don't have. But SSH is just one ALPN. A node that only needs Git can skip SSH entirely and use `alknet/git` directly. A browser can use `h3` (WebTransport) + `alknet/call` without SSH.

## Auth and Identity

Auth is shared across all handlers. The `IdentityProvider` trait resolves credentials to an `Identity`:

```rust
pub trait IdentityProvider: Send + Sync + 'static {
    fn resolve_from_fingerprint(&self, fingerprint: &str) -> Option<Identity>;
    fn resolve_from_token(&self, token: &AuthToken) -> Option<Identity>;
}
```

Each handler presents credentials differently, but all resolve through the same provider:

| Handler | Credential presentation | Resolves via |
|---------|------------------------|-------------|
| SshAdapter | SSH public key handshake | `resolve_from_fingerprint()` |
| CallAdapter | AuthToken in first frame | `resolve_from_token()` |
| HttpAdapter | `Authorization: Bearer` header | `resolve_from_token()` |
| DnsAdapter | AuthToken in query labels | `resolve_from_token()` |
| WebTransportAdapter | AuthToken in CONNECT headers | `resolve_from_token()` |
| GitAdapter | Signed push certificate | `resolve_from_fingerprint()` |

## Distributed Git via ERC721

The git adapter integrates with on-chain identity for decentralized repository management:

```
Creator mints RepoToken (ERC721) → on-chain metadata stores:
  • name, owner (UserToken)
  • authorized committers (list of UserTokens or key fingerprints)
  • optional: preferred replicator set

Committer pushes → GitAdapter verifies:
  1. Resolve signer's key → UserToken (via on-chain IdentityProvider)
  2. Check UserToken is in RepoToken's committer list
  3. Accept push, compute new refs
  4. Gossip update to replicator mesh

Replicator receives gossip → verifies independently → stores in local repo
```

Replicators are voluntary nodes that:
- Subscribe to gossip for specific repos (by token ID)
- Serve repos via the git ALPN
- Advertise which repos they replicate via pkarr/DNS records
- May also serve other ALPNs (dns, msg, call)
- Have donation addresses in node metadata (no protocol-level economics)

## Messaging Layers

Three distinct privacy layers on the same relay infrastructure:

| Layer | What's hidden | Mechanism | Use case |
|-------|---------------|-----------|----------|
| E2E encryption | Message content | Encrypt with recipient's public key | Direct messages, file transfer |
| Gossip + call protocol | Nothing hidden by design | Public broadcast, structured RPC | Group chat, repo notifications |
| Mixnet (Chaum 1981) | Sender/recipient metadata | Nested encryption, batch-and-reorder | Pseudonymous commits, anonymous tips |

The replicator doesn't care which layer it's serving. It sees "encrypted blob, forward to X" and does the same thing regardless. A replicator that handles git gossip is also a mix node is also a relay for e2e DMs — it's all encrypted bytes through the same relay infrastructure.

## Crate Decomposition

```
alknet-core          Endpoint, ALPN router, auth/identity, config, call protocol
alknet-ssh           SshAdapter (russh, SOCKS5, port forwarding)
alknet-call          CallAdapter (JSON-RPC, operation registry, access control)
alknet-git           GitAdapter (gix, pkt-line protocol)
alknet-sftp          SftpAdapter (russh-sftp protocol core)
alknet-msg           MessageAdapter (E2E encryption, mixnet)
alknet-http          HttpAdapter (axum, REST API)
alknet-dns           DnsAdapter (hickory-proto, pkarr, service discovery)
alknet-secret        BIP39/SLIP-0010/AES-GCM (standalone)
alknet-napi          Node.js native addon (call protocol client)
alknet               CLI binary (assembles everything)
```

Each protocol crate depends on `alknet-core` for auth/identity/config. No trait interop without crate dependencies. The CLI binary registers handlers with the endpoint.

## What Stays (Preserved from Current Implementation)

- **Transport implementations** (TCP, TLS, iroh) — become the endpoint's connection acceptors
- **SSH client/server** (russh, SOCKS5, port forwarding, channel proxy) — become `alknet-ssh`
- **Call protocol** (EventEnvelope framing, operation registry, access control, pending request map) — become `alknet-call`
- **Auth/identity** (Ed25519 keys, API keys, `IdentityProvider`, `ConfigIdentityProvider`) — shared in `alknet-core`
- **Dynamic config reload** (`ArcSwap<DynamicConfig>`, `ConfigReloadHandle`) — shared in `alknet-core`
- **Secret crate** (BIP39/SLIP-0010/AES-GCM, irpc service, key caching) — standalone `alknet-secret`
- **NAPI addon** (connect, serve, ThreadsafeFunction callbacks) — becomes call protocol client
- **Stealth mode** (byte-peek protocol detection) — replaced by ALPN negotiation (cleaner, no peeking needed)

## What Gets Dropped/Deferred

- **`StreamInterface` / `MessageInterface` traits** — replaced by `ProtocolHandler`
- **`ListenerConfig` enum** — replaced by ALPN advertisement configuration
- **`OperationEnv` three dispatch paths** — replaced by "call through `alknet/call` ALPN"
- **The metagraph data model** (alknet-storage) — defer until concrete need
- **The flowgraph crate** — defer, observability infrastructure
- **`CredentialProvider` phase progression** (A through D) — simplify to what's needed now
- **38 ADRs for unbuilt features** — archive, revisit if needed

## Migration Path

1. Define `ProtocolHandler` trait and ALPN router in `alknet-core`
2. Extract SSH code into `alknet-ssh` implementing `ProtocolHandler`
3. Extract call protocol into `alknet-call` implementing `ProtocolHandler`
4. Wire existing auth/config into shared `alknet-core`
5. Build GitAdapter on gix + pkt-line protocol
6. Build SftpAdapter on russh-sftp protocol core
7. Build HttpAdapter on axum
8. Build DnsAdapter on hickory-proto + pkarr
9. Build MessageAdapter (E2E + mixnet)
10. Integrate WebTransport via wtransport
11. Update NAPI to be a call protocol client
12. CLI binary registers all handlers

## Reference Projects

| Project | Path | Relevance |
|---------|------|-----------|
| iroh | `/workspace/iroh/` | ALPN-based protocol dispatch on QUIC endpoints |
| reverse-proxy | `/workspace/@alkdev/reverse-proxy` | ALPN routing (h2/http1.1), ArcSwap config, TLS acceptor pattern |
| russh-sftp | `/workspace/russh-sftp/` | Transport-agnostic protocol core, WASM-ready |
| hickory-dns | `/workspace/hickory-dns/` | DNS wire format (#![no_std]), ALPN-registered transports (dot/doq/h2/h3) |
| wtransport | `/workspace/wtransport/` | WebTransport (h3 ALPN), browser-compatible QUIC streams |
| rtc | `/workspace/rtc/` | Sans-I/O WebRTC (datachannel, RTP) — reference for P2P alternative |
| gix | crates.io | Apache-2.0/MIT git operations (pack generation, ref resolution, object store) |
