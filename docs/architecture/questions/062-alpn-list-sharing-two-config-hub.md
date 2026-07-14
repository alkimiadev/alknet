# OQ-62: Does a Hub Pass the Same ALPN List to Both `TlsServerConfig`s?

- **Origin**: `docs/architecture/crates/tls/README.md` (the
  "What `AlknetEndpoint` does after the refactor" section describes a
  hub holding two `TlsServerConfig`s — raw key + X.509/ACME — but does
  not state whether each receives the same ALPN list or different
  lists); `docs/architecture/crates/core/endpoint.md` (the ALPN section
  previously stated "both connection sources advertise the same set of
  ALPNs," which is stale under the two-config hub model).
- **Status**: open
- **Door type**: one-way (the ALPN list each `TlsServerConfig` advertises
  is baked into the `rustls::ServerConfig` at construction; changing it
  after the hub is deployed is a config+restart, but the *pattern* —
  same-list vs split-list — sets the assembly-layer wiring shape that
  downstream consumers copy)
- **Priority**: high (the hub is the first two-config consumer; its
  wiring sets the pattern, and an implementer cannot write the hub's
  assembly code without this decided)
- **Resolution**: Not yet decided. The two plausible options:

  **Option A — same list (union) to both configs.** Both
  `TlsServerConfig`s receive `registry.alpn_strings()` verbatim. The
  raw-key QUIC listener advertises `h2`/`http/1.1` (browsers can't
  connect to a raw-key listener anyway, so the advertisement is
  harmless dead negotiation). The X.509 TCP+TLS listener advertises
  `alknet/call` (a native client connecting over TCP+TLS with an X.509
  client cert can use it). Simplest wiring; no split logic; every
  transport can serve every ALPN.

  **Option B — split list, transport-appropriate.** The raw-key config
  gets the native ALPNs (`alknet/call`, `alknet/channels`,
  `alknet/tty`); the X.509/ACME config gets the union including
  `h2`/`http/1.1` (browser ALPNs that only make sense over TCP+TLS with
  a domain cert). The assembly layer filters `registry.alpn_strings()`
  by which transports can serve each ALPN. More logic; cleaner
  advertisement (no browser ALPNs on a raw-key listener).

  The question is whether the "harmless dead negotiation" in Option A
  is acceptable or whether the cleaner advertisement in Option B is
  worth the split logic. This needs a decision before the hub's
  assembly code is written — it is not guessable from the existing
  specs, and guessing produces a wiring shape that downstream consumers
  copy.

  Note: this is distinct from the *iroh* path. Iroh takes its ALPN list
  from `iroh::Endpoint::builder().alpns()` at construction, set by the
  assembly layer from `registry.alpn_strings()`. Iroh uses raw keys
  only, so it gets the native ALPN set regardless of which option is
  chosen for the quinn/TCP+TLS pair.
- **Cross-references**: ADR-082 (`TlsServerConfig::new` takes
  `alpns: &[Vec<u8>]` — the caller decides), ADR-083 (the assembly
  layer builds transports; ALPN-list construction is its job),
  OQ-64 (client-side TLS helper — related but orthogonal; this OQ is
  server-side advertisement)