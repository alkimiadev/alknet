# iroh Proxy Support — Direct-Connection Peer-Exposure PoC

**Status:** Findings complete. Resolves OQ-67 (`docs/architecture/questions/067-iroh-proxy-support.md`). Option 2 (force relay-only) is validated end-to-end by a PoC (`/workspace/iroh-proxy-poc`): a relay-only, proxied iroh client connects to an iroh server over a local relay whose WebSocket is tunneled through an HTTP CONNECT proxy; the selected path is a relay path, no direct IP path is ever established, and a 45-byte echo exchange completes through the proxied relay (5/5 runs clean). Option 1 (SOCKS5 UDP ASSOCIATE over iroh's direct path, the quinn-PoC analogue) is **not feasible** without forking iroh: iroh exposes no socket-injection hook for the IP/direct transport. The recommendation is **Option 2 (force relay-only) as the default**, with Option 1 deferred unless a concrete direct-path-privacy use case surfaces that justifies an iroh fork.
**Date:** 2026-07-16
**Scope:** The iroh *direct* (hole-punched) connection path's peer-IP exposure when a SOCKS5 proxy is configured (OQ-67). The relay-mediated path is already covered by iroh's `proxy_url` (set at the assembly layer per ADR-089 §3); this document covers the gap: the direct path. The quinn case was settled separately in `../quinn-quic-proxy/findings.md`.

---

## TL;DR

1. **iroh does NOT expose a `new_with_abstract_socket`-equivalent for the direct path.** iroh's QUIC stack (`noq`, a quinn fork) *has* `Endpoint::new_with_abstract_socket` (`noq-1.0.1/src/endpoint.rs:162`), and iroh uses it internally (`iroh-1.0.2/src/socket.rs:1027`) — but it passes its own `Transport` multiplexer, and the public `noq_endpoint()` accessor is `pub(crate)` (`socket.rs:1121`). The IP/direct transport binds its own `netwatch::UdpSocket` internally (`socket/transports/ip.rs:164-167`) with **no injection point**. There is no public API to hand iroh a custom UDP socket for the direct path. The quinn PoC's `Socks5UdpSocket` (an `AsyncUdpSocket` impl) **does not transfer** to iroh.

2. **The only public socket-injection surface is `unstable-custom-transports`, and it is the wrong shape.** `Builder::add_custom_transport` (`endpoint.rs:813`, behind `feature = "unstable-custom-transports"`) accepts a `CustomTransport` (`socket/transports/custom.rs:24`) that produces `CustomEndpoint`/`CustomSender` impls. But custom transports operate on a **separate `CustomAddr` address space** (`AddrKind::Custom(u64)`), not the IP address space. A `CustomSender::poll_send` receives a `&CustomAddr` destination, not a `SocketAddr`. It is a *new transport type alongside IP/relay*, not a transparent replacement of the IP transport's underlying socket. You cannot make iroh's hole-punching machinery send SOCKS5-framed UDP to a peer's IP address through a `CustomTransport` — hole punching is hardcoded to the IP transport's `netwatch::UdpSocket`. See §3.

3. **Option 2 (force relay-only) is the right answer and it works.** Three public, stable Builder knobs together force every byte through the relay and the relay through the proxy:
   - `clear_ip_transports()` (`endpoint.rs:503`) — no IP/direct transport is bound at all, so there is no kernel UDP socket from which a direct path could start.
   - `addr_filter(AddrFilter::relay_only())` (`endpoint.rs:617`, `iroh-dns-1.0.2/src/endpoint_info.rs:269`) — the endpoint's published addresses (via address-lookup / the relay path) contain only relay URLs, no direct IPs, so the peer cannot attempt a direct connection.
   - `proxy_url(http://proxy)` (`endpoint.rs:690`) — the relay's WebSocket is tunneled through an HTTP CONNECT proxy. The relay sees the proxy's IP, not the client's.
   The PoC proves the composition: `selected_is_relay=true`, `selected_is_ip=false`, `any_direct_ip_path=false`, 45-byte echo through the proxied relay, 5/5 runs. The peer (server) sees the relay's IP; the relay sees the proxy's IP; the client's real IP is hidden on both surfaces.

4. **One correction to OQ-67's premises, important for the spec.** OQ-67 states iroh's `proxy_url` "proxies iroh's HTTP(S) traffic (relay connections, DNS-over-HTTPS, pkarr publishing) through the configured proxy." In iroh 1.0.x this is **only partially true**: `proxy_url` flows *only* to the relay transport actor (`socket/transports/relay/actor.rs:310-311`) and is used *only* for the relay's WebSocket connection (`iroh-relay-1.0.2/src/client/tls.rs:127-216`, an HTTP CONNECT handshake). It does **not** flow to the pkarr publisher (uses the `pkarr` crate directly, not reqwest), nor to the DNS-over-HTTPS resolver (uses `hickory-resolver` directly), nor to the `reqwest` client builder (`util.rs:80-91` has no `.proxy()` call). So `proxy_url` covers the **relay WebSocket** exposure surface only. For alknet this is fine — the relay WebSocket is the load-bearing path — but the spec should not claim pkarr/DoH are proxied by `proxy_url`. See §4.

5. **The integration into the alknet client crate is small and additive**, and composes with the quinn integration from `../quinn-quic-proxy/findings.md`. When `Socks5ProxyConfig` is set and the transport is iroh, the assembly layer (ADR-089 §3) builds the iroh `Endpoint` with the three relay-only knobs above. When the transport is raw quinn (the `dial_quic` path), it uses the `Socks5UdpSocket` from the quinn PoC. The two are mutually exclusive per-dial and selected by the transport enum. ~20 lines in the assembly's iroh-builder, no new trait, no iroh fork. See §6.

---

## 1. The problem (recap from OQ-67)

`dial_iroh` does not consume `Socks5ProxyConfig` (ADR-090 §5). iroh's `proxy_url` (set at `Endpoint` construction by the assembly layer) covers the **relay-exposure** surface: it tunnels the relay WebSocket through the proxy, hiding the client's IP from the relay. For the relay-mediated path this covers both exposure surfaces (peer sees relay's IP; relay sees proxy's IP).

The gap is the **direct path**: when iroh hole-punches a direct QUIC connection, the peer sees the client's real IP, and `Socks5ProxyConfig` does not cover it. OQ-67 posed three candidate approaches and asked for an investigation of iroh's socket stack to drive the choice.

## 2. iroh's socket stack — where the direct path lives

### 2.1 iroh uses a quinn fork called `noq`

iroh 1.0.x depends on `noq` (formerly `iroh-quinn`), a quinn fork. `noq` exposes the same `AsyncUdpSocket` trait and `Endpoint::new_with_abstract_socket` constructor that the quinn PoC relies on:

- `noq::AsyncUdpSocket` (`noq-1.0.1/src/runtime/mod.rs:44`) — same shape as `quinn::AsyncUdpSocket`: `create_sender`/`poll_recv`/`local_addr`/`max_receive_segments`/`may_fragment`.
- `noq::Endpoint::new_with_abstract_socket` (`noq-1.0.1/src/endpoint.rs:162`) — accepts a `Box<dyn AsyncUdpSocket>`.

So at the *noq* layer, the hook the quinn PoC uses exists. The question is whether **iroh** (the layer above noq) exposes it.

### 2.2 iroh does NOT expose the hook

iroh's `Endpoint` is built around an internal `Socket` + `Transports` multiplexer. At bind time (`iroh-1.0.2/src/socket.rs:1027`), iroh constructs the noq endpoint with its own `Transport` impl:

```rust
let endpoint = noq::Endpoint::new_with_abstract_socket(
    endpoint_config,
    Some(server_config),
    Box::new(Transport::new(sock.clone(), transports)),  // iroh's own multiplexer
    runtime.clone(),
)
```

The `Transport` multiplexer (`socket/transports.rs:952`, `impl noq::AsyncUdpSocket for Transport`) routes datagrams by address type:
- `Addr::Ip(SocketAddr)` → the IP transport's `netwatch::UdpSocket` (`socket/transports/ip.rs`).
- `Addr::Relay(url, id)` → the relay transport actor (a WebSocket over TCP+TLS).
- `Addr::Custom(CustomAddr)` → any registered `CustomTransport`.

The accessor for the underlying noq endpoint is **not public**:

```rust
// iroh-1.0.2/src/socket.rs:1121
pub(crate) fn noq_endpoint(&self) -> &noq::Endpoint { ... }
```

And the IP transport binds its own socket with no injection point:

```rust
// iroh-1.0.2/src/socket/transports/ip.rs:164-172
pub(crate) fn bind(config: Config, metrics: Arc<SocketMetrics>) -> io::Result<Self> {
    let addr: SocketAddr = config.into();
    let socket = netwatch::UdpSocket::bind_full(addr)?;   // <-- kernel UDP socket, no hook
    let local_addr = socket.local_addr()?;
    Ok(Self::new(config, Arc::new(socket), metrics.clone()))
}
```

There is no `Builder` method to supply a custom `UdpSocket` or `AsyncUdpSocket` for the IP transport. `Builder::bind_addr` / `bind_addr_with_opts` only configure *which address* to bind, not *what socket implementation* to use. The `Runtime::wrap_udp_socket` hook (`iroh-1.0.2/src/runtime.rs:106-111`) exists but is a `noq::Runtime` trait method that iroh's own `Runtime` impl marks as unused ("We're not actually using this function in iroh") and is not exposed on the `Builder`.

**Conclusion:** an application cannot hand iroh a `Socks5UdpSocket`-equivalent for the direct path. The direct UDP socket is created and owned deep inside iroh, behind `pub(crate)`.

### 2.3 What about `rebind_abstract`?

`noq::Endpoint::rebind_abstract` (`noq-1.0.1/src/endpoint.rs:280`) can swap the `AsyncUdpSocket` of a live endpoint. But iroh does not expose `noq_endpoint()` publicly, so an application cannot call it. And even if it could, swapping iroh's `Transport` multiplexer for a raw `Socks5UdpSocket` would *break iroh's relay and custom-transport routing* — the multiplexer is what makes `Addr::Relay` send over the WebSocket instead of UDP. You cannot replace it without losing iroh's core behavior.

## 3. Why `CustomTransport` is the wrong shape (Option 1 rejected)

The `unstable-custom-transports` feature (`iroh-1.0.2/src/socket/transports/custom.rs`) is the only public socket-injection surface. It exposes:

```rust
pub trait CustomTransport: Debug + Send + Sync + 'static {
    fn bind(&self) -> io::Result<Box<dyn CustomEndpoint>>;
}
pub trait CustomEndpoint: Debug + Send + Sync + 'static {
    fn watch_local_addrs(&self) -> n0_watcher::Direct<Vec<CustomAddr>>;
    fn create_sender(&self) -> Arc<dyn CustomSender>;
    fn poll_recv(&mut self, cx, bufs, metas, source_addrs: &mut [Addr]) -> Poll<io::Result<usize>>;
}
pub trait CustomSender: Debug + Send + Sync + 'static {
    fn is_valid_send_addr(&self, addr: &CustomAddr) -> bool;
    fn poll_send(&self, cx, dst: &CustomAddr, transmit: &Transmit) -> Poll<io::Result<()>>;
}
```

It looks promising — `poll_send` takes a `Transmit`, just like `AsyncUdpSocket::try_send`. But the critical difference is the **address space**:

- `CustomSender::poll_send` takes a **`&CustomAddr`** destination, not a `&SocketAddr`. Custom transports live in `AddrKind::Custom(u64)` (`socket/transports.rs:641-650`), a separate space from `AddrKind::IpV4`/`IpV6`.
- iroh's hole-punching machinery (`socket/remote_map/remote_state.rs:675-731`, `trigger_holepunching`/`do_holepunching`) operates on the **IP transport**: it reads `local_direct_addrs` (a `BTreeSet<SocketAddr>`) and `get_remote_nat_traversal_addresses` (also `SocketAddr`s) and sends STUN/UDP probes via the IP transport's `netwatch::UdpSocket`. A custom transport does not participate in this — it has its own address discovery via `watch_local_addrs` and is reached when a peer dials a `CustomAddr`.
- To use a `CustomTransport` as a SOCKS5-UDP tunnel to a peer's *IP* address, you would need iroh to route IP-destination hole-punch probes through your custom sender. It does not — hole punching is hardcoded to the IP transport. You would also need the peer to advertise a `CustomAddr` that maps to your SOCKS5 relay, which defeats the point (the peer doesn't know about your proxy).

