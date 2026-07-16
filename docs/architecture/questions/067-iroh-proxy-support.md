# OQ-67: iroh Proxy Support (Direct-Connection Peer Exposure)

- **Origin**: `docs/architecture/decisions/090-client-dial-socks5-proxy-seam.md`
  §5; `docs/architecture/crates/client/README.md` §"SOCKS5 proxy
  (ADR-090)".
- **Status**: resolved (by the iroh-proxy POC and ADR-090 §5 amendment,
  2026-07-16)
- **Door type**: one-way (the force-relay-only decision is a deployment
  tradeoff: relay-bounded availability, no direct fallback)
- **Priority**: medium
- **Impacts** (now closed): A client that dials over iroh direct
  (hole-punched) connections and wants to hide its real IP from the
  peer. Did NOT block the first hub deployment — the hub's outbound
  worker dials (the hub-as-client case, ADR-087 §5) use QUIC or TCP+TLS,
  both of which honor the proxy (ADR-090). Did NOT block the
  relay-mediated iroh path. The gap was the iroh *direct* path: when
  iroh hole-punches a direct QUIC connection, the peer sees the client's
  real IP. **Resolved by eliminating the direct path when a proxy is
  configured** — force relay-only.
- **Investigation** (completed): Worked through the iroh socket stack
  to determine whether iroh exposes a socket abstraction analogous to
  quinn's `AsyncUdpSocket` + `new_with_abstract_socket`. **Result: it
  does not.** iroh uses a quinn fork (`noq`) that *has*
  `new_with_abstract_socket`, but iroh calls it internally with its
  own `Transport` multiplexer and keeps `noq_endpoint()` as
  `pub(crate)`. The IP/direct transport binds its own
  `netwatch::UdpSocket` with no injection point. The only public
  socket-injection surface (`unstable-custom-transports`,
  `CustomTransport`) operates on a separate `CustomAddr` address space
  that iroh's hole-punching does not route through — wrong shape. The
  quinn POC's `Socks5UdpSocket` does not transfer to iroh. Making
  SOCKS5-UDP-over-iroh-direct work would require forking iroh.
- **What is decided (ADR-090 §5, amended 2026-07-16)**: `dial_iroh`
  with a proxy configured forces **relay-only** via three stable public
  iroh Builder knobs: `clear_ip_transports()` (no IP/direct transport
  is bound), `addr_filter(AddrFilter::relay_only())` (no direct IPs
  published), and `proxy_url(http://...)` (relay WebSocket tunneled
  through an HTTP CONNECT proxy). The peer sees the relay's IP; the
  relay sees the proxy's IP; the client's real IP is hidden on both
  surfaces. The POC (`/workspace/iroh-proxy-poc`,
  `docs/research/iroh-proxy-poc/findings.md`) validates this
  end-to-end: `selected_is_relay=true`, `selected_is_ip=false`,
  `any_direct_ip_path=false`, 45-byte echo through the proxied relay,
  5/5 runs clean. No iroh fork required — all three knobs are
  unconditional public APIs.
- **What is open**: the direct-path SOCKS5-UDP option (Option 1) is
  **not pursued**. It requires forking iroh (no public IP-transport
  socket-injection hook; `CustomTransport` is the wrong address space),
  for a use case that is currently hypothetical (direct-path peer-IP
  privacy with direct-path latency). The quinn POC's `Socks5UdpSocket`
  remains the reference shape *if* iroh ever exposes an IP-transport
  socket hook upstream — tracked informally, not as an OQ (no concrete
  use case). The `proxy_url` coverage gap on pkarr/DoH (see below) is a
  separate gap, not this OQ's direct-path question.
- **Correction to this OQ's original premises**: the original
  "What is decided" section stated iroh's `proxy_url` "proxies iroh's
  HTTP(S) traffic (relay connections, DNS-over-HTTPS, pkarr
  publishing)." Tracing iroh 1.0.2, this is **only partially true**:
  `proxy_url` flows only to the relay transport actor and is used only
  for the relay's WebSocket connection (an HTTP CONNECT handshake). It
  does **not** flow to the pkarr publisher (uses the `pkarr` crate
  directly, not reqwest), nor to the DNS-over-HTTPS resolver (uses
  `hickory-resolver` directly), nor to the `reqwest` client builder
  (`util.rs:80-91` has no `.proxy()` call). `proxy_url` covers the
  **relay WebSocket** exposure surface only. For alknet's privacy
  model this is fine because the recommended configuration is force
  relay-only: with `clear_ip_transports()`, QAD (QUIC address
  discovery) is disabled, and peer-IP exposure is fully handled by the
  relay path. If a deployment needs pkarr/DoH proxied too, that is a
  separate gap (a small upstream contribution to wire reqwest's
  `.proxy()` into iroh's `util::reqwest_client_builder`), not this OQ.
- **The HTTP-to-SOCKS5 bridge**: iroh's `proxy_url` expects an HTTP
  CONNECT proxy, but `Socks5ProxyConfig` (ADR-090) is SOCKS5. The
  integration runs a tiny local HTTP-to-SOCKS5 bridge (~80 lines, the
  POC's `src/proxy.rs` is the template) in the alknet client process
  when `Socks5ProxyConfig` is set and the iroh path is used. iroh's
  `proxy_url` is pointed at `http://127.0.0.1:<local-port>`. This
  unifies on a single `Socks5ProxyConfig` and is invisible to the
  operator. Behind the `socks5` feature flag.
- **What is given up**: force relay-only forgoes iroh's
  direct-connection latency advantage (the relay adds one network hop).
  For the hub deployment (hub runs its own relay), this is negligible.
  Relay availability becomes a hard dependency — with
  `clear_ip_transports()`, if the relay is down, the client cannot
  connect at all. This is the intended privacy/availability tradeoff; a
  caller that prefers availability over privacy for the iroh path
  simply does not set the proxy.
- **Resolution**: **Force relay-only (Option 2) is the default.** Validated
  by the iroh-proxy POC, uses only stable public iroh APIs, requires no
  fork, and fully closes the peer-IP-exposure gap (by eliminating the
  direct path when a proxy is configured). Option 1 (SOCKS5 UDP
  ASSOCIATE over iroh's direct path) is not feasible without forking
  iroh and is not pursued (no concrete use case justifies the fork
  maintenance burden). The `Socks5ProxyConfig` covers all three dials
  uniformly: UDP ASSOCIATE for `dial_quic`, CONNECT for `dial_tcp_tls`,
  and force-relay-only + HTTP-to-SOCKS5 bridge for `dial_iroh`. See
  ADR-090 §5 (amended 2026-07-16).
- **Cross-references**: ADR-090 (§5 — resolves this OQ with the
  force-relay-only decision + HTTP-to-SOCKS5 bridge), ADR-089 (§3 —
  iroh shares the key, not the config; `proxy_url` is set at the
  assembly layer), ADR-087 (§3 — the iroh exception on the client side),
  [`docs/research/quinn-quic-proxy/findings.md`](../../research/quinn-quic-proxy/findings.md)
  (the quinn-over-SOCKS5 PoC — the reference shape Option 1 would need;
  does not transfer to iroh),
  [`docs/research/iroh-proxy-poc/findings.md`](../../research/iroh-proxy-poc/findings.md)
  (the iroh-proxy POC findings — the investigation that resolved this
  OQ).