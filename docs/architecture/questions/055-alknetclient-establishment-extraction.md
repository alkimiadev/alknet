# OQ-55: AlknetClient / Client Establishment Extraction

- **Origin**: `docs/research/alknet-channels/phase-0-findings.md` OQ-CH-14
  (the `AlknetClient` clarification); `docs/research/alknet-channels/poc-summary.md`
  §"Issues Surfaced" #1 (the `BidiStreamSource` finding that motivates
  separating the client-extraction question from the `Connection` extension
  question).
- **Status**: deferred(scope)
- **Door type**: two-way
- **Priority**: medium
- **Impacts**: Does NOT block individual transport dials — each
  transport-specific dial helper builds its `TlsClientConfig` (ADR-087)
  and its own connector standalone. Blocks only the *shared*
  `AlknetClient::dial()` extraction (one entry point that picks the
  transport and calls the right connector). Low impact until a second
  transport's dial exists and the duplicated dial boilerplate becomes
  worth extracting.
- **Blocked on**: a **second transport's** real dial existing, not just a
  second QUIC dial. The blocking condition is met when, e.g., the SSH
  crate's raw-TCP dial or the HTTP-wrapped call dial exists — so the
  transport-polymorphic dial+TLS seam is extractable from two
  *different* transport implementations, not two QUIC variants.
  `ChannelClient`'s `connect_quic` does not unblock this; it is a second
  *client* but the same *transport shape*. `ChannelClient`'s
  `from_connection` (the transport-agnostic take-over, ADR-080) is decided
  and is not the thing being deferred — the shared *dial* is. The deferral
  is on transport-polymorphism of the dial, not on client count or on the
  channels protocol's API.
- **What is NOT deferred (amended by ADR-087)**: the client-side TLS
  config (`TlsClientConfig`) is **not** part of this deferral. OQ-64
  is resolved — `alknet-tls` provides `TlsClientConfig` now, unblocked
  by this OQ. The TLS config is a **prerequisite** for the dial, not a
  consequence of it; it is transport-agnostic (ADR-034 verifier
  selection + ADR-084 provider, both already decided). Each
  transport-specific dial helper builds its `TlsClientConfig` and
  passes it to its transport's connector. This OQ defers only the
  *transport-polymorphic dial extraction* — the shared `AlknetClient::dial()`
  that picks the transport and calls the right connector. When a second
  transport's dial exists, the dial seam is extractable; the TLS config
  is already shared by then.
- **Resolution**: Not yet decidable. The shared substance across
  TLS-carrying transports is ADR-034's verifier-selection rule (PeerEntry
  presence → fingerprint pin : CA-verify / fail-closed) and the
  `rustls::ClientConfig` construction — now centralized in
  `TlsClientConfig::new` (ADR-087, OQ-64 resolved). What remains
  transport-specific is the *dial* itself — `quinn::Endpoint::connect`,
  `TcpStream::connect` + `TlsConnector::connect`, iroh's
  `Endpoint::connect`. We have one of ~5 shapes implemented (QUIC; the
  others being HTTP, TCP+TLS, WebTransport, raw TCP). Extracting a
  QUIC-shaped connector to core and naming it `AlknetClient` would bake
  QUIC in as *the* establishment shape — the same welding ADR-065
  unwound on the server side, repeated on the client side. The dial is
  transport-polymorphic; the shared TLS config is narrow and now
  extracted (ADR-087). Until a second transport's dial exists, the seam
  between "dial" (per-transport) and "spawn the dispatcher" (per-crate)
  is not extractable from two real shapes — it's guessable from one.
- **What does NOT block on this**: each crate building its own client
  standalone with a shared `TlsClientConfig` (ADR-087), and each
  transport-specific dial helper. `CallClient`'s transport-agnostic
  take-over (`spawn_dispatch`) and `ChannelClient`'s transport-agnostic
  take-over (`from_connection`, ADR-080) are decided; `connect_quic`
  dials QUIC and calls `from_connection`. The SSH crate's TCP client,
  the HTTP call client, a `connect_tcp_tls` / `connect_webtransport`
  helper — each builds its `TlsClientConfig` (ADR-087) and its own dial
  standalone. Core already permits all of this —
  `Connection::from_stream` / `from_bidi` (ADR-065) handles the
  non-QUIC transport on the server side, and nothing prevents a client
  from constructing a `Connection` the same way after its own
  transport-specific dial. The friction is the duplicated dial
  boilerplate (each dial helper calls its transport's connector), not
  duplicated TLS config (that is shared via ADR-087). The
  bidirectionality criterion (a crate needs a Client type when (a) the
  endpoint has protocol-level authority — e.g., channels' id allocation
  — or (b) the protocol needs a reliable establishment interface) is
  met by each crate independently; `AlknetClient` is the eventual
  *shared dial seam*, not a prerequisite for any single client to
  exist.
- **Cross-references**: ADR-034 (verifier selection — centralized in
  `TlsClientConfig::new` per ADR-087), ADR-087 (`TlsClientConfig` is
  not blocked on this OQ — the TLS config is extracted; only the dial
  remains deferred), ADR-065 (server-side transport generalization —
  the client-side analogue this OQ's deferral avoids preempting),
  ADR-070 (the `BidiStreamSource` extension point, which is the
  *Connection* opening and is orthogonal to the *client* establishment
  question), OQ-CH-14 in
  `docs/research/alknet-channels/phase-0-findings.md` (the research-scope
  question this core-scope OQ carries forward).