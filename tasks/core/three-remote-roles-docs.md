---
id: core/three-remote-roles-docs
name: Document the three remote roles and client-side verifier selection rule (ADR-034)
status: completed
depends_on: [core/peer-entry-model]
scope: single
risk: trivial
impact: isolated
level: implementation
---

## Description

Update the in-code comments and doc comments in `alknet-core/src/auth.rs` and
`alknet-core/src/endpoint.rs` to document the three remote roles (ADR-034) and
the client-side verifier selection rule. This is a documentation/comment task —
the server-side endpoint code is unchanged; the client-side verifier selection
is a call-side task (`call/call-client-verifier-selection`).

### Three remote roles (ADR-034 §1)

| Role | Identity | alknet peer? | `PeerEntry` on local side? |
|------|----------|--------------|----------------------------|
| **Public X.509 endpoint** | Domain + CA-issued X.509 | No (local node is a client) | No |
| **Transport relay** (iroh's DERP-equivalent) | iroh `NodeId` (Ed25519) | No (infrastructure) | No |
| **Hub / hosting node** | Ed25519 raw key **and/or** X.509 | Yes (full peer) | Yes |

`PeerEntry` (and the `PeerId` it resolves to) is the model for peers in the
call-protocol peer graph (ADR-029). A pure-client connection to a public X.509
endpoint is **not** in that graph on the client side: no `PeerEntry`, no
`PeerId`, no `PeerRef::Specific` routing.

### Client-side verifier selection rule (ADR-034 §3)

| Local has `PeerEntry` for remote? | Remote cert type | Client verifier |
|----------------------------------|------------------|-----------------|
| No (public X.509 endpoint) | X.509 | `WebPkiServerVerifier` (CA verification) |
| No | Ed25519 raw key | fails closed (no CA to fall back to) |
| Yes (hub, Ed25519 path) | Ed25519 raw key | fingerprint match (`ed25519:<hex>`) |
| Yes (hub, X.509 path) | X.509 | fingerprint match (`SHA256:<hex>`) |

### What to update

1. **`auth.rs` doc comments**: add the three-roles table and the verifier
   selection rule to the `Identity` / `PeerEntry` section doc comments,
   referencing ADR-034. The `auth.md` spec already has this; mirror it in the
   source comments.

2. **`endpoint.rs` doc comments**: clarify that the server-side
   `AcceptAnyCertVerifier` is "request-but-don't-require" mode for fingerprint
   extraction (unchanged), and that the **client-side** verifier selection is
   by `PeerEntry` presence (ADR-034 §3) — note that this is a `CallClient`
   concern, not an endpoint concern.

3. **No code changes** — this is comments/docs only. The server-side endpoint
   is unchanged by ADR-034. The client-side verifier is
   `call/call-client-verifier-selection`.

## Acceptance Criteria

- [ ] `auth.rs` doc comments document the three remote roles (ADR-034 §1)
- [ ] `auth.rs` doc comments document the client-side verifier selection rule (ADR-034 §3)
- [ ] `endpoint.rs` doc comments clarify server-side vs client-side verifier concerns
- [ ] Comments reference ADR-034 and `auth.md`
- [ ] No code changes (comments only)
- [ ] `cargo test -p alknet-core` succeeds (no regressions from comment changes)
- [ ] `cargo clippy -p alknet-core` succeeds with no warnings

## References

- docs/architecture/crates/core/auth.md — Three Remote Roles, Client-side verifier selection
- docs/architecture/decisions/034-outgoing-only-x509-and-three-peer-roles.md — ADR-034

## Notes

> Documentation-only task to ensure the three-roles model and verifier selection
> rule are visible in the source, not just the specs. The server-side endpoint
> is unchanged by ADR-034; the client-side verifier selection is implemented in
> `call/call-client-verifier-selection`. Folding this into a standalone task
> keeps the fingerprint-normalization and resolution-logic tasks focused on
> code, not prose.

## Summary

Added doc comments to alknet-core/src/auth.rs (three remote roles table + client-side verifier selection rule from ADR-034 §1/§3, referencing auth.md and ADR-034) and alknet-core/src/endpoint.rs (clarified server-side AcceptAnyCertVerifier is request-but-don't-require fingerprint extraction, and that client-side verifier selection is a CallClient concern by PeerEntry presence per ADR-034 §3). Comments-only, no code changes. cargo build/clippy/test all clean; rustfmt clean on touched files.