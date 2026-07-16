# OQ-67: iroh Proxy Support (Direct-Connection Peer Exposure)

- **Origin**: `docs/architecture/decisions/090-client-dial-socks5-proxy-seam.md`
  §5; `docs/architecture/crates/client/README.md` §"SOCKS5 proxy
  (ADR-090)".
- **Status**: deferred(unclear)
- **Door type**: two-way
- **Priority**: medium
- **Impacts**: A client that dials over iroh direct (hole-punched)
  connections and wants to hide its real IP from the peer. Does NOT
  block the first hub deployment — the hub's outbound worker dials
  (the hub-as-client case, ADR-087 §5) use QUIC or TCP+TLS, both of
  which honor the proxy (ADR-090). Does NOT block the relay-mediated
  iroh path — iroh's relay already hides the client's IP from the peer
  (the peer sees the relay's IP), and iroh's `proxy_url` (set at the
  assembly layer) hides the client's IP from the relay. The gap is the
  iroh *direct* path: when iroh hole-punches a direct QUIC connection,
  the peer sees the client's real IP, and `Socks5ProxyConfig` (ADR-090)
  does not cover it.
- **Investigation**: Work through the iroh socket stack to determine
  whether iroh exposes a socket abstraction analogous to quinn's
  `AsyncUdpSocket` + `new_with_abstract_socket` (the hook the quinn
  POC uses, validated in `docs/research/quinn-quic-proxy/findings.md`).
  If it does, a `Socks5UdpSocket`-equivalent for iroh is the same shape
  as the quinn integration (adapt the PoC). If it does not, the
  alternatives are: (a) force relay-only when a proxy is configured
  (disable iroh's direct-connection path — the peer always sees the
  relay's IP, never the client's), or (b) accept the gap (document that
  iroh direct connections expose the client's IP, and privacy-conscious
  iroh deployments use `proxy_url` + relay-only). The investigation
  needs a concrete iroh-direct-with-proxy use case to drive the choice
  — without one, "force relay-only" is the conservative default.
- **What is decided (ADR-090 §5)**: `dial_iroh` does **not** consume
  `Socks5ProxyConfig`. iroh's `proxy_url` (set at `iroh::Endpoint`
  construction by the assembly layer — ADR-089 §3, "iroh shares the
  key, not the config") covers the relay-exposure surface: it proxies
  iroh's HTTP(S) traffic (relay connections, DNS-over-HTTPS, pkarr
  publishing) through the configured proxy, hiding the client's IP
  from iroh's relay. For the relay-mediated path, this covers both
  exposure surfaces (peer sees relay's IP; relay sees proxy's IP). The
  open case is the direct path.
- **What is open**: how to cover iroh's *direct* (hole-punched)
  connection path when a SOCKS5 proxy is configured. Three candidate
  approaches:
  1. **Wrap iroh's QUIC in SOCKS5 UDP ASSOCIATE.** Analogous to the
     quinn POC — if iroh exposes a socket abstraction that accepts a
     custom UDP impl, wrap it in a `Socks5UdpSocket`-equivalent. This
     is a *different* integration than the quinn POC (iroh has its own
     socket stack, not `quinn::AsyncUdpSocket`), so the PoC does not
     directly transfer. Requires investigating iroh's API surface for a
     socket-injection hook.
  2. **Force relay-only when a proxy is configured.** Disable iroh's
     direct-connection path when `Socks5ProxyConfig` is set (or when
     the assembly layer sets iroh's `proxy_url`); all iroh connections
     go through the relay, which hides the client's IP from the peer
     by design. This is the conservative default — no new socket
     integration, no ECN loss (the relay path is TCP-based), but it
     gives up iroh's direct-connection latency advantage.
  3. **Accept the gap.** Document that iroh direct connections expose
     the client's IP to the peer, and that privacy-conscious iroh
     deployments should use `proxy_url` (relay-exposure) + force
     relay-only (peer-exposure). This is honest but leaves the
     `Socks5ProxyConfig` partially effective for iroh.
  The choice depends on whether a concrete iroh-direct-with-proxy use
  case exists that needs the peer-IP privacy AND the direct-connection
  latency. Without that use case, (2) is the conservative default; if
  the use case exists, (1) is the target (and needs the iroh socket
  investigation).
- **Why deferred(unclear), not deferred(scope)**: The pieces exist —
  SOCKS5 UDP ASSOCIATE is validated for quinn (the PoC), iroh's
  `proxy_url` exists and covers the relay path, iroh's direct vs relay
  path is a known iroh property. What is unclear is the *composition*
  for iroh specifically: does iroh expose a socket hook (making (1)
  feasible), and is there a concrete use case that needs direct-path
  privacy (making (1) worth building vs. (2) sufficient)? The
  resolution is *investigation* (work through iroh's socket stack, find
  the use case), not *waiting* (for a spec or use case to arrive). The
  quinn POC settled the quinn case; the iroh case needs its own
  investigation, not a copy of the quinn answer.
- **Resolution**: Not yet decidable. The iroh socket stack needs
  investigation (does it expose a `new_with_abstract_socket`-equivalent?),
  and a concrete iroh-direct-with-proxy use case needs to surface
  (or a deliberate decision to force relay-only needs to be made). The
  quinn POC's `Socks5UdpSocket` is the reference shape if iroh exposes
  the hook; the "force relay-only" fallback is the conservative default
  if it does not or if no use case demands direct-path privacy. This
  does not block the first hub deployment or the QUIC/TCP+TLS proxy
  capability (ADR-090) — it is a gap specific to iroh direct
  connections.
- **Cross-references**: ADR-090 (§5 — defers iroh proxy support to
  this OQ), ADR-089 (§3 — iroh shares the key, not the config;
  `proxy_url` is set at the assembly layer), ADR-087 (§3 — the iroh
  exception on the client side),
  [`docs/research/quinn-quic-proxy/findings.md`](../../research/quinn-quic-proxy/findings.md)
  (the quinn-over-SOCKS5 PoC — the reference shape for the iroh
  investigation).