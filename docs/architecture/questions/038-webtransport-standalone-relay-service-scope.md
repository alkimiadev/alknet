# OQ-38: WebTransport Standalone Relay Service Scope

- **Origin**: [ADR-034](decisions/034-outgoing-only-x509-and-three-peer-roles.md)
  §5, [webtransport.md](crates/http/webtransport.md)
- **Status**: open (scope, not deferral)
- **Door type**: One-way (crate boundary), two-way (mechanism)
- **Priority**: low
- **Resolution**: There are two distinct "WebTransport proxy" concepts
  that must not be conflated:

  1. **In-process ALPN-stream-proxy (resolved, in `alknet-http`).**
     The `h3` handler hands a WebTransport stream to another ALPN
     handler (`SshAdapter`, `GitAdapter`, etc.) as a `Connection`, so
     a browser with a WASM parser can reach any ALPN service via
     WebTransport. This is resolved by
     [ADR-040](decisions/040-webtransport-alpn-stream-proxy.md) and
     lives in `alknet-http`'s `h3` handler. Not this OQ.

  2. **Standalone relay service (this OQ).** A full relay — a fork of
     `iroh-relay` — that provides NAT traversal infrastructure with
     WebTransport-based proxy as a fallback alongside WebSocket. This
     is a separate service, not a mode of the `h3` handler: it
     terminates the browser's WebTransport connection and forwards
     encrypted traffic to a P2P hub's Ed25519 endpoint (so the hub need
     not expose its own public X.509 cert). ADR-034 §5 recorded it in
     the h3/WebTransport bucket; ADR-038 brought h3/WebTransport into
     scope (later superseded by [ADR-044](decisions/044-defer-webtransport-browsers-use-websocket.md),
     which deferred h3/WebTransport as a scope decision — the browser
     bidirectional path uses WebSocket); ADR-040 resolved the in-process
     proxy (now parked per ADR-044). This OQ is the remaining scope
     question: does the standalone relay live in a future `alknet-relay`
     crate (a fork of `iroh-relay` with WebTransport proxy fallback) or
     is it out of scope for the current alknet work?

  This is a genuine scope question, not a deferral. The relay use case
  is not yet concrete enough to commit the crate boundary — no
  deployment has asked for a standalone relay with WebTransport
  fallback yet, and the design (transport-only proxy, no auth-model
  change per ADR-034 §5) is clear but the home is not. The decision is
  made when the browser-to-P2P-peer relay use case becomes concrete;
  until then it is tracked here, not deferred with "v1/later" language.
  The relay does not change the auth model (bearer token +
  `PeerEntry.auth_token_hash`; relay is transport-only), so it does not
  block any other ADR.
- **Cross-references**: ADR-027, ADR-030, ADR-034, ADR-038 (superseded),
  ADR-040 (parked), ADR-044, [webtransport.md](crates/http/webtransport.md)