A `CustomTransport` is designed for genuinely new network types (e.g. an in-memory test mesh, a Bluetooth transport, a tor onion service) where both peers speak the custom protocol. It is **not** a transparent proxy wrapper for the existing IP transport. The in-memory `TestTransport` (`iroh-1.0.2/src/test_utils/test_transport.rs`) confirms this: it uses `CustomAddr::from_parts(id, &[id])` and a `TestNetwork` of mpsc channels — a completely separate address space, not IP.

**Conclusion:** the quinn PoC pattern (implement `AsyncUdpSocket` with SOCKS5 UDP ASSOCIATE framing, plug it into `new_with_abstract_socket`) **does not transfer to iroh**. iroh does not expose the IP transport's socket for replacement, and the only injection surface (`CustomTransport`) operates on a different address space that iroh's hole-punching does not route through. Making Option 1 work would require forking iroh to either (a) expose `noq_endpoint()` and let the app swap the `Transport` multiplexer (breaking relay routing), or (b) add a socket-injection hook to `IpTransport::bind`. Both are invasive and carry ongoing re-tracking burden, for a use case that is currently hypothetical (see §5).

## 4. What `proxy_url` actually covers (correction to OQ-67)

OQ-67 (lines 36-40) states `proxy_url` "proxies iroh's HTTP(S) traffic (relay connections, DNS-over-HTTPS, pkarr publishing) through the configured proxy." Tracing the code in iroh 1.0.2:

