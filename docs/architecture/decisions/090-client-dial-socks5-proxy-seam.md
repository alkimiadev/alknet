# ADR-090: Client-Dial SOCKS5 Proxy Seam

## Status

Accepted (adds a capability to ADR-089's `AlknetClient`; raises OQ-67 for iroh)

## Context

### The privacy gap

A native client (`AlknetClient`, ADR-089) that dials a hub directly exposes
its real IP address to the hub (and to any relay on the path). This is the
same privacy exposure iroh documents for its own direct-connection case:
without a proxy, the peer and the relay see one's real IP. A client that
wants to keep its real IP private from the hub — the primary use case —
needs to route the dial through a trusted proxy that terminates the
network path and presents its own IP to the hub.

This is a first-class client capability, not a niche feature. The
old (`alknet-main`) client had a `ConnectOptions.proxy: Option<String>`
field for exactly this, and the iroh transport builder already took a
`proxy_url` parameter. But the old client's proxy support was
incomplete: the TCP/TLS transports never consumed the `proxy` field
(`TcpStream::connect` went direct), and only iroh's `proxy_url` was
wired. ADR-089's `AlknetClient` is the greenfield rewrite, and this is
the time to make the proxy a real, uniform, tested capability rather
than an aspirational field.

### Two distinct SOCKS5 uses — do not conflate

There are two unrelated SOCKS5 concepts in the alknet design space. This
ADR is about one of them; naming both prevents a conflation that would
distort the scope.

1. **Client-dial proxy** (this ADR). The *client* routes its outbound
   dial to the hub through a SOCKS5 proxy. The client owns the proxy
   config; the hub sees the proxy's IP. This is transport-layer privacy
   for the establishment side. Lives in `alknet-client` (the dial seam,
   ADR-089).

2. **SOCKS5-over-channels handler** (the planned `alknet-socks5` crate,
   not yet specced — ADR-085 scope table). One side *offers* a SOCKS5
   proxy service to the other over a channels data channel (ALPN
   `alknet/socks5`). The offering side runs a SOCKS5 server backed by
   channels `direct-tcpip`-equivalent opens; the consuming side tunnels
   it locally and uses it like any SOCKS5 server. This is a service, not
   a client-dial concern. Lives in `alknet-socks5` (foundational
   handler, rides inside channels). Out of scope here.

The two compose without coupling: a client that wants both privacy on
its dial *and* a SOCKS5 service from the hub sets its dial
`Socks5ProxyConfig` (this ADR) to its local SOCKS5 endpoint, and that
endpoint may itself be a local tunnel to the hub's `alknet/socks5`
channels service. The dial proxy doesn't know the SOCKS5 server is a
channels handler; the channels handler doesn't know its client is an
`AlknetClient` dial. The composition is at the SOCKS5 protocol level,
not the alknet type level. This ADR concerns only #1.

### QUIC-over-SOCKS5 is validated

The central technical question — can a quinn QUIC dial go through a
SOCKS5 proxy? — is settled by a de-risking PoC. QUIC is UDP; SOCKS5 UDP
ASSOCIATE (RFC 1928 §6/§7) carries UDP datagrams; quinn exposes a public
socket abstraction (`quinn::AsyncUdpSocket`) and a public constructor
(`quinn::Endpoint::new_with_abstract_socket`) that accept any impl. A
SOCKS5 UDP ASSOCIATE tunnel implemented as an `AsyncUdpSocket` is the
exact extension point. The PoC (`/workspace/quinn-proxy-poc`,
`docs/research/quinn-quic-proxy/findings.md`) runs end-to-end: a quinn
client completes the QUIC handshake and exchanges stream data with all
UDP traffic tunneled through a `fast-socks5` proxy (5/5 runs clean). The
identical pattern is in production in BotBrowser (QUIC/HTTP3 + STUN over
SOCKS5 UDP ASSOCIATE in Chromium's network stack).

The PoC surfaced two real, acceptable limitations:

- **ECN is lost across the proxy.** The SOCKS5 UDP header carries no
  ECN bits, so the proxied QUIC path falls back to non-ECN congestion
  control. A performance cost on congested links, not a correctness
  issue — QUIC degrades gracefully. For alknet's call/channels traffic
  (low-bandwidth, latency-sensitive, not throughput-critical), the cost
  is negligible. Anyone who chooses to route through a proxy already
  accepts added latency and the loss of path optimizations; this is an
  expected cost of the privacy choice, not a surprise.
- **The proxy must support UDP ASSOCIATE.** Not every SOCKS5 proxy does
  (notably `ssh -D` does not). This is a deployment requirement, not a
  code problem. The dial surfaces a clear error when the proxy rejects
  `UDP ASSOCIATE`; the operator chooses a UDP-capable proxy. The
  fallback policy (fall back to `dial_tcp_tls`, or fail closed) is a
  caller concern — see §6.

The per-datagram framing overhead (6–22 bytes of SOCKS5 header per
datagram, ~0.4–1.8% of a typical QUIC datagram) and the one extra TCP
control connection keeping the UDP association alive are both negligible.

### Why SOCKS5 only (no HTTP CONNECT)

SOCKS5 supports both TCP (CONNECT, RFC 1928 §3) and UDP (UDP ASSOCIATE,
§6) within one protocol. HTTP CONNECT is TCP-only with no UDP
equivalent. Supporting only SOCKS5 means one `ProxyConfig` shape covers
both `dial_quic` (UDP ASSOCIATE) and `dial_tcp_tls` (CONNECT); adding
HTTP CONNECT would add an enum variant that only one of the three dials
could use, for no privacy gain (SOCKS5 CONNECT does what HTTP CONNECT
does). The old code supported both, but the greenfield rewrite has no
backward-compatibility obligation, and the simplicity of a single proxy
protocol is worth more than the marginal compatibility benefit of HTTP
CONNECT. SOCKS5 is the sole proxy protocol.

## Decision

### 1. The proxy config is a first-class dial parameter on `AlknetClient`

`AlknetClient` gains an optional `Socks5ProxyConfig`. Following the
no-env-vars invariant (ADR-014), the config comes from `Capabilities` /
the assembly layer, never from `ALL_PROXY` / `HTTPS_PROXY` env vars.
(The old `alknet-main` client had a `proxy: Option<String>` field on
`ConnectOptions`; this ADR makes the concept real, typed, and uniformly
honored across the dials, where the old code's TCP/TLS transports
silently ignored it.)

```rust
pub struct Socks5ProxyConfig {
    /// The proxy's TCP address (where the SOCKS5 control connection
    /// connects). For UDP ASSOCIATE (the QUIC dial), the proxy replies
    /// with a UDP relay address that may differ; the dial uses that.
    pub addr: SocketAddr,
    /// Optional username/password auth (RFC 1929). None = no-auth.
    pub credentials: Option<Socks5Credentials>,
}

pub struct Socks5Credentials {
    pub username: String,
    pub password: String,
}
```

The config's crate location is an implementation detail (it can live in
`alknet-client`, `alknet-core`, or `alknet-tls` alongside the other
client config types), same as the `CallCredentials` location is an
implementation detail per ADR-089 §5. The shape is decided; the crate
home is two-way-door.

### 2. `AlknetClient` holds the proxy; each dial applies it

`AlknetClient` holds an `Option<Socks5ProxyConfig>` set via a builder
method, mirroring the `with_quinn` / `with_iroh` / `with_tcp_tls`
builder pattern:

```rust
impl AlknetClient {
    /// Set the SOCKS5 proxy for all subsequent dials. When set, every
    /// dial routes its transport through this proxy: UDP ASSOCIATE for
    /// `dial_quic`, CONNECT for `dial_tcp_tls`. `dial_iroh` is the
    /// exception — see OQ-67.
    pub fn with_socks5_proxy(mut self, proxy: Socks5ProxyConfig) -> Self;
}
```

The proxy is set once on the client (all dials use it), not per-dial.
Rationale: the proxy is a client-level privacy posture, not a
per-connection choice — a client that wants to hide its IP wants to hide
it on every dial. The per-dial alternative (each `dial_*` takes an
`Option<&Socks5ProxyConfig>`) is more flexible but rarely what a caller
wants, and it complicates the QUIC path where the proxy must be built
into the `quinn::Endpoint` (transport handle) rather than applied per
`connect`. A future per-dial override is a two-way-door addition; the
client-level default is the one-way-door surface.

The builder-method placement mirrors `AlknetEndpoint`'s builder
(ADR-083): the assembly layer builds the transport handles and hands
them to the client; the proxy is another pre-built handle. The symmetry
holds — `AlknetEndpoint` takes pre-built server transports;
`AlknetClient` takes pre-built client transports + a proxy.

### 3. `dial_quic` routes QUIC through SOCKS5 UDP ASSOCIATE

When a proxy is configured, `dial_quic` does not call
`quinn::Endpoint::client`. It performs the SOCKS5 UDP ASSOCIATE
handshake (TCP control connection to the proxy, optional auth, receive
the UDP relay address), wraps the result in a `Socks5UdpSocket` (the
`AsyncUdpSocket` impl from the PoC), and builds the `quinn::Endpoint`
via `new_with_abstract_socket` with that socket. The TLS config
(`TlsClientConfig`, ADR-087), the ALPN negotiation, the `Connection`
construction (`Connection::from_quinn_with_alpn`, ADR-065), and the
credential handshake are all unchanged — the proxy is below all of them.

The `Socks5UdpSocket` implementation (~250 lines, validated by the PoC)
lives in `alknet-client` behind a `socks5` feature flag. It is the
establishment concern (how the client reaches the hub), not a protocol
concern (what runs on the `Connection`). The `Connection`/dispatch/
credentials stay proxy-unaware.

Without a proxy, `dial_quic` is unchanged from ADR-089 —
`quinn::Endpoint::client` with a raw kernel UDP socket. The proxy is a
strict addition; the no-proxy path is the zero-cost default.

```rust
impl AlknetClient {
    #[cfg(feature = "quinn")]
    pub async fn dial_quic(
        &self,
        addr: SocketAddr,
        server_name: &str,
        alpn: &[u8],
        credentials: &CallCredentials,
    ) -> Result<Connection, ClientDialError> {
        let tls_config = TlsClientConfig::new(/* ... */)?;
        let client_config = quinn::ClientConfig::new(tls_config.for_quinn());
        let endpoint = match &self.socks5 {
            Some(proxy) => build_proxied_quinn_endpoint(proxy).await?,
            None => quinn::Endpoint::client("0.0.0.0:0".parse().unwrap())?,
        };
        let conn = endpoint.connect_with(client_config, addr, server_name)?.await?;
        Ok(Connection::from_quinn_with_alpn(conn, alpn.to_vec()))
    }
}

#[cfg(feature = "socks5")]
async fn build_proxied_quinn_endpoint(
    proxy: &Socks5ProxyConfig,
) -> Result<quinn::Endpoint, ClientDialError> {
    // 1. TCP control connection to proxy.addr.
    // 2. SOCKS5 UDP ASSOCIATE handshake (with optional auth).
    // 3. Socks5UdpSocket::from_datagram on the result.
    // 4. quinn::Endpoint::new_with_abstract_socket with the Socks5UdpSocket.
}
```

### 4. `dial_tcp_tls` routes TCP through SOCKS5 CONNECT

When a proxy is configured, `dial_tcp_tls` connects a `TcpStream` to the
proxy's address, performs the SOCKS5 CONNECT handshake (RFC 1928 §3) to
the target `addr`, then wraps the resulting stream in `TlsConnector` as
before. This is the straightforward TCP proxy case — SOCKS5 CONNECT is
TCP-native. The `TlsClientConfig`, the SNI, the ALPN, and the
`Connection::from_bidi` (ADR-065) are all unchanged.

Without a proxy, `dial_tcp_tls` is unchanged — `TcpStream::connect(addr)`
then `TlsConnector::connect`. Same zero-cost default as `dial_quic`.

### 5. `dial_iroh` is the exception — deferred (OQ-67)

iroh's `proxy_url` (set at `iroh::Endpoint` construction) proxies iroh's
HTTP(S) traffic — relay connections, DNS-over-HTTPS, pkarr publishing —
through the proxy. This hides the client's IP from iroh's *relay*, which
is a distinct privacy goal from hiding it from the *peer*. iroh's relay
already hides the client's IP from the peer (the peer sees the relay's
IP, by design), so `proxy_url` covers both exposure surfaces for the
relay-mediated path without touching iroh's QUIC socket.

