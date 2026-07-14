# OQ-60: Where Does Transport Construction Live?

- **Origin**: `docs/architecture/decisions/083-endpoint-as-accept-loop-runner.md`
  (the endpoint refactor commits to the boundary — construction is not
  in the endpoint — but not the location of transport construction).
- **Status**: resolved
- **Door type**: one-way (where `build_iroh_endpoint` and the TCP+TLS
  loop live determines who depends on `iroh` / `tokio-rustls` for
  transport construction; the dep-graph shape is structural)
- **Priority**: high (the hub is the first multi-transport consumer;
  its assembly code sets the pattern)
- **Resolution**: Split answer (resolved by ADR-083 revision, 2026-07-14):

  **TCP+TLS accept loop → `alknet-core` behind a `tcp` feature (owned
  by the endpoint).** The endpoint takes a `TcpListener` +
  `TlsAcceptor` via `with_tcp_tls(listener, acceptor)` and runs the
  accept loop inside `run()` alongside the quinn and iroh loops. TCP+TLS
  is a listener transport — same shape as quinn and iroh (accept →
  extract ALPN + fingerprint → `Connection::from_bidi` → `dispatch`).
  Making it owned gives the endpoint a single uniform ownership model:
  it owns all its accept loops, `shutdown()` stops them all. The
  multi-owner shutdown problem (OQ-61) does not arise. Any node that
  wants TCP+TLS enables the `tcp` feature and calls `with_tcp_tls` — no
  hub dependency. This reverses ADR-010's "TCP is not an endpoint struct
  concern": the reason TCP was excluded (the endpoint built transports
  internally, TCP+TLS couldn't fit) is gone; the endpoint is now a
  multi-transport accept-loop runner and TCP+TLS fits the same shape.

  **Builder functions (`build_iroh_endpoint`, `build_quinn_endpoint`,
  `build_tcp_tls`) → inlined by the assembly layer.** These are trivial
  API calls (2-15 lines each, pure configuration, no shared logic). The
  assembly layer (the deployment binary — today primarily the hub
  crate's composition code) inlines them. No helper crate or module —
  20 lines total across all three. A `alknet-transport` crate was
  considered and rejected: it would contain only trivial builders (the
  real component, the TCP+TLS loop, is in core). Hub-specific transport
  helpers, if they accumulate, live in the hub crate, not a generic
  transport crate.

  **`dispatch` stays public** — but for genuinely external shapes (SSH
  channels, future WebTransport streams), not for TCP+TLS. The
  distinction: listener transports (quinn, iroh, TCP+TLS) produce
  connections from an accept loop the endpoint owns; multiplexing
  transports (SSH, WT) produce connections from within an existing
  connection, and the endpoint can't own their accept loop.

  **Why not the hub crate for the TCP+TLS loop:** "the assembly layer"
  is, in practice, usually the hub. But a hub-worker serving TCP+TLS
  shouldn't depend on the hub crate for a transport loop. The loop
  belongs in core (behind a feature), where any node can use it. The hub
  crate owns hub-specific *composition* (wiring adapters, relay, peer
  lifecycle), not transport runtimes.

  **Why not `alknet-tls`:** `alknet-tls`'s stated job is TLS setup —
  `rustls::ServerConfig`, cert resolvers, ACME (ADR-082). The TCP+TLS
  accept loop is transport runtime, not TLS setup. It calls
  `endpoint.dispatch()`, which is core's API. Adding it to `alknet-tls`
  would make a cert-provider crate depend on the endpoint's dispatch API
  and own a transport runtime — a category error.

  **Why not `alknet-transport` (new crate):** The real component (the
  TCP+TLS loop) is in core. What's left for a transport crate is trivial
  builder functions. A crate for 20 lines of API calls doesn't earn its
  existence. If future transport runtimes accumulate that don't fit
  core, a transport crate can be created then — but not speculatively.
- **Cross-references**: ADR-083 (endpoint refactor — the revision that
  resolved this), ADR-082 (`alknet-tls` — the cert provider boundary),
  ADR-010 (original endpoint design — "TCP is not an endpoint struct
  concern" is revised), OQ-61 (dissolved — the multi-owner shutdown
  problem does not arise with TCP+TLS owned by the endpoint)