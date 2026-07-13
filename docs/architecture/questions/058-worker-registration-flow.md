# OQ-58: Worker Registration Flow

- **Origin**: `docs/architecture/crates/hub/README.md` §"Worker
  registration" — the provisioning → enrollment → connection sequence
  that makes TCP+TLS a hard requirement for the hub.
- **Status**: open
- **Door type**: one-way (the registration endpoint shape and the
  enrollment-token model are an API surface; changing them after
  workers are provisioned against them is a breaking change for every
  deployment)
- **Priority**: high
- **Blocked on**: nothing structural — the identity machinery
  (`resolve_from_token`, `PeerEntry.auth_token_hash`, ADR-030/034)
  and the HTTP substrate (`HttpAdapter` on `h2`/`http/1.1` over
  TCP+TLS, ADR-010 Am. 1 / ADR-065) both exist. What remains is
  deciding the token model and the endpoint shape, not building new
  primitives.
- **Resolution**: Not yet decided. The flow is decision-ready in
  shape (see "Decision-ready shape" below). The open sub-questions are:
  1. **Enrollment-token model.** Is the registration token one-time
     single-use (burned on first POST), one-time-per-worker (a worker
     can retry the POST until it succeeds, then the token is
     invalidated), or refresh-capable (the token can enroll multiple
     workers, e.g. a pool token)? One-time-per-worker is the likely
     default (matches the provisioning flow: one token per instance);
     pool tokens are a feature extension.
  2. **Session credential returned.** After registration, does the
     hub return a bearer token for the ongoing channels connection
     (the worker authenticates over TCP+TLS via `auth_token`), or
     does it record the worker's fingerprint and expect fingerprint
     auth on the channels connection (the worker authenticates over
     QUIC via its raw key)? Both are valid; the choice depends on
     whether the worker's transport is known at registration time. A
     hub that accepts both should probably support both return shapes
     (return a token *and* record the fingerprint).
  3. **Endpoint path.** `POST /register`? `POST /v1/workers/register`?
     A versioned path is safer for a one-way door.
  4. **Token source.** Does the hub generate the enrollment token, or
     does the assembly layer (the provisioning system) generate it
     and the hub just validates it? The latter keeps the hub out of
     the token-generation business but requires a shared secret or
     signature scheme.
- **Decision-ready shape**: the registration endpoint is an HTTP POST
  on the hub's `HttpAdapter` (served on `h2`/`http/1.1` over TCP+TLS).
  The request carries the worker's public key and the enrollment
  token. The hub validates the token, creates a `PeerEntry` for the
  worker (fingerprint from the key, `auth_token_hash` from a
  session token the hub issues), and returns the session credential.
  The worker then connects via channels (QUIC or TCP+TLS) and
  authenticates with the fingerprint or the bearer token. The
  `PeerEntry` created at registration is what `resolve_from_token`
  / `resolve_from_fingerprint` matches at connection time.
- **What does NOT block on this**: the hub's multi-transport accept
  loop, the channels-over-TCP path, the identity-over-TCP path
  (bearer token via `resolve_from_token`), and the supervision loop.
  All of these are decided and use existing machinery. OQ-58 is
  about the registration endpoint specifically — the one new surface
  the hub introduces.
- **Cross-references**: ADR-030 (PeerEntry, auth_token_hash),
  ADR-034 (bearer-token identity over non-fingerprint transports),
  ADR-010 Am. 1 (TCP+TLS dispatch via from_stream), ADR-065
  (Connection::from_bidi), ADR-080 (ChannelClient::from_connection),
  OQ-52 (CallConnection::wait_for_close — the supervision loop the
  registration flow feeds into).