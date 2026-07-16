# Quinn QUIC over SOCKS5 Proxy — UDP ASSOCIATE PoC

**Status:** Findings complete. Approach validated by an end-to-end PoC (`/workspace/quinn-proxy-poc`) in which a quinn client opens a QUIC connection to a quinn server with all UDP traffic tunneled through a SOCKS5 proxy. The PoC reliably completes a QUIC handshake and exchanges stream data through the proxy (5/5 runs clean). No quinn fork required — the whole integration uses one public trait (`quinn::AsyncUdpSocket`) and one public constructor (`quinn::Endpoint::new_with_abstract_socket`). The integration crate for alknet-call is small (~250 lines, one new file) and lands cleanly behind the existing `quinn` feature flag.
**Date:** 2026-07-16
**Scope:** Resolves the "quinn has no proxy support" blocker for QUIC client connections in `alknet-call`. TCP proxying is already straightforward and is explicitly out of scope here. This document covers only QUIC/quinn proxying via SOCKS5 with UDP ASSOCIATE (RFC 1928 §6).

---

## TL;DR

1. **Quinn does not natively support proxies, but it was designed for exactly this.** Quinn routes every network byte through a single trait, `quinn::AsyncUdpSocket` (`quinn-0.11.11/src/runtime.rs:42`), and exposes `Endpoint::new_with_abstract_socket` (`endpoint.rs:133`) to accept any impl. The maintainers added it for runtime independence, but it is the precise extension point a proxy integration needs. There is no need to fork quinn, patch internals, or wait for an upstream feature.

2. **The integration is a SOCKS5 UDP ASSOCIATE tunnel wrapped as an `AsyncUdpSocket`.** SOCKS5's `UDP ASSOCIATE` command (RFC 1928 §6) lets a client send UDP datagrams through a proxy: the client opens a TCP control connection, requests `UDP ASSOCIATE`, gets back the proxy's UDP relay address, then sends each datagram to the relay wrapped in a small SOCKS5 header (RSV/FRAG/ATYP/DST.ADDR/DST.PORT + payload). The proxy forwards to the real destination and relays replies back. This is exactly what quinn needs — it sees a UDP socket, the proxy sees SOCKS5-framed UDP, the QUIC server sees the proxy's IP.

3. **A working PoC proves it end-to-end.** `/workspace/quinn-proxy-poc` runs: a `fast-socks5` SOCKS5 server with UDP enabled, a quinn echo server, and a quinn client whose `Endpoint` is built on a `Socks5UdpSocket`. The client completes the QUIC handshake, opens a bidi stream, sends 34 bytes, and gets them echoed back — all through the proxy. 5/5 runs clean. The load-bearing file is `src/socks5_udp_socket.rs` (~250 lines).

4. **The approach is proven in production elsewhere.** BotBrowser implements the identical pattern (SOCKS5 UDP ASSOCIATE for QUIC/HTTP/3 and STUN) inside Chromium's network stack (`deepwiki.com/botswin/BotBrowser/6.4-udp-over-socks5`). The mechanism is the same; only the integration layer differs (Chromium's socket abstraction vs quinn's `AsyncUdpSocket`).

5. **Two real limitations, both acceptable for alknet's use:**
   - **ECN is lost.** The SOCKS5 UDP header does not carry ECN bits, so Explicit Congestion Notification cannot survive the proxy hop. `Socks5UdpSocket` reports `ecn: None` on receive and `may_fragment() == true`, which makes quinn disable path MTU discovery and fall back to the non-ECN path. This is a performance/efficiency cost, not a correctness one — QUIC degrades gracefully without ECN.
   - **Proxy must support UDP ASSOCIATE.** Not every SOCKS5 proxy does. This is a deployment requirement, not a code problem. The PoC's `fast-socks5` server proves the capability is widely available in off-the-shelf proxies. A production integration should probe the handshake result and fall back to a direct connection (or a TCP-based transport per `../transport-generalization/findings.md`) if the proxy lacks UDP support.

