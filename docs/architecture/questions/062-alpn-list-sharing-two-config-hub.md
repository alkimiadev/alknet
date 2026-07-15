# OQ-62: Does a Hub Pass the Same ALPN List to Both `TlsServerConfig`s?

- **Origin**: `docs/architecture/crates/tls/README.md` (the
  "What `AlknetEndpoint` does after the refactor" section describes a
  hub holding two `TlsServerConfig`s — raw key + X.509/ACME — but does
  not state whether each receives the same ALPN list or different
  lists); `docs/architecture/crates/core/endpoint.md` (the ALPN section
  previously stated "both connection sources advertise the same set of
  ALPNs," which is stale under the two-config hub model).
- **Status**: resolved
- **Door type**: one-way (the ALPN list each `TlsServerConfig` advertises
  is baked into the `rustls::ServerConfig` at construction; changing it
  after the hub is deployed is a config+restart, but the *pattern* —
  same-list vs split-list — sets the assembly-layer wiring shape that
  downstream consumers copy)
- **Priority**: high (the hub is the first two-config consumer; its
  wiring sets the pattern, and an implementer cannot write the hub's
  assembly code without this decided)
- **Resolution**: **Split list, by endpoint type.** Each
  `TlsServerConfig` advertises only the ALPNs its endpoint type's
  client class can negotiate. The hub composes a subset of three
  endpoint types (web, native, iroh), each with its own identity model,
  auth model, and transport(s). The assembly layer filters
  `registry.alpn_strings()` per `TlsServerConfig` by endpoint type:

  - **raw-key config (native endpoint)**: `alknet/channels`,
    `alknet/call`, `alknet/ssh` (future) — the native ALPNs. No
    `h2`/`http/1.1` (browsers cannot connect to a raw-key listener;
    native clients do not negotiate HTTP ALPNs).
  - **X.509/ACME config (web endpoint)**: `h2`, `http/1.1`,
    `alknet/channels` (for WebSocket-carrying-channels, per OQ-65),
    `acme-tls/1` (appended automatically). No `alknet/call` as a
    top-level ALPN unless the deployment explicitly serves
    call-over-TCP on the web endpoint.
  - **iroh builder (iroh endpoint)**: `alknet/channels`,
    `alknet/call` — iroh uses raw keys only; native ALPNs.

  The rationale: each endpoint type serves a different client class.
  Advertising ALPNs the client class cannot negotiate (e.g., `h2` on a
  raw-key listener) is harmless but misleading — it implies the
  listener serves a client class it cannot. The split makes the
  advertisement honest and the assembly-layer wiring pattern guessable.

  The naming that makes this answerable is the **entry-point vs.
  endpoint** distinction (ADR-086 §2): entry-point ALPNs (`h2`,
  `http/1.1`, future `alknet/register`) are accepted without
  established peer identity (per-request auth); endpoint ALPNs
  (`alknet/channels`, `alknet/call`, `alknet/ssh`) require identity
  resolution. The web config advertises entry-point ALPNs (for
  registration, browsers) + `alknet/channels` (for
  WebSocket-channels); the native config advertises endpoint ALPNs
  (for native clients); iroh advertises endpoint ALPNs (for p2p
  peers).

  See [ADR-086](../decisions/086-endpoint-types-and-entry-points.md)
  for the full decision, including the three-endpoint-type model, the
  hub-shape table (full / web+native / native+iroh / minimal), and the
  foundational-handler categorization (channels data-channel ALPNs vs.
  SSH as an endpoint ALPN that wraps channels).
- **Cross-references**: [ADR-086](../decisions/086-endpoint-types-and-entry-points.md)
  (the decision), ADR-082 (`TlsServerConfig::new` takes
  `alpns: &[Vec<u8>]` — the caller decides; this OQ specified how),
  ADR-083 (the assembly layer builds transports; ALPN-list construction
  is its job), OQ-65 (WebSocket carrying channels — why the web config
  advertises `alknet/channels`), OQ-64 (client-side TLS helper —
  related but orthogonal; this OQ is server-side advertisement)