The open case is iroh's *direct* (hole-punched) connection: if iroh
establishes a direct QUIC connection to the peer, the peer sees the
client's real IP. Wrapping iroh's QUIC in SOCKS5 UDP ASSOCIATE is a
*different* integration than the quinn POC — iroh has its own socket
stack, not `quinn::AsyncUdpSocket` — and whether to force relay-only
when a proxy is configured vs. wrap iroh's QUIC is an open question. See
[OQ-67](../questions/067-iroh-proxy-support.md).

For now, `dial_iroh` does not consume `Socks5ProxyConfig`. The iroh
endpoint is built by the assembly layer (ADR-089 §3 — iroh shares the
key, not the config); if the assembly layer sets iroh's `proxy_url`, the
relay-exposure surface is covered. The peer-exposure surface for iroh
direct connections is deferred to OQ-67. This does not block the first
hub deployment — the hub's outbound worker dials (the hub-as-client case,
ADR-087 §5) use QUIC or TCP+TLS, both of which honor the proxy.

### 6. Fallback policy is a caller concern, not a dial decision

When a proxy is configured and the proxy rejects UDP ASSOCIATE (the QUIC
case) or CONNECT (the TCP case), `dial_quic` / `dial_tcp_tls` return an
error. The dial does not silently fall back to a direct connection —
that would defeat the privacy posture the caller configured the proxy to
enforce. A caller that wants "try proxy, fall back to direct" composes
it: catch the error, dial on an `AlknetClient` *without* the proxy set.
Because the proxy is set once on the client (§2), this requires two
`AlknetClient` instances — one built with `with_socks5_proxy`, one
without — and the caller dials the proxy-less one on fallback. A future
per-dial proxy override (§2, two-way-door) would remove the need for a
second client, but the two-instance pattern is the composition the
current API supports. The fallback policy is the caller's, not the
dial's, because the dial cannot know whether the caller's privacy
requirement permits a direct fallback. This mirrors ADR-089's
"no transport fallback" stance — the dial is one-shot; the fallback
policy is a caller concern.