- **Relay WebSocket: YES.** `Builder::proxy_url` (`endpoint.rs:690`) is stored in `Options` and flowed into `RelayConnectionOptions` (`socket.rs:852`), which the relay actor passes to `relay::client::ClientBuilder::proxy_url` (`socket/transports/relay/actor.rs:310-311`). The client's `dial_url_proxy` (`iroh-relay-1.0.2/src/client/tls.rs:127-216`) performs an **HTTP CONNECT** handshake (line 173: `.method("CONNECT")`) to tunnel the relay's WebSocket through the proxy. This is the load-bearing path for the relay-mediated connection.
- **pkarr publishing: NO.** The pkarr publisher (`iroh-1.0.2/src/address_lookup/pkarr.rs:579-616`) builds its reqwest client via `util::reqwest_client_builder` (`util.rs:80-91`), which calls `reqwest::Client::builder()` with TLS + DNS config but **no `.proxy()` call**. `proxy_url` is never passed to the pkarr publisher. (pkarr publishing to a pkarr relay server happens over plain HTTP/HTTPS via reqwest; without a proxy, the pkarr relay sees the client's IP.)
- **DNS-over-HTTPS: NO.** iroh-dns uses `hickory-resolver` (`iroh-dns-1.0.2/src/dns.rs:179`, `ConnectionConfig::https`) directly, not reqwest. `hickory-resolver` has its own connection layer that does not consult iroh's `proxy_url`. (The DoH server sees the client's IP.)
- **net_report / STUN-over-QUIC / portal detection: NO.** `net_report/reportgen.rs:588-617` builds a reqwest client without a proxy. The QUIC address-discovery (QAD) probes go over the iroh endpoint's own noq endpoint (`socket.rs:1012-1017`), which is the IP transport — not proxied.

