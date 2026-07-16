# OQ-64: Should `alknet-tls` Provide a Client-Side TLS Config Helper?

- **Origin**: `docs/architecture/crates/tls/README.md` (the "Server-only
  for now" section flagged this as deferred); ADR-084 (requires the
  client-side `rustls::ClientConfig` to use the same `aws_lc_rs`
  provider — previously enforced by convention, not shared code).
- **Status**: resolved
- **Door type**: one-way (`TlsClientConfig` as the shared client-side
  TLS config in `alknet-tls` is structural — every outbound-dialing
  crate depends on it. Reversing would re-distribute verifier selection
  + provider wiring across crates.)
- **Priority**: high (upgraded from medium — the hub-as-client
  requirement makes this a prerequisite for the first hub deployment,
  not a future extraction)
- **Resolution**: **Yes. `alknet-tls` provides `TlsClientConfig`.** It
  is not blocked on the dial-seam extraction (OQ-55).

  The previous deferral linked OQ-64 and OQ-55 as "the same seam,"
  creating a circular dependency: the TLS config is deferred behind the
  dial, but the dial needs the TLS config. The circle is broken by
  separating the two concerns:

  1. **`TlsClientConfig`** — the `rustls::ClientConfig` + ADR-034
     verifier selection + ADR-084 crypto provider. Transport-agnostic.
     All decisions are made. Buildable today. It is a **prerequisite**
     for any dial, not a consequence of it.
  2. **The dial** (`AlknetClient::dial()`) — transport-specific
     connection establishment. Extracting a transport-polymorphic dial
     from one shape (QUIC) would bake QUIC in. **Legitimate deferral
     (OQ-55, unchanged).**

  `TlsClientConfig::new` takes a verifier context (the inputs to
  ADR-034's rule: `PeerEntry` presence, expected fingerprint, remote
  cert type) and returns a `rustls::ClientConfig`. The caller
  (transport-specific dial helper — now `AlknetClient::dial_quic` /
  `dial_tcp_tls` per ADR-089; the per-protocol `CallClient::connect` /
  `ChannelClient::connect_quic` convenience constructors are removed
  per ADR-089 §5) passes it to the transport's connector. Iroh is
  the exception — it has its own TLS and does not consume a
  `rustls::ClientConfig`; the iroh dial helper applies the same
  ADR-034 rule via iroh's `NodeId` verification API.

  The hub makes this non-optional: a hub dials out to workers it
  supervises and to other hubs (hub-as-client). The hub's
  `dial_worker_connection` / `supervise_worker` need a
  `TlsClientConfig` for the outbound dial. The first hub deployment
  (web + native) dials workers over QUIC with the worker's fingerprint
  pinned. There is no "later" — it is on the critical path for the
  first hub and for `alknet-worker`.

  See [ADR-087](../decisions/087-tlsclientconfig-not-blocked-on-dial.md)
  for the full decision, including the circular-hedge analysis, the
  hub-as-client requirement, and the iroh exception.
- **Cross-references**: [ADR-087](../decisions/087-tlsclientconfig-not-blocked-on-dial.md)
  (the decision), [ADR-034](../decisions/034-outgoing-only-x509-and-three-peer-roles.md)
  §3 (verifier selection — the rule `TlsClientConfig::new`
  centralizes), [ADR-084](../decisions/084-aws-lc-rs-crypto-provider.md)
  (provider consistency — enforced by code, not convention, for the
  client side), [ADR-082](../decisions/082-alknet-tls-extraction.md)
  (`TlsServerConfig` — the server-side analogue), OQ-55 (the dial seam
  — remains deferred; this OQ's resolution does not affect it),
  OQ-63 (`TlsError` shape — now covers both server and client variants)