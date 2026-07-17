# OQ-66: `alknet/register` Wire Protocol

- **Origin**: `docs/architecture/decisions/089-alknetclient-native-dial-seam.md`
  §6; `docs/architecture/crates/client/README.md` §"`alknet/register`".
- **Status**: deferred(scope)
- **Door type**: one-way (the wire protocol — once native workers and
  hubs exchange registration frames, the format is compatibility-locked)
- **Priority**: medium
- **Impacts**: Blocks native worker registration over QUIC/TCP+TLS
  without HTTP. A worker that has no HTTP client (a minimal native
  worker, an iroh-only deployment) cannot enroll its key with a hub
  until this is specced and implemented. The HTTP registration
  endpoint (OQ-58) remains the first implementation, so this does NOT
  block the first hub deployment (web + native uses HTTP registration
  for worker provisioning). It blocks the *native-only* registration
  path and the no-HTTP minimal-hub case.
- **Blocked on**: OQ-58's token model (one-time vs. refresh,
  single-use vs. multi-use, rotation). The `alknet/register` wire
  protocol and the HTTP registration endpoint (OQ-58) share the
  enrollment semantics and the token model — they should converge on
  one model before either's wire format is locked. OQ-58 is open
  (resolvable now); this OQ is blocked on its resolution. The
  frame-format fork (reuse `EventEnvelope` vs. standalone) and the
  enrollment-trait shape (point 5) are independently workable but
  not independently decidable — they depend on the token model.
- **What is decided (ADR-089 §6)**: `alknet/register` is a dialable
  ALPN — an **entry point** (ADR-086 §2) accepted without an
  established peer identity. Two registration cases exist, both hub
  concerns and both optional:
  - **Token registration** — a freshly-provisioned worker (docker,
    vast.ai, runpod) generates its local identity, dials the hub on
    `alknet/register`, presents a one-time registration token, and
    enrolls its key. The hub creates a `PeerEntry` (mixed-fingerprint
    shape, ADR-034 §3) and returns a session credential.
  - **No-token (open) registration** — a hub that hosts public
    services over channels, or a relay/gateway, accepts registration
    without a token. The enrollment creates a `PeerEntry` with no
    token requirement.
  The dial is the same as any other ALPN (`AlknetClient::dial_quic` /
  `dial_tcp_tls` on `b"alknet/register"`); the difference is the
  handshake protocol on the resulting `Connection`.
- **What is open**: the **wire protocol** — the handshake on the
  `Connection` after the dial. Specifically:
  1. **Frame format** — does `alknet/register` reuse the call
     protocol's `EventEnvelope` framing (length-prefixed JSON), or
     does it have its own minimal framing? Registration is a one-shot
     request/response (send public key + token, receive session
     credential), not a long-lived call session. Reusing `EventEnvelope`
     ties the register ALPN to `alknet-call` (a dependency); a minimal
     own-format keeps it standalone but adds a second wire format.
  2. **Token model** — one-time vs. refresh, single-use vs. multi-use,
     rotation. This is the same open question as OQ-58 (the HTTP
     registration endpoint's token model). The two paths should share
     the token model — a token issued by the hub works over both HTTP
     and `alknet/register`.
  3. **No-token policy** — who decides whether a hub accepts
     no-token registration? Is it a `DynamicConfig` flag, an
     `IdentityProvider` policy, or a hub-crate config? What `PeerEntry`
     shape does an open-registration enrollment produce (no
     `auth_token_hash`, fingerprint-only)?
  4. **Session credential return** — what does the hub return on
     successful registration? A `PeerEntry`? A session token? Both?
     How does the returned credential feed into the subsequent
      `alknet/channels` connection's `ConnectionCredentials`?
  5. **Relationship to OQ-58** — the HTTP registration endpoint
     (OQ-58) and `alknet/register` share the enrollment semantics
     (create `PeerEntry`, return credential) but differ in transport
     (HTTP vs. raw ALPN). Should they share an enrollment trait in
     `alknet-hub`, with the HTTP endpoint and the `alknet/register`
     handler as two transport-specific front-ends? Or are they
     separate handlers that happen to call the same `IdentityStore`
     write path?
- **Resolution**: Not yet decidable. The token model (point 2) is the
  shared dependency with OQ-58 — both paths should converge on one
  model. The frame format (point 1) is a genuine fork: reusing
  `EventEnvelope` adds an `alknet-call` dependency to the register
  path (which may be fine — the hub already depends on
  `alknet-call`); a standalone format keeps register minimal but
  diverges. The relationship to OQ-58 (point 5) is the shape question
  that determines whether the register handler lives in `alknet-hub`
  (alongside the HTTP endpoint) or in a separate crate. These need a
  dedicated ADR that works through the token model, the frame format,
  and the enrollment-trait shape together — they are not independently
  decidable.
- **Cross-references**: ADR-089 (§6 — names the ALPN and its
  entry-point role; defers the wire protocol to this OQ), OQ-58
  (worker registration flow — the HTTP path; shares the token model),
  ADR-086 (§2 — entry-point vs. endpoint ALPN distinction),
  ADR-034 (§3 — the mixed-fingerprint `PeerEntry` shape token
  registration creates), ADR-072 (channel 0 identity resolution —
  the path the session credential feeds into).