**Implication for the spec:** `proxy_url` covers the **relay WebSocket** exposure surface (the relay sees the proxy's IP). It does **not** cover pkarr-publishing or DoH exposure surfaces. For alknet's privacy model this is acceptable *because* the recommended configuration is Option 2 (force relay-only): with `clear_ip_transports()`, QAD is disabled (`socket.rs:1012`: `has_ip_transports.then(...)`), and the peer-IP exposure is fully handled by the relay path. But the spec text in ADR-089/ADR-090 should be corrected to say "`proxy_url` proxies the relay WebSocket" rather than "proxies iroh's HTTP(S) traffic (relay connections, DNS-over-HTTPS, pkarr publishing)." If pkarr/DoH privacy matters for a deployment, that is a *separate* gap (not OQ-67's direct-path gap) and would need reqwest's `.proxy()` wired into `util::reqwest_client_builder` — a small upstream contribution, not a fork.

A secondary note: iroh's `proxy_url` expects an **HTTP CONNECT** proxy (the `http://` scheme; `tls.rs:143` checks `proxy_url.scheme() == "http"` for the proxy-to-relay leg, then does CONNECT regardless). It is **not** a SOCKS5 proxy. So the alknet `Socks5ProxyConfig` (ADR-090) is the *wrong shape* to feed directly into iroh's `proxy_url` — iroh wants an HTTP proxy URL. The assembly layer must either (a) require an HTTP CONNECT proxy URL for the iroh path (distinct from the SOCKS5 proxy used by the quinn/TCP paths), or (b) run a local HTTP-to-SOCKS5 bridge. See §6.2.

## 5. Option 2 (force relay-only) — the PoC

Location: `/workspace/iroh-proxy-poc`. Run: `cargo run -r`.

### 5.1 Topology

```
iroh client                                            iroh server
(relay-only, proxied)                                  (direct + relay)
   │                                                       │
   │  (no IP transport: clear_ip_transports())              │
   │  (no direct addrs published: addr_filter(relay_only))  │
   │                                                       │
   └──► HTTP CONNECT proxy ──► relay server ──► ───────────┘
        (tiny_http)          (iroh test_utils
                             run_relay_server)
```

- **Relay server**: `iroh::test_utils::run_relay_server()` — a local self-signed relay with QUIC enabled.
- **HTTP CONNECT proxy**: `src/proxy.rs` — a minimal `tiny_http`/tokio HTTP CONNECT proxy (the protocol iroh's `proxy_url` speaks). Not SOCKS5: iroh's relay client does HTTP CONNECT (`iroh-relay-1.0.2/src/client/tls.rs:173`).
- **Server endpoint**: `Endpoint::builder(presets::Minimal)` with `RelayMode::Custom(relay_map)` — normal, direct + relay.
- **Client endpoint**: `Endpoint::builder(presets::Minimal)` + the three relay-only knobs:
  ```rust
  .clear_ip_transports()              // no IP/direct transport
  .addr_filter(AddrFilter::relay_only())  // don't publish direct addrs
  .proxy_url(format!("http://{proxy_addr}").parse()?)  // tunnel relay WS
  ```

### 5.2 The three knobs, and why all three are needed

1. **`clear_ip_transports()`** — removes all `TransportConfig::Ip` entries (`endpoint.rs:503-506`). Without an IP transport, iroh has no kernel UDP socket to hole-punch from. This is the *source-side* suppression: there is no direct path to initiate.

2. **`addr_filter(AddrFilter::relay_only())`** — the `AddrFilter` (`iroh-dns-1.0.2/src/endpoint_info.rs:269`) is a filter function applied to the `Vec<TransportAddr>` that address-lookup services publish. `relay_only()` keeps only `TransportAddr::Relay` entries, dropping all `TransportAddr::Ip`. So the client's published `EndpointAddr` (what a peer learns via the relay/pkarr/DNS) contains only relay URLs. This is the *peer-side* suppression: even if the client had local direct addrs, the peer would never learn them, so the peer cannot attempt a direct connection *to* the client. (Belt-and-suspenders with #1, but matters for the symmetric case where the peer is the dialer.)

3. **`proxy_url(http://proxy)`** — tunnels the relay WebSocket through the HTTP CONNECT proxy. This is the *relay-exposure* suppression: the relay sees the proxy's IP, not the client's.

All three are stable, public, non-`unstable-*`-gated Builder methods (`clear_ip_transports` and `addr_filter` are unconditional; `proxy_url` is unconditional). None require a feature flag in the integration. (`add_custom_transport` is the only one behind `unstable-custom-transports`, and we don't use it.)

### 5.3 Result

```
INFO iroh_proxy_poc: local relay server up relay_url=https://127.0.0.1:33655/
INFO iroh_proxy_poc::proxy: HTTP CONNECT proxy listening addr=127.0.0.1:32799
INFO iroh_proxy_poc: server endpoint up (direct + relay) server_id=00c3...
INFO iroh_proxy_poc::proxy: proxy: CONNECT request peer=127.0.0.1:53088 line=CONNECT 127.0.0.1:33655 HTTP/1.1
INFO iroh_proxy_poc: client endpoint up (relay-only, proxied) client_id=0989...
INFO iroh_proxy_poc: client direct ip addrs: 0 (expected 0)
INFO iroh_proxy_poc: connection established
INFO iroh_proxy_poc: selected path selected_is_relay=true selected_is_ip=false remote=Relay(https://127.0.0.1:33655/) rtt_ms=14
INFO iroh_proxy_poc: PASS: selected path is a relay path (peer sees relay IP, not client IP)
INFO iroh_proxy_poc: direct IP path present? (expected false) any_direct_ip_path=false
INFO iroh_proxy_poc: echo exchange complete sent=45 recv=45
INFO iroh_proxy_poc: PASS: end-to-end data exchange over proxied relay path succeeded
INFO iroh_proxy_poc: === PoC complete: Option 2 (force relay-only) validated ===
```

5/5 consecutive runs clean. `cargo clippy -r` clean. The two `PASS` lines and the `CONNECT 127.0.0.1:33655` proxy log are the load-bearing evidence: the relay WebSocket went through the proxy, the selected path is relay, no direct IP path exists, and data flowed.

### 5.4 What is given up

- **Direct-connection latency.** The relay adds one network hop (client → relay → server). For a local relay (the hub-deployment case where the hub runs its own relay), this is negligible (the PoC's RTT is 14-34ms on localhost). For a far-away relay, it's the relay's round-trip. iroh's relay path is TCP+TLS+WebSocket-over-QUIC-framing, so it also forgoes QUIC's head-of-line-blocking-free UDP advantage on the client↔relay leg (the relay↔server leg can still be direct UDP from the relay's perspective).
- **Relay availability becomes a hard dependency.** With `clear_ip_transports()`, if the relay is down, the client cannot connect at all — there is no direct fallback. This is the intended privacy/availability tradeoff: privacy is absolute (no IP leak), availability is relay-bounded. For the hub deployment (ADR-087), the hub runs its own relay, so this is acceptable.

## 6. Integration into the alknet client crate

### 6.1 The change is small and composes with the quinn integration

The assembly layer (ADR-089 §3, "iroh shares the key, not the config") already constructs the iroh `Endpoint`. The change: when `Socks5ProxyConfig` is set *and* the transport is iroh, the assembly applies the three relay-only knobs. Sketch:

```rust
fn build_iroh_endpoint(
    secret_key: SecretKey,
    relay_map: RelayMap,
    proxy: Option<&ProxyConfig>,
) -> Result<iroh::Endpoint> {
    let mut builder = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .secret_key(secret_key)
        .relay_mode(iroh::RelayMode::Custom(relay_map))
        .alpns(vec![ALPN.to_vec()])
        .ca_tls_config(CaTlsConfig::insecure_skip_verify()); // or real roots

    if let Some(proxy) = proxy {
        builder = builder
            .clear_ip_transports()                          // no direct path
            .addr_filter(iroh::address_lookup::AddrFilter::relay_only())  // hide direct addrs
            .proxy_url(proxy.http_connect_url.clone()?);    // tunnel relay WS (see 6.2)
    }
    builder.bind().await
}
```

No `Connection`/stream/dispatch changes. The proxy is invisible above the endpoint layer. The quinn path (`dial_quic`) continues to use `Socks5UdpSocket` from `../quinn-quic-proxy/findings.md`; the iroh path uses the relay-only knobs. The two are selected by the transport enum at dial time.

### 6.2 The proxy-protocol mismatch (SOCKS5 vs HTTP CONNECT)

**This is the one real integration wrinkle.** ADR-090's `Socks5ProxyConfig` is a SOCKS5 proxy. iroh's `proxy_url` expects an **HTTP CONNECT** proxy (`iroh-relay-1.0.2/src/client/tls.rs:173`). They are different protocols:
- The quinn PoC uses SOCKS5 UDP ASSOCIATE (for UDP/QUIC).
- The TCP+TLS path (`../transport-generalization/findings.md`) uses SOCKS5 CONNECT (for TCP).
- iroh's relay path uses HTTP CONNECT (for the relay WebSocket).

So a single `Socks5ProxyConfig` cannot be fed verbatim into iroh's `proxy_url`. Three options for the spec:

1. **Require an HTTP CONNECT proxy URL for the iroh path** (distinct field, e.g. `ProxyConfig { socks5: Option<Socks5Config>, http_connect: Option<Url> }`). Cleanest, but asks the operator to configure two proxies. Most SOCKS5 proxy daemons (dante, etc.) don't also speak HTTP CONNECT.
2. **Run a local HTTP-to-SOCKS5 bridge** in the alknet client process when `Socks5ProxyConfig` is set and the iroh path is used: a tiny local HTTP CONNECT server (the PoC's `src/proxy.rs` is ~80 lines) that forwards each CONNECT tunnel to the SOCKS5 proxy's CONNECT command. Then point iroh's `proxy_url` at `http://127.0.0.1:<local-port>`. This unifies on a single `Socks5ProxyConfig` and is invisible to the operator. ~80 lines, no new deps beyond what the quinn PoC already uses (`fast-socks5`).
3. **Upstream an HTTP-or-SOCKS5 selector to iroh-relay's `dial_url_proxy`** so `proxy_url` accepts `socks5://`. Small upstream contribution (the `pkarr`/reqwest path would still need a separate fix per §4). Rejected for now — depends on upstream merge cadence.

Recommend **(2)** for the first integration: it keeps `Socks5ProxyConfig` as the single operator-facing config, composes with the quinn/TCP paths, and the bridge is tiny and local-only (no external surface). Revisit (3) if the bridge's extra localhost hop matters.

### 6.3 Feature flags

- `iroh` (existing): the iroh transport. Unchanged.
- `socks5` (new, from the quinn PoC): enables the SOCKS5 proxy paths. For the iroh relay-only path, the HTTP-to-SOCKS5 bridge (§6.2 option 2) lives behind this same flag. No new iroh-side feature flag — `clear_ip_transports`/`addr_filter`/`proxy_url` are all unconditional iroh APIs.

### 6.4 What does NOT change

- `Connection`/`SendStream`/`RecvStream` (transport-generalization composes: a proxied iroh relay connection still yields a `Connection`).
- The dispatch loop / `Dispatcher` / `CallConnection`.
- The server side. The server endpoint is unchanged — it just sees a connection arriving from the relay's IP.
- The quinn integration (`../quinn-quic-proxy/findings.md`). The two proxy integrations are independent per-transport.

## 7. Recommendation for OQ-67

**Resolve OQ-67 with Option 2 (force relay-only) as the default.** It is validated, uses only stable public iroh APIs, requires no fork, and fully closes the peer-IP-exposure gap for the iroh direct path (by eliminating the direct path when a proxy is configured). The privacy/availability tradeoff (relay-bounded availability, no direct-fallback) is the intended one for a privacy-conscious deployment and is acceptable for the hub deployment (ADR-087, where the hub runs its own relay).

**Defer Option 1 (SOCKS5 UDP ASSOCIATE over iroh's direct path) unless a concrete use case surfaces** that needs *both* direct-path peer-IP privacy *and* direct-path latency. The investigation shows it requires forking iroh (no public socket-injection hook for the IP transport; `CustomTransport` is the wrong address space). Without a use case that justifies the fork maintenance burden, it is not worth building. The quinn PoC's `Socks5UdpSocket` remains the reference shape *if* iroh ever exposes an IP-transport socket hook upstream.

**Correct the spec text** in ADR-089/ADR-090: `proxy_url` proxies the iroh **relay WebSocket** (HTTP CONNECT), not pkarr/DoH. Add the HTTP-to-SOCKS5 bridge (§6.2) to the client crate's proxy integration so a single `Socks5ProxyConfig` covers the iroh relay path, the quinn direct path, and the TCP+TLS path uniformly.

This does not block the first hub deployment or the QUIC/TCP+TLS proxy capability (ADR-090) — it is the iroh-direct-path resolution, and it lands as the conservative default.

## 8. References

- **iroh source (cargo cache, `iroh-1.0.2`):**
  - `src/endpoint.rs:503` — `Builder::clear_ip_transports` (force relay-only knob 1).
  - `src/endpoint.rs:617` — `Builder::addr_filter` (knob 2).
  - `src/endpoint.rs:690` — `Builder::proxy_url` (knob 3).
  - `src/endpoint.rs:813` — `Builder::add_custom_transport` (the `unstable-custom-transports` hook; wrong shape, §3).
  - `src/socket.rs:1027` — iroh's internal `noq::Endpoint::new_with_abstract_socket` call (passes its own `Transport`).
  - `src/socket.rs:1121` — `pub(crate) fn noq_endpoint` (the hook is not public).
  - `src/socket/transports.rs:952` — `impl noq::AsyncUdpSocket for Transport` (the multiplexer).
  - `src/socket/transports/ip.rs:164-172` — `IpTransport::bind` binds `netwatch::UdpSocket` with no injection point.
  - `src/socket/transports/custom.rs:24,36,79,86,92` — `CustomTransport`/`CustomEndpoint`/`CustomSender` (the wrong-shape injection surface).
  - `src/socket/transports/relay/actor.rs:310-311` — `proxy_url` flows to the relay client builder.
  - `src/runtime.rs:106-111` — `wrap_udp_socket` (unused, not exposed on `Builder`).
  - `src/util.rs:80-91` — `reqwest_client_builder` (no `.proxy()`; pkarr/DoH not proxied by `proxy_url`).
- **iroh-relay source (`iroh-relay-1.0.2`):**
  - `src/client/tls.rs:127-216` — `dial_url_proxy`: the HTTP CONNECT handshake that `proxy_url` drives.
  - `src/client/tls.rs:143,173` — `proxy_url.scheme() == "http"` check + `Method::CONNECT` (iroh wants an HTTP proxy, not SOCKS5).
- **iroh-dns source (`iroh-dns-1.0.2`):**
  - `src/endpoint_info.rs:269` — `AddrFilter::relay_only` (the filter used by knob 2).
  - `src/dns.rs:179` — DoH via `hickory-resolver` (not reqwest; not proxied by `proxy_url`).
- **noq source (`noq-1.0.1`):**
  - `src/runtime/mod.rs:44` — `AsyncUdpSocket` trait (same shape as quinn's).
  - `src/endpoint.rs:162` — `Endpoint::new_with_abstract_socket` (the hook iroh uses internally but doesn't expose).
  - `src/endpoint.rs:280` — `rebind_abstract` (not reachable from outside iroh).
- **PoC:** `/workspace/iroh-proxy-poc` — `cargo run -r`. Load-bearing files: `src/main.rs` (Option 2 end-to-end), `src/proxy.rs` (HTTP CONNECT proxy).
- **Related alknet work:**
  - `../quinn-quic-proxy/findings.md` — the quinn-over-SOCKS5 PoC (the reference shape Option 1 would need; does not transfer to iroh, §3).
  - `../transport-generalization/findings.md` — `Connection::from_stream` composes with this.
  - `docs/architecture/questions/067-iroh-proxy-support.md` — the OQ this resolves.
  - ADR-090 §5 (defers iroh proxy to this OQ), ADR-089 §3 (assembly sets `proxy_url`), ADR-087 §3 (the iroh exception on the client side).