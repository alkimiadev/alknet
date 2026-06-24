---
id: core/endpoint-request-client-cert
name: Switch rustls ServerConfig from with_no_client_auth to request-but-don't-require client certs
status: completed
depends_on: [core/endpoint-client-fingerprint]
scope: narrow
risk: medium
impact: component
level: implementation
---

## Description

`core/endpoint-client-fingerprint` landed the extraction logic: when a
client certificate *is* presented, `dispatch_quinn` / `dispatch_iroh`
extract the fingerprint and populate `AuthContext`. However, the server
still builds `rustls::ServerConfig` with `with_no_client_auth()` in all
three `TlsIdentity` branches (`endpoint.rs:477`, `490`, `501`), so the
server never *requests* a client cert. Extraction is a safe no-op until
this task changes the server-side TLS config.

This follow-up switches from `with_no_client_auth()` to a
request-but-don't-require mode so that peers presenting a client cert
(X.509 or RFC 7250 raw Ed25519 key) flow through the extraction path
landed in the predecessor task, while peers without a cert still connect
without regression.

### Design decision: how to request-but-not-require

rustls does not have a direct `with_optional_client_auth()` builder.
The standard approach is:

1. Build the config with `.with_client_auth(verifier)` where `verifier`
   is a custom `ServerCertVerifier` that accepts any presentation (returns
   `Ok(Certified::yes())` when a cert is presented, `Ok(Certified::no())`
   when none is presented — rustls 0.23's `WebPkiServerVerifier` cannot
   be used directly for optional auth).
2. Alternatively, use `rustls::server::WebPkiServerVerifier` with a
   `NoClientAuth` fallback — check the exact rustls API available in the
   pinned version before implementing.

Read the rustls API docs for the pinned version
(`rustls::server::ServerConfig::builder_with_provider`) and confirm the
correct verifier construction. The key property: a peer *may* present a
cert, and if it does, `peer_identity()` returns it; if it doesn't, the
connection still succeeds.

### iroh path

iroh's `Endpoint` builder uses its own TLS session internally. For the
raw-key path (`TlsIdentity::RawKey`), iroh already advertises
`only_raw_public_keys()` via `RawKeyCertResolver` — the server-side half
of RFC 7250. The client-side presentation is set by the client's
`rustls::ClientConfig`, not the server. So the iroh path may already
receive peer identities when the client is an iroh node (the `NodeId` is
always in the TLS cert). Verify: does `Connection::remote_node_id()`
already work for iroh connections today, or does it require the server to
request client certs? If iroh always presents a cert (raw-key mode), no
server-side change is needed for the iroh path — only quinn/X.509 needs
this task. Confirm before implementing.

## Acceptance Criteria

- [ ] `build_rustls_server_config` uses request-but-don't-require client auth (not `with_no_client_auth()`) for at least the X.509 path
- [ ] Peer presenting a client cert: `peer_identity()` returns the cert chain → fingerprint extraction works end-to-end
- [ ] Peer without a client cert: connection still succeeds, `tls_client_fingerprint` is `None` (no regression)
- [ ] iroh path: confirm whether a server-side change is needed; if yes, apply it; if no, document why
- [ ] Integration test: quinn endpoint with a client that presents a cert → `AuthContext.tls_client_fingerprint` is `Some(SHA256:...)`
- [ ] Integration test: quinn endpoint with a client that presents no cert → `AuthContext.tls_client_fingerprint` is `None` and connection succeeds
- [ ] `cargo test -p alknet-core --all-features` succeeds
- [ ] `cargo clippy -p alknet-core --all-features --all-targets` succeeds with no warnings
- [ ] `auth.md` updated: server-config decision documented (request-but-don't-require, not no-client-auth)

## References

- tasks/core/endpoint-client-fingerprint.md — predecessor task (landed extraction, deferred this config change)
- crates/alknet-core/src/endpoint.rs:477, 490, 501 — the three `with_no_client_auth()` sites
- crates/alknet-core/src/endpoint.rs — `extract_quinn_client_fingerprint` / `extract_iroh_client_fingerprint` (already landed, waiting for certs to flow)
- docs/architecture/crates/core/auth.md — fingerprint format table and endpoint-level resolution flow
- docs/architecture/decisions/004-auth-as-shared-core.md — ADR-004 (hybrid resolution)
- docs/architecture/open-questions.md — OQ-12 (TLS identity provisioning)

## Notes

> Split from `core/endpoint-client-fingerprint` per the task's own
> suggestion: extraction is correct either way (returns `None` when no
> cert), so landing it first is a safe no-op. This task is the
> behavioral change that makes fingerprints actually flow. The risk is
> medium because it alters the TLS handshake for every connection —
> ensure the no-cert-peer case has explicit test coverage.