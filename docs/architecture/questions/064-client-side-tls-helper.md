# OQ-64: Should `alknet-tls` Provide a Client-Side TLS Config Helper?

- **Origin**: `docs/architecture/crates/tls/README.md` (the "Server-only
  for now" section flags this as deferred); ADR-084 (requires the
  client-side `rustls::ClientConfig` to use the same `aws_lc_rs`
  provider — currently enforced by convention, not shared code).
- **Status**: deferred(scope)
- **Door type**: two-way (adding a client helper to `alknet-tls` is
  additive; the risk is not the addition but *extracting it
  prematurely* and baking in a QUIC-shaped client — see Blocking on)
- **Priority**: medium
- **Blocked on**: the `AlknetClient` dial-seam extraction (OQ-55). The
  client-side TLS helper and the shared dial are the same seam: both
  answer "how does an outbound connection build its
  `rustls::ClientConfig` + select a verifier (ADR-034) + dial the
  transport." Extracting the TLS helper without a second transport's
  real client dial existing would bake the QUIC client's shape into a
  shared helper — the same welding ADR-065 unwound on the server side.
  The blocking condition is the same as OQ-55: a second transport's
  real dial (TCP+TLS, SSH raw-TCP, HTTP-wrapped call) existing, so the
  transport-polymorphic client+TLS seam is extractable from two
  *different* transport implementations.
- **Resolution**: Not yet decidable. `alknet-tls` is server-side only
  as specified. The client side — `rustls::ClientConfig` construction +
  ADR-034 verifier selection (fingerprint pin for known peers, CA-verify
  for unknown X.509, fail-closed for unknown raw-key) — lives in
  `alknet-call`'s `FingerprintPinVerifier` today. The
  provider-consistency requirement (ADR-084: `aws_lc_rs` on all paths)
  is enforced by convention (two crates independently constructing
  `aws_lc_rs::default_provider()`) until this OQ is resolved.

  What does NOT block on this: each client (`CallClient`,
  `ChannelClient`) building its own `ClientConfig` standalone with the
  matching provider. The friction is duplicated boilerplate (each
  client rebuilds verifier selection + provider wiring), not a missing
  capability and not a QUIC-welded client API. The
  transport-agnostic take-over (`CallClient::spawn_dispatch`,
  `ChannelClient::from_connection` — ADR-080) is decided and is not
  the thing being deferred; only the shared *client TLS config helper*
  is.

  Note on the hub-as-client case: a hub (A) that dials another hub (B)
  uses a client to do so — from B's perspective A is a worker. The
  bidirectionality of the call and channels protocols means both sides
  can be both hub and worker within a connection. This does not change
  the blocking condition: the shared client TLS helper is still about
  the *dial*, regardless of whether the dialer is a hub, a worker, or a
  hub-worker. The hub-as-client case is a use case that the resolved
  helper must cover, not a reason to resolve it now.
- **Cross-references**: OQ-55 (the `AlknetClient` dial-seam extraction
  — this OQ's blocking condition), ADR-034 (verifier selection — the
  rule the helper would centralize), ADR-084 (provider consistency —
  the convention that holds until this is resolved), ADR-065 (the
  server-side transport generalization this OQ's deferral avoids
  preempting on the client side)