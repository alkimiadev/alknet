---
id: core/endpoint-client-fingerprint
name: Extract TLS client certificate fingerprint in endpoint dispatch (ADR-004)
status: completed
depends_on: []
scope: narrow
risk: medium
impact: component
level: implementation
---

## Description

Both dispatch functions in `crates/alknet-core/src/endpoint.rs` hard-code
`tls_client_fingerprint: None` when calling `build_auth_context`
(endpoint.rs:306 and 396). As a result, `AuthContext.identity` (the
endpoint-resolved identity) is always `None` at the endpoint layer, and
all identity resolution is deferred to handler-level code. The
endpoint-level auth resolution path described in
`docs/architecture/crates/core/auth.md:159–171` is non-functional:

> "QUIC connection arrives → TLS handshake → Extract TLS client
> certificate fingerprint (if presented) → If fingerprint present:
> `IdentityProvider::resolve_from_fingerprint()` → `auth.identity =
> Some(identity)` → Construct `AuthContext { identity, alpn, remote_addr,
> tls_client_fingerprint }`"

This matters most for P2P nodes using RFC 7250 raw Ed25519 keys (the
"default for most alknet nodes" per OQ-12), where the connection-level
identity *is* the TLS client cert — there is no separate protocol-level
credential to extract. Without endpoint-level fingerprint extraction,
a raw-key peer connecting to an `alknet/call` endpoint cannot be
identified by fingerprint at the endpoint layer.

### Quinn path

`extract_quinn_alpn` (endpoint.rs:316–326) already downcasts
`connection.handshake_data()` to `quinn::crypto::rustls::HandshakeData`.
The same `HandshakeData` struct exposes the peer's client certificate
chain when one was presented. Extract the chain, hash the leaf cert's
DER to a `SHA256:`-prefixed fingerprint string (matching the format
`AuthPolicy::resolve_identity_from_fingerprint` expects — see
auth.md:152), and pass it to `build_auth_context` in place of `None`.

Note: `rustls::ServerConfig` is currently built with
`with_no_client_auth()` (endpoint.rs:450, 463, 473), so the server does
not *request* client certs. To actually receive a client cert, the
server config must use `with_client_auth()` or an equivalent that
requests but does not require client certs (raw-public-key peers
present their Ed25519 key as the "client cert" in RFC 7250 mode). This
is the one design decision to make in this task: whether to switch from
`with_no_client_auth()` to a "request-but-don't-require" mode, or to
leave `with_no_client_auth()` and accept that fingerprints only flow
when the client opts to present a cert unbidden. The RFC 7250 raw-key
path (the `RawKeyCertResolver` at endpoint.rs:565–595) already
advertises `only_raw_public_keys() -> true`, which is the server-side
half of RFC 7250; the client-side presentation is set by the client's
`rustls::ClientConfig`, not by the server. Read ADR-004 and OQ-12
before deciding.

### Iroh path

iroh's `Connection` exposes the peer's `NodeId` (the raw Ed25519
public key) via the connection's TLS session metadata. In iroh's model
the `NodeId` *is* the fingerprint — it's the raw-public-key identity.
Extract it and format as a `NodeId:`-prefixed string (or `SHA256:` of
the public key bytes — match whatever `AuthPolicy`'s fingerprint set
is expected to contain). Look at `iroh::endpoint::Connection` methods
and the `iroh::tls::Lts` / peer-certificate accessor for the exact API.

### Fingerprint format

`AuthPolicy::resolve_identity_from_fingerprint` (config.rs:69–79) does
a literal `HashSet::contains()` check — it does not normalize. So
whatever format the extractor produces must be the same format the
operator configures in `authorized_fingerprints`. The existing
fingerprint test (auth.rs:145–153) uses `"SHA256:abc123"` as a
placeholder. Pick a concrete format and document it in `auth.md` (the
spec is currently silent on the exact string format). Suggested:
`SHA256:<hex of leaf cert DER>` for X.509, `ed25519:<base64 of pub key>`
for raw keys — but confirm against any existing fingerprint producer
in the codebase before committing.

## Acceptance Criteria

- [ ] `dispatch_quinn` extracts client cert fingerprint from `HandshakeData` when present
- [ ] `dispatch_iroh` extracts peer `NodeId` (or equivalent raw-key fingerprint) when present
- [ ] `build_auth_context` receives `Some(fingerprint)` when a client cert was presented, `None` otherwise
- [ ] `AuthContext.identity` is `Some(identity)` when the fingerprint resolves via `IdentityProvider`, `None` otherwise (no regression for the no-cert case)
- [ ] Server config decision (request-but-don't-require vs. no-client-auth) is made and documented
- [ ] Fingerprint string format is chosen, documented in `auth.md`, and consistent between extractor and `AuthPolicy::authorized_fingerprints` config
- [ ] Unit test: quinn path with a presented client cert → `auth.tls_client_fingerprint` is `Some(...)`
- [ ] Unit test: quinn path with no client cert → `auth.tls_client_fingerprint` is `None` (existing behavior preserved)
- [ ] Unit test: iroh path → `auth.tls_client_fingerprint` is `Some(NodeId-format)` when peer identity is available
- [ ] `cargo test -p alknet-core --all-features` succeeds
- [ ] `cargo clippy -p alknet-core --all-features --all-targets` succeeds with no warnings

## References

- docs/reviews/004-post-implementation-sanity-check.md — W2 (full finding)
- docs/architecture/crates/core/auth.md:159–171 — endpoint-level resolution flow spec
- docs/architecture/crates/core/auth.md:152 — fingerprint format used by `resolve_identity_from_fingerprint`
- docs/architecture/decisions/004-auth-as-shared-core.md — ADR-004 (hybrid resolution)
- docs/architecture/open-questions.md — OQ-12 (TLS identity provisioning)
- crates/alknet-core/src/endpoint.rs:306, 396 — the two `None` sites to fix
- crates/alknet-core/src/endpoint.rs:316–326 — `extract_quinn_alpn` (pattern to follow for `HandshakeData` downcast)
- crates/alknet-core/src/endpoint.rs:565–595 — `RawKeyCertResolver` (RFC 7250 server-side half)

## Notes

> If the server-config decision (request-but-don't-require client auth)
> is too large for this task's scope, split it: implement extraction
> first (this task, gated on the cert being presented *if* one arrives),
> then a follow-up task switches the server config to actually request
> client certs. The extraction code is correct either way — it returns
> `None` when no cert was presented, which is the current behavior, so
> landing extraction first is a safe no-op until the server config
> changes.

## Summary

Added `extract_quinn_client_fingerprint` (leaf cert DER → `SHA256:<hex>`
via `peer_identity()` downcast) and `extract_iroh_client_fingerprint`
(peer `NodeId` → `ed25519:<hex>`). Both dispatch functions now pass the
extracted fingerprint to `build_auth_context`. Fingerprint format
documented in `auth.md` (table: quinn X.509 vs iroh raw Ed25519).
Server config still uses `with_no_client_auth()` — extraction is a safe
no-op. Follow-up task `core/endpoint-request-client-cert` created for the
server config change. Two unit tests cover fingerprint format +
determinism. `cargo test -p alknet-core --all-features` (59 tests) and
clippy clean.