The `ClientDialError` (ADR-089) gains one variant:

```rust
pub enum ClientDialError {
    // ... existing variants ...

    /// SOCKS5 proxy failure — handshake rejected, UDP ASSOCIATE
    /// unsupported, auth failed, or the proxy closed the control
    /// connection. The dial did not reach the remote; the caller
    /// decides whether to fall back to a direct dial or surface the
    /// error.
    #[error("SOCKS5 proxy: {0}")]
    Proxy(String),
}
```

`Proxy(String)` takes `String` (not the proxy crate's error type) for
the same reason `Connect(String)` and `Handshake(String)` do per ADR-089
— the proxy error source type (`fast-socks5` or a vendored impl) is an
implementation detail; the category is in the variant. The string
content is an implementation detail.

### 7. Feature gates

```toml
[features]
default = []
quinn = ["dep:quinn", "alknet-tls/quinn"]
tcp = ["dep:tokio-rustls", "alknet-tls/tcp"]
iroh = ["dep:iroh"]
socks5 = ["dep:fast-socks5"]   # enables the proxied dial paths
```

`socks5` is independent of the transport features — it enables the
proxy code path that `dial_quic` (UDP ASSOCIATE) and `dial_tcp_tls`
(CONNECT) use when a proxy is configured. Enabling `socks5` without
`quinn` or `tcp` is a no-op (no dial uses it). Enabling `quinn` + `socks5`
enables proxied QUIC; `tcp` + `socks5` enables proxied TCP+TLS. The
`fast-socks5` dep is behind `socks5`, so deployments that don't use a
proxy don't pay the dep.

### 8. The `socks5` crate dependency

The PoC uses `fast-socks5` (v1.0.0, maintained by anyip.io, a residential
proxy provider) for the SOCKS5 client handshake and the test server.
The client-side surface used is small (`Socks5Datagram::bind` /
`bind_with_password`, `new_udp_header`, the header parsing). Whether to
keep `fast-socks5` as a dep or vendor the ~100 lines of SOCKS5 handshake
+ header framing is a two-way-door implementation detail. This ADR
records that `fast-socks5` is the starting choice (it's the validated
choice from the PoC); vendoring is a future option if the dep becomes a
concern. The `AsyncUdpSocket` impl (`Socks5UdpSocket`, ~250 lines) is
alknet's own code regardless — it's the integration glue, not the
SOCKS5 protocol implementation.

## What this does NOT change

- **`AlknetEndpoint` (ADR-083)** — the server side is unchanged. The
  server accepts connections arriving from a proxy's IP, which is just
  normal QUIC/TCP. No proxy-specific code on the server.
- **`TlsClientConfig` (ADR-087)** — the client-side TLS config is
  unchanged. The proxy is below the TLS layer; the TLS handshake happens
  over the proxied transport exactly as it happens over a direct one.
- **`CallClient::spawn_dispatch` / `ChannelClient::from_connection`
  (ADR-017, ADR-080)** — the take-over APIs are unchanged. They consume
  the `Connection` the dial produces; they don't know it came through a
  proxy.
- **The `Connection` type (ADR-065, ADR-070)** — unchanged. A proxied
  QUIC connection still yields a `Connection::from_quinn_with_alpn`; a
  proxied TCP+TLS connection still yields a `Connection::from_bidi`. The
  `Connection` is proxy-unaware.
- **The `CallCredentials` / `RemoteIdentity` (ADR-089)** — unchanged.
  The credential bundle the dial takes is unaffected by the proxy.
- **The hub's `supervise_worker` (hub README §"Dial")** — the closure
  seam is unchanged. A hub that wants its outbound worker dials to go
  through a proxy builds its `AlknetClient` with `with_socks5_proxy`;
  the `supervise_worker` closure calls `dial_quic` / `dial_tcp_tls` and
  the proxy is applied transparently. The hub does not need to know the
  proxy exists.
- **`alknet-socks5` (the channels data-channel handler, ADR-085)** —
  unchanged and out of scope. The two SOCKS5 concepts compose at the
  SOCKS5 protocol level, not the alknet type level (see §"Two distinct
  SOCKS5 uses" above).

## Consequences

**Positive:**

- **The privacy gap is closed for QUIC and TCP+TLS dials.** A native
  client that wants to hide its real IP from the hub configures a
  SOCKS5 proxy; `dial_quic` and `dial_tcp_tls` route through it. The
  hub sees the proxy's IP. This is the capability the old
  `ConnectOptions.proxy` field aspired to but never delivered for
  TCP/TLS.
- **The QUIC proxy is real, not theoretical.** The PoC (`/workspace/quinn-proxy-poc`,
  5/5 runs clean) and BotBrowser's production use validate the
  `AsyncUdpSocket` + UDP ASSOCIATE approach. The ECN loss is an accepted,
  documented cost of the privacy choice, not a hidden limitation.
- **The proxy is uniform across the rustls dials.** One
  `Socks5ProxyConfig` covers both `dial_quic` (UDP ASSOCIATE) and
  `dial_tcp_tls` (CONNECT). SOCKS5's TCP+UDP coverage is the reason a
  single proxy protocol suffices — no HTTP CONNECT variant needed.
- **The proxy is invisible above the dial.** `Connection`, dispatch,
  credentials, TLS config, the hub's `supervise_worker` closure — none
  know a proxy is in the path. The proxy is purely an establishment
  concern, localized to `alknet-client`.
- **The no-proxy path is the zero-cost default.** `dial_quic` and
  `dial_tcp_tls` without a configured proxy are byte-identical to
  ADR-089. The `socks5` feature and the `fast-socks5` dep are opt-in;
  deployments that don't use a proxy pay nothing.
- **The two SOCKS5 concepts compose.** A client using the hub's
  `alknet/socks5` channels service for privacy points its dial
  `Socks5ProxyConfig` at the local tunnel end. No special integration;
  the composition is at the SOCKS5 protocol level.

**Negative:**

- **iroh direct connections are not proxied by this ADR.** `dial_iroh`
  does not consume `Socks5ProxyConfig`; iroh's `proxy_url` (set at the
  assembly layer) covers the relay-exposure surface, but a direct
  iroh connection exposes the client's real IP to the peer. This is
  OQ-67. It does not block the first hub deployment (hub outbound
  dials use QUIC/TCP+TLS), but it is a gap for iroh-direct privacy.
- **The `socks5` feature and `fast-socks5` dep are new.** A new
  optional feature and a new optional dep. The cost is contained (off
  by default, only pulled when a deployment uses a proxy), but it is
  a new entry in the feature matrix.
- **ECN loss on proxied QUIC.** The proxied QUIC path loses Explicit
  Congestion Notification. This is a performance cost on congested
  links, not a correctness issue. It is an inherent property of
  SOCKS5 UDP ASSOCIATE (the header has no ECN bits), not an alknet
  choice. Documented and accepted.
- **Proxy must support UDP ASSOCIATE for the QUIC dial.** A
  deployment requirement. `ssh -D` does not work; a UDP-capable SOCKS5
  daemon (dante, fast-socks5-based, etc.) is needed. The dial surfaces
  a clear error when the proxy lacks UDP support; the operator chooses
  a compatible proxy.

## Door type

**One-way (proxy as a dial capability + the `Socks5ProxyConfig`
shape).** The proxy being a first-class dial parameter on
`AlknetClient` is structural — every outbound-dialing role (hub, worker,
hub-worker) that wants privacy depends on it. The `Socks5ProxyConfig`
shape and the `with_socks5_proxy` builder method are one-way — changing
them after consumers exist is a rewrite. The `Proxy` variant on
`ClientDialError` is one-way (it's a public error variant). The
`socks5` feature name and the `fast-socks5` dep choice are two-way
(the feature can be renamed pre-1.0; the dep can be vendored later).
The `Socks5UdpSocket` internal implementation is two-way. The iroh
proxy question (OQ-67) is two-way until decided. The fallback policy
(caller's concern, not the dial's) is one-way — once the dial refuses
to silently fall back, callers rely on that for their privacy
posture.

## References

- **PoC:** `/workspace/quinn-proxy-poc` — run `cargo run -r`. Load-bearing
  file: `src/socks5_udp_socket.rs` (~250 lines). 5/5 runs clean.
- **Research findings:**
  [`docs/research/quinn-quic-proxy/findings.md`](../../research/quinn-quic-proxy/findings.md)
  — the full write-up: the `AsyncUdpSocket` extension point, the
  SOCKS5 UDP ASSOCIATE mechanism, the PoC topology, the limitations
  (ECN, UDP ASSOCIATE requirement), the alternatives rejected (fork
  quinn, QUIC-in-TCP, HTTP CONNECT, userspace QUIC-over-TCP, wait for
  upstream), and the integration sketch.
- [ADR-089](089-alknetclient-native-dial-seam.md) — `AlknetClient`, the
  dial seam this ADR adds the proxy capability to. The three dials
  (`dial_quic` / `dial_tcp_tls` / `dial_iroh`) are the surfaces the
  proxy applies to.
- [ADR-087](087-tlsclientconfig-not-blocked-on-dial.md) —
  `TlsClientConfig`, unchanged by the proxy (the TLS handshake happens
  over the proxied transport).
- [ADR-083](083-endpoint-as-accept-loop-runner.md) — `AlknetEndpoint`,
  unchanged (the server accepts connections from a proxy's IP as
  normal).
- [ADR-065](065-connection-from-stream-generic-single-stream.md) —
  `Connection::from_quinn_with_alpn` / `from_bidi`, unchanged (the
  `Connection` is proxy-unaware).
- [ADR-014](014-secret-material-flow-and-capability-injection.md) — the
  no-env-vars invariant; the proxy config comes from
  `Capabilities` / the assembly layer, not env vars.
- [ADR-085](085-workspace-scope-core-vs-consumer-repos.md) — the scope
  table naming `alknet-socks5` (the channels data-channel handler,
  distinct from this ADR's client-dial proxy).
- RFC 1928 (SOCKS5) §3 (CONNECT), §6/§7 (UDP ASSOCIATE + UDP request
  header).
- RFC 1929 (SOCKS5 username/password auth).
- BotBrowser UDP-over-SOCKS5
  (`deepwiki.com/botswin/BotBrowser/6.4-udp-over-socks5`) — production
  implementation of the identical pattern in Chromium's network stack.
- OQ-67 (raised by this ADR) — iroh proxy support (direct-connection
  peer exposure).