6. **Integration into alknet-call is small and additive.** The current `connect()` (`crates/alknet-call/src/client/call_client.rs:141`) builds a `quinn::Endpoint::client(bind_addr)`. The change is: when a proxy is configured, build the endpoint via `new_with_abstract_socket` with a `Socks5UdpSocket` instead. ~30 lines changed in `connect()`, plus one new `socks5_udp_socket.rs` module (the PoC's file, adapted). No `Connection`/`SendStream`/`RecvStream` changes, no core changes, no new trait. Lands behind the existing `quinn` feature flag with an optional `socks5` feature for the `fast-socks5` dep.

---

## 1. The problem

### 1.1 Quinn's API surface hides the socket

`alknet-call`'s QUIC client (`crates/alknet-call/src/client/call_client.rs:141-168`) dials with the most ergonomic quinn constructor:

```rust
let endpoint = quinn::Endpoint::client(bind_addr)?;
let connection = endpoint.connect_with(client_config, addr, "alknet")?.await?;
```

`Endpoint::client` (`quinn-0.11.11/src/endpoint.rs:75`) binds a kernel UDP socket and wraps it in quinn's tokio runtime adapter. There is no parameter for a proxy, and `connect_with` only takes a `SocketAddr` + server name — the destination is fixed at the QUIC layer, with no hook to redirect the underlying UDP packets.

Reading the docs alone, it looks like quinn is welded to raw UDP sockets and proxy support would require a fork.

### 1.2 Why "QUIC is just UDP" isn't quite enough

QUIC runs over UDP, so in principle any SOCKS5 proxy that supports `UDP ASSOCIATE` can tunnel it. But the client application has to (a) establish the SOCKS5 UDP association over a TCP control connection, (b) wrap every outgoing datagram in a SOCKS5 UDP request header, and (c) strip that header from every incoming datagram. If the QUIC library owns the UDP socket (as quinn's `Endpoint::client` does), the application never sees the raw datagrams and cannot do this wrapping. The wrapping has to happen *inside* the socket abstraction the QUIC library polls.

---

## 2. The extension point

### 2.1 `AsyncUdpSocket`

Quinn abstracts the UDP socket behind `quinn::AsyncUdpSocket` (`quinn-0.11.11/src/runtime.rs:42`):

```rust
pub trait AsyncUdpSocket: Send + Sync + Debug + 'static {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>>;
    fn try_send(&self, transmit: &Transmit) -> io::Result<()>;
    fn poll_recv(&self, cx: &mut Context, bufs: &mut [IoSliceMut], meta: &mut [RecvMeta])
        -> Poll<io::Result<usize>>;
    fn local_addr(&self) -> io::Result<SocketAddr>;
    fn max_transmit_segments(&self) -> usize { 1 }
    fn max_receive_segments(&self) -> usize { 1 }
    fn may_fragment(&self) -> bool { true }
}
```

Every byte quinn sends goes through `try_send` as a `udp::Transmit` (`{destination, ecn, contents, segment_size, src_ip}`). Every byte quinn receives comes through `poll_recv` as a `udp::RecvMeta` (`{addr, len, stride, ecn, dst_ip}`). Implement this trait and you control the entire network path.

### 2.2 `Endpoint::new_with_abstract_socket`

The constructor that accepts a custom socket (`endpoint.rs:133`):

```rust
pub fn new_with_abstract_socket(
    config: EndpointConfig,
    server_config: Option<ServerConfig>,
    socket: Arc<dyn AsyncUdpSocket>,
    runtime: Arc<dyn Runtime>,
) -> io::Result<Self>
```

The doc comment confirms the intended use: *"Useful when `socket` has additional state (e.g. sidechannels) attached for which shared ownership is needed."* A SOCKS5 tunnel with its TCP control connection is exactly "additional state attached to a socket."

There is also `rebind_abstract` (`endpoint.rs:253`) for swapping the socket live, but for a client-only proxy integration `new_with_abstract_socket` is sufficient.

### 2.3 The tokio reference impl

Quinn's own tokio adapter (`runtime/tokio.rs:57-102`) is the template. It wraps a `tokio::net::UdpSocket` + `quinn_udp::UdpSocketState` and implements `AsyncUdpSocket` by delegating `try_send`/`poll_recv` to `quinn_udp`'s GSO/GRO-aware `send`/`recv`. The proxy impl has the same shape but inserts SOCKS5 framing between the quinn datagram and the kernel send.

---

## 3. SOCKS5 UDP ASSOCIATE (RFC 1928 §6/§7)

### 3.1 The handshake

1. Client opens a TCP connection to the proxy.
2. Version/method negotiation: client sends `05 NMETHODS METHODS...`, server picks one (none, or username/password).
3. Optional username/password auth (RFC 1929).
4. Client sends `UDP ASSOCIATE` request: `05 03 00 ATYP DST.ADDR DST.PORT`. For a generic UDP tunnel the DST is `0.0.0.0:0` (the client doesn't know in advance which remote it will talk to).
5. Server replies `05 00 00 ATYP BND.ADDR BND.PORT` — the proxy's UDP relay address.
6. Client binds a local UDP socket and `connect()`s it to the relay address. The TCP control connection stays open for the lifetime of the association; dropping it tears down the relay.

### 3.2 The datagram wrapper

Every UDP datagram the client sends to the relay carries a SOCKS5 header (RFC 1928 §7):

```
+----+------+------+----------+----------+----------+
|RSV | FRAG | ATYP | DST.ADDR | DST.PORT |   DATA   |
+----+------+------+----------+----------+----------+
| 2  |  1   |  1   | Variable |    2     | Variable |
+----+------+------+----------+----------+----------+
```

- `RSV` = `00 00`, `FRAG` = `0` (fragmentation unsupported in our use).
- `ATYP` = `01` (IPv4) / `03` (domain) / `04` (IPv6).
- `DST.ADDR` + `DST.PORT` = the real QUIC server address.
- `DATA` = the quinn datagram payload.

The proxy strips the header, forwards `DATA` to `DST`, and when the server replies, wraps the reply in the same header shape (with `DST` = the server's address) and sends it back to the client's UDP socket. The client strips the header and hands `DATA` + the source address to quinn.

For quinn, `DST` is always the `Transmit.destination` (a `SocketAddr`, so IPv4/IPv6 — never a domain). The wrapper is ~6–22 bytes of header per datagram.

---

## 4. The PoC

Location: `/workspace/quinn-proxy-poc`

### 4.1 Topology

```
quinn client  ──(SOCKS5 UDP ASSOCIATE)──►  SOCKS5 proxy  ──(raw UDP)──►  quinn server
(Socks5UdpSocket)                          (fast-socks5 server)         (raw UdpSocket)
```

- **Quinn echo server**: `main.rs:spawn_quinn_echo_server` — binds a raw UDP socket, accepts connections, echoes each bidi stream.
- **SOCKS5 proxy**: `main.rs:spawn_socks5_proxy` — a `fast-socks5` server with `run_udp_proxy` enabled on `UDP ASSOCIATE` commands.
- **Quinn client**: `main.rs:connect_quinn_via_socks5` — performs the SOCKS5 handshake, wraps the result in `Socks5UdpSocket`, builds the endpoint via `new_with_abstract_socket`, and connects to the server.

### 4.2 The load-bearing file: `src/socks5_udp_socket.rs`

`Socks5UdpSocket<S>` implements `AsyncUdpSocket` over a `fast_socks5::client::Socks5Datagram<S>`. The three interesting methods:

- **`try_send`**: builds the SOCKS5 frame with `fast_socks5::new_udp_header(transmit.destination)` (sync), appends `transmit.contents`, and does a non-blocking `try_send` on the inner (proxy-connected) tokio UDP socket via `try_io(Interest::WRITABLE, ...)`. On `WouldBlock` it returns `WouldBlock`, which is the signal quinn uses to call `poll_writable` on the poller.

- **`poll_recv`**: reads one datagram from the relay into a scratch buffer, parses the SOCKS5 header *synchronously* (a small `parse_udp_header` function — `fast_socks5::parse_udp_request` is async and can't be called from a sync poll fn), copies the payload into quinn's buffer, and fills `RecvMeta` with the source address from the header (the real QUIC server's address, not the proxy's). Returns `1` (one datagram; no GRO).

- **`create_io_poller`**: returns a `WritablePoller` that polls the underlying socket's `poll_send_ready` directly. No stored future, no borrow-lifetime gymnastics. The poller holds a `dup()`'d copy of the tokio UDP fd (tokio's `UdpSocket` is `!Clone`, so we duplicate the kernel fd and wrap it in a fresh tokio handle for independent readiness tracking).

### 4.3 Result

```
INFO quinn_proxy_poc: quinn echo server listening on 127.0.0.1:58219
INFO quinn_proxy_poc: SOCKS5 proxy listening on 127.0.0.1:40909 (UDP enabled)
INFO quinn_proxy_poc: connecting quinn client through proxy to 127.0.0.1:58219
INFO quinn_proxy_poc: socks5 udp associate established; local udp 127.0.0.1:54664 -> proxy relay Ok(Ip(127.0.0.1:59999))
INFO quinn_proxy_poc: quinn connection established through proxy
INFO quinn_proxy_poc: SUCCESS: quinn client sent 34 bytes through SOCKS5 UDP relay and got them echoed back
```

5/5 consecutive runs clean. `cargo clippy` clean.

Run it yourself:

```bash
cd /workspace/quinn-proxy-poc
cargo run -r
```

---

## 5. Limitations and trade-offs

### 5.1 ECN is lost across the proxy

The SOCKS5 UDP header carries RSV/FRAG/ATYP/ADDR/PORT — no ECN bits. Quinn uses ECN for congestion control (RFC 9000 §13.4, §14.x) and path validation. Without it, quinn falls back to loss-based congestion detection. `Socks5UdpSocket` handles this correctly by:

- returning `ecn: None` in `RecvMeta` (quinn treats the path as non-ECN-capable),
- returning `may_fragment() == true` (disables path MTU discovery — the proxied path can't relay ICMP frag-needed back to quinn).

This is a performance cost on congested links, not a correctness issue. QUIC is fully functional without ECN. For alknet's call-protocol use (low-bandwidth, latency-sensitive but not throughput-critical), the cost is negligible.

### 5.2 Proxy must support UDP ASSOCIATE

This is a deployment requirement. Many SOCKS5 proxies (e.g. Shadowsocks-rust, fast-socks5, dante with UDP enabled, anyip.io residential proxies) support it; some (notably basic corporate proxies, some older `ssh -D` setups) do not. A production integration should:

1. Attempt the SOCKS5 handshake.
2. If the proxy returns `CommandNotSupported` for `UDP ASSOCIATE`, fall back to a direct QUIC connection (if the network allows) or to a TCP-based transport (per `../transport-generalization/findings.md`'s `Connection::from_stream`).
3. Surface a clear error to the operator if neither works.

`ssh -D` deserves a specific callout: OpenSSH's dynamic SOCKS5 port forwarding does **not** implement `UDP ASSOCIATE`. Alknet nodes that currently expose a SOCKS5 proxy via `ssh -D` will not work with this integration. Use a dedicated SOCKS5 daemon (dante, fast-socks5-based, etc.) or a UDP-capable proxy service instead.

### 5.3 One extra TCP control connection

The SOCKS5 UDP association requires a TCP connection to stay open for the lifetime of the UDP relay. This is one extra FD and ~one kernel TCP connection per quinn endpoint. Negligible.

### 5.4 Per-datagram framing overhead

6 bytes (IPv4) or 22 bytes (IPv6) of SOCKS5 header per UDP datagram. QUIC datagrams are typically 1200–1472 bytes on the wire, so the overhead is 0.4–1.8%. Negligible.

### 5.5 No GSO/GRO

Quinn's kernel adapter uses GSO (Generic Segmentation Offload) to send many datagrams in one syscall and GRO to receive them batched. Through a SOCKS5 relay, each datagram is independently framed, so `max_transmit_segments` and `max_receive_segments` return `1`. This is a syscall-rate cost under high packet rates, not a correctness issue. For alknet's call protocol (a handful of streams, modest packet rate), it's invisible.

---

## 6. Integration into alknet-call

### 6.1 The change is small

The current client (`crates/alknet-call/src/client/call_client.rs:141-168`):

```rust
pub async fn connect(&self, addr: SocketAddr, credentials: CallCredentials)
    -> Result<CallConnection, ClientError>
{
    let alpn = b"alknet/call".to_vec();
    let client_config = build_quinn_client_config(&credentials, &alpn)?;
    let bind_addr: SocketAddr = "0.0.0.0:0".parse().unwrap();
    let endpoint = quinn::Endpoint::client(bind_addr)?;          // <-- this line
    let connection = endpoint.connect_with(client_config, addr, "alknet")?.await?;
    let connection = Connection::from_quinn_with_alpn(connection, alpn);
    Ok(self.spawn_dispatch(connection))
}
```

The proxy-aware version (sketch):

```rust
pub async fn connect(&self, addr: SocketAddr, credentials: CallCredentials)
    -> Result<CallConnection, ClientError>
{
    let alpn = b"alknet/call".to_vec();
    let client_config = build_quinn_client_config(&credentials, &alpn)?;
    let endpoint = match &self.proxy {
        Some(proxy) => build_proxied_quinn_endpoint(proxy)?,    // new path
        None => quinn::Endpoint::client("0.0.0.0:0".parse().unwrap())?,
    };
    let connection = endpoint.connect_with(client_config, addr, "alknet")?.await?;
    let connection = Connection::from_quinn_with_alpn(connection, alpn);
    Ok(self.spawn_dispatch(connection))
}

#[cfg(feature = "socks5")]
fn build_proxied_quinn_endpoint(proxy: &Socks5ProxyConfig)
    -> Result<quinn::Endpoint, ClientError>
{
    // 1. TCP control connection to the proxy (blocking dial off the runtime
    //    loop, or via tokio::spawn + oneshot if connect() stays async — it
    //    already is async, so a direct await is fine).
    // 2. Socks5Datagram::bind / bind_with_password.
    // 3. Socks5UdpSocket::from_datagram.
    // 4. quinn::Endpoint::new_with_abstract_socket.
}
```

The `Connection`/`SendStream`/`RecvStream` types, the dispatch loop, the credential handshake — all unchanged. The proxy is invisible above the endpoint layer.

### 6.2 Where the proxy config comes from

`CallClient` currently has no proxy field. Following the no-env-vars invariant (ADR-014), the proxy config comes from `Capabilities` (or the equivalent config source that already feeds `CallCredentials`). A new `Socks5ProxyConfig { addr, username: Option<String>, password: Option<String> }` lives in the same place `CallCredentials` does. The `CallClient` holds an `Option<Socks5ProxyConfig>`; `connect()` branches on it.

### 6.3 Feature flags

- `quinn` (existing, default): QUIC via raw kernel UDP socket. Unchanged.
- `socks5` (new, optional, implies `quinn` + `dep:fast-socks5`): enables the proxied endpoint path. Off by default to avoid the `fast-socks5` dep for deployments that don't use a proxy. The `Socks5UdpSocket` module is `#[cfg(feature = "socks5")]`.

### 6.4 What does *not* change

- `Connection`, `SendStream`, `RecvStream` (the transport-generalization work in `../transport-generalization/findings.md` is orthogonal and composes: a proxied QUIC connection still yields a `Connection::from_quinn_with_alpn`).
- The dispatch loop / `Dispatcher` / `CallConnection`.
- The credential handshake and TLS config (`build_quinn_client_config`).
- The server side. This is client-only. (Server-side proxying is a different problem — the server would need to accept connections arriving from a proxy's IP, which is just normal QUIC over UDP. No proxy-specific code on the server.)
- iroh. The iroh transport is unaffected.

---

## 7. Alternatives considered and rejected

### 7.1 Fork quinn to add a proxy parameter

Rejected. `AsyncUdpSocket` + `new_with_abstract_socket` already provide the hook. A fork would carry an ongoing maintenance burden (re-tracking upstream) for zero capability gain. The PoC proves the public API is sufficient.

### 7.2 Tunnel QUIC-in-TCP via SOCKS5 CONNECT

SOCKS5 `CONNECT` (the TCP command) is universally supported, so one could tunnel QUIC inside a TCP connection. But this throws away QUIC's primary advantages (no head-of-line blocking, UDP-native congestion control) and reintroduces TCP-over-TCP meltdown risk if the underlying transport is also TCP-based. It's the fallback of last resort, not the primary path. The `Connection::from_stream` work in `../transport-generalization/findings.md` covers the "just use TCP+TLS" case directly, which is cleaner than QUIC-in-TCP.

### 7.3 Use an HTTP CONNECT proxy

HTTP proxies don't support UDP at all. Same problem as 7.2 but worse (no standards for UDP-over-HTTP-CONNECT).

### 7.4 Use a userspace QUIC-over-TCP implementation

E.g. `quinn` over a custom `AsyncUdpSocket` that internally TCP-frames datagrams. This is essentially 7.2 with extra steps. Rejected for the same reason.

### 7.5 Wait for upstream quinn proxy support

No upstream work is happening on this — the maintainers' position is that `AsyncUdpSocket` is the integration point. There's nothing to wait for.

---

## 8. Open questions for the integration PR

1. **Proxy config source.** Confirm whether `Socks5ProxyConfig` lives in `Capabilities` (ADR-014) or a separate config struct. The no-env-vars invariant means it's not `ALL_PROXY`/`HTTPS_PROXY` env vars.
2. **Auth method surface.** The PoC uses no-auth. The integration should support username/password (RFC 1929). `fast-socks5` supports it directly via `bind_with_password`. Decide whether to expose private-key/mTLS auth to the proxy as a future feature or keep it username/password-only initially.
3. **Fallback policy.** When the proxy lacks UDP support, do we (a) error clearly, (b) fall back to direct QUIC, or (c) fall back to a TCP transport per `../transport-generalization/findings.md`? (a) is simplest; (b)/(c) need a policy decision.
4. **Server-side proxy.** Out of scope here, but worth confirming: does alknet ever need the *server* to be reachable only via a proxy? If so, that's a different integration (the server's `Endpoint` would need to accept connections arriving from a proxy's IP, which is just normal QUIC — but if the server is behind a proxy that only exposes TCP, QUIC can't reach it at all and the TCP transport from `../transport-generalization` is the answer).
5. **`fast-socks5` vs a hand-rolled SOCKS5 client.** The PoC uses `fast-socks5` for both the client handshake and the test server. For the alknet integration, `fast-socks5`'s client is small and well-maintained (by anyip.io, a residential proxy provider). Alternatively, the ~100 lines of SOCKS5 handshake + header framing could be vendored to avoid the dep. Recommend starting with `fast-socks5` behind a feature flag and revisiting if the dep becomes a concern.

---

## 9. References

- **Quinn source (cargo cache):**
  - `quinn-0.11.11/src/runtime.rs:42` — `AsyncUdpSocket` trait definition.
  - `quinn-0.11.11/src/endpoint.rs:133` — `new_with_abstract_socket`.
  - `quinn-0.11.11/src/endpoint.rs:253` — `rebind_abstract` (live socket swap).
  - `quinn-0.11.11/src/runtime/tokio.rs:57-102` — tokio reference impl, the template.
  - `quinn-udp-0.5.14/src/lib.rs:95-147` — `RecvMeta` / `Transmit` shapes.
- **RFC 1928** (SOCKS5) §6 (UDP ASSOCIATE), §7 (UDP request header).
- **RFC 1929** (SOCKS5 username/password auth).
- **`fast-socks5`** (`crates.io/crates/fast-socks5`, v1.0.0) — `client::Socks5Datagram` for the handshake + framed send/recv; `server::run_udp_proxy` for the test server. Maintained by anyip.io.
- **BotBrowser UDP-over-SOCKS5** (`deepwiki.com/botswin/BotBrowser/6.4-udp-over-socks5`) — production implementation of the identical pattern (SOCKS5 UDP ASSOCIATE for QUIC/HTTP/3 + STUN) inside Chromium's network stack. Confirms the approach scales to a real browser's traffic.
- **PoC:** `/workspace/quinn-proxy-poc` — run `cargo run -r`. Load-bearing file: `src/socks5_udp_socket.rs`.
- **Related alknet work:** `../transport-generalization/findings.md` — `Connection::from_stream` is orthogonal and composes with this. A proxied QUIC connection and a TCP+TLS connection both yield a `Connection`; the dispatch loop doesn't care which.