# OQ-37: X.509 Outgoing-Only Case (Three Peer Roles)

- **Origin**: ADR-030 §"Bearer tokens" (the three credential types), the
  discussion that X.509 is fundamentally different from Ed25519
- **Status**: **resolved** (2026-06-28 by ADR-034)
- **Door type**: One-way (how X.509 server identity integrates with the
  peer model)
- **Priority**: medium → resolved
- **Resolution**: **The pre-ADR-034 framing conflated three distinct
  remote roles under "X.509 endpoint."** [ADR-034](decisions/034-outgoing-only-x509-and-three-peer-roles.md)
  names them and resolves the peer-model question:

  1. **Public X.509 endpoint** — a remote HTTPS / `alknet/call`-over-TLS
     server reachable by domain, authenticated by CA verification
     (`WebPkiServerVerifier`). The local node is a *client*; it
     authenticates by bearer token. **Not a `PeerEntry` on the client
     side** — it is not in the call-protocol peer graph (ADR-029), gets
     no `PeerId`, and is not addressable via `PeerRef::Specific`. Ops
     discovered via `from_call`/`from_openapi`/`from_mcp` land in the
     connection's Layer 2 overlay and are invoked through the
     connection handle.
  2. **Transport relay** — iroh's DERP-equivalent (`iroh-relay`).
     Infrastructure, not an alknet peer; no `PeerEntry` / `PeerId`.
     Inherited with the `iroh` feature; its identity is iroh's concern.
  3. **Hub / hosting node** — an alknet application peer (head/worker
     hub, git-hosting hub) that *also* exposes a public domain + X.509
     for browsers. A single `PeerEntry` with **mixed fingerprints**
     (`ed25519:...` + `SHA256:...`), already supported by ADR-030.
     Browsers connecting to it are *not* alknet peers — served by
     `alknet-http`, bearer-token auth, no `PeerId`.

  **The "make `PeerEntry` symmetric" instinct is rejected.** `PeerEntry`
  is for peers in the call-protocol peer graph; pure-client connections
  to public X.509 endpoints are not in that graph on the client side.
  The asymmetry reflects a real trust-model difference: known peers have
  stable logical identities (pin the fingerprint); public APIs don't
  (trust the CA, hold the connection handle directly).

  **Client-side verifier selection rule (extends OQ-29):** known peer
  (`PeerEntry` present) → fingerprint pin (Ed25519 `ed25519:<hex>` or
  X.509 `SHA256:<hex>`); unknown X.509 remote (`PeerEntry` absent) → CA
  verification. An unknown Ed25519 raw-key remote cannot be verified at
  all (no CA fallback) and fails closed — same model as iroh.

  **Downstream, not blocking, recorded so they don't get lost:**
  WebTransport relay-as-proxy (browser → proxy → P2P hub) is the
  remaining scope question tracked as OQ-38 (h3/WebTransport itself is
  now in scope, ADR-038); ADR-030 §6's fingerprint normalization already
  keeps the proxied path clean. On-chain / smart-contract peer
  discovery (relays syncing git repos via iroh gossip) is a *source* of
  `PeerEntry` records, fits the OQ-36 repo/adapter pattern
  (`alknet-peer-store-onchain` implementing `IdentityProvider`), and
  does not change the auth model.

  Not blocking the ADR-029 migration — the Ed25519 path is the primary
  use case and was already resolved; this ADR closes the X.509
  outgoing-only remainder.
- **Cross-references**: ADR-027, ADR-029, ADR-030, ADR-033, ADR-034,
  OQ-29, OQ-36, [client-and-adapters.md](crates/call/client-and-adapters.md),
  [endpoint.md](crates/core/endpoint.md), [auth.md](crates/core/auth.md)
