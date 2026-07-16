# OQ-55: AlknetClient / Client Establishment Extraction

- **Origin**: `docs/research/alknet-channels/phase-0-findings.md` OQ-CH-14
  (the `AlknetClient` clarification); `docs/research/alknet-channels/poc-summary.md`
  §"Issues Surfaced" #1 (the `BidiStreamSource` finding that motivates
  separating the client-extraction question from the `Connection` extension
  question).
- **Status**: resolved (ADR-089)
- **Door type**: one-way (crate existence + dial seam — ADR-089)
- **Priority**: medium
- **Resolution**: `AlknetClient` is extracted as a new crate
  `alknet-client` — the client-side analogue of `AlknetEndpoint`
  (ADR-083). Three dial methods: `dial_quic` / `dial_tcp_tls` (both
  build a `TlsClientConfig` via ADR-087 and call their transport's
  connector) + `dial_iroh` (the key-not-config exception, shares the
  `Ed25519SecretKey`). The dial produces a `Connection` for the
  protocol take-overs (`CallClient::spawn_dispatch`,
  `ChannelClient::from_connection`) to consume — it does not run
  protocols.
- **Why the deferral collapsed**: the blocking condition ("a second
  transport's real dial existing") is met *within the native endpoint
  type* (ADR-086): QUIC + TCP+TLS (both rustls-consuming via
  `TlsClientConfig`) + iroh (key-based) — three dial shapes, two
  sharing `TlsClientConfig`. ADR-087 broke the circular hedge
  (`TlsClientConfig` is a prerequisite, not a consequence of the dial).
  ADR-083 made the server-side shape a clean accept-loop runner, giving
  the client-side shape by symmetry. The dial seam is extractable from
  two different transport implementations, not guessable from one.
- **Scope of the resolution**: the **native** transport-polymorphic
  dial. The web/browser client (WebSocket, HTTP — the browser
  bidirectional path per ADR-044/048) was never what this OQ was
  about — it is a different client surface (the JS SDK / wasm), not a
  Rust dial, and does not use `AlknetClient`. Non-Rust native clients
  (Node/Deno/Bun, Python, wasm) that negotiate TLS against an X.509
  endpoint and implement the wire protocols directly are also out of
  scope — `AlknetClient` is the Rust native client, one of several
  possible native clients sharing the same wire protocols.
- **What does NOT block on this (unchanged)**: each crate building its
  own client standalone with a shared `TlsClientConfig` (ADR-087), and
  each transport-specific dial helper. `CallClient`'s transport-agnostic
  take-over (`spawn_dispatch`) and `ChannelClient`'s transport-agnostic
  take-over (`from_connection`, ADR-080) are decided; the
  `connect` / `connect_quic` convenience constructors are **removed**
  per ADR-089 §5 (not delegated — the dial is centralized in
  `AlknetClient`, and the protocol crates shed their TLS/transport deps).
- **Cross-references**: ADR-089 (the resolution — `AlknetClient` native
  dial seam), ADR-083 (the server-side shape mirrored), ADR-086
  (endpoint types — native has QUIC + TCP+TLS + iroh), ADR-087
  (`TlsClientConfig` — the prerequisite the dial consumes), ADR-034
  (verifier selection — centralized in `TlsClientConfig::new`), ADR-065
  (server-side transport generalization — the client-side analogue
  this OQ's deferral avoided preempting, now completed), ADR-070 (the
  `BidiStreamSource` extension point — orthogonal to the client
  establishment question), OQ-CH-14 in
  `docs/research/alknet-channels/phase-0-findings.md` (the research-scope
  question this core-scope OQ carried forward).