# ADR-034: Outgoing-Only X.509 and the Three Peer Roles

## Status

Accepted (resolves OQ-37)

## Context

OQ-37 framed the open question as: "the three credential types (Ed25519,
X.509, bearer token) and how X.509 server identity fits the peer model."
During resolution, it became clear that **three distinct remote roles**
had been conflated under the single label "X.509 endpoint," and that the
conflation was the actual source of the confusion — not the TLS
mechanics, which ADR-027 and ADR-030 had already settled.

The three roles are real and structurally different:

1. **Public X.509 endpoint** — a remote HTTPS or `alknet/call`-over-TLS
   server reachable by domain name, authenticated by a CA-issued X.509
   cert. The local alknet node is a *client* of it. Examples: a
   third-party API (`vast.ai`, `api.openai.com`), a public alknet hub
   that the local node dials over the open internet, an `alknet/call`
   peer that has chosen to expose a domain + X.509 instead of (or in
   addition to) an Ed25519 raw key. The client authenticates to the
   server by **bearer token** (browsers and most HTTP clients cannot do
   TLS client-auth); the server authenticates to the client by **CA
   verification** (WebPKI), not by fingerprint pinning.

2. **Transport relay** — iroh's DERP-equivalent (`iroh-relay`). A
   connectivity-assistance node that forwards encrypted datagrams
   between peers who cannot directly connect (NAT traversal). It is
   *infrastructure*, not an alknet application peer: it does not
   register operations, does not participate in the call protocol's
   peer graph, and has no `PeerEntry` / `PeerId` in alknet's auth
   model. Alknet inherits it for free when the `iroh` feature is on; the
   relay's own identity (an Ed25519 `NodeId`) is iroh's concern, not
   alknet's.

3. **Hub / hosting node** — an alknet application peer that acts as a
   hub in a hub-and-spoke (head/worker) topology. It is an ordinary
   `PeerEntry` that *happens* to also expose a public domain + X.509
   (so browsers / external HTTPS clients can reach it) *and* an Ed25519
   identity (so other alknet nodes can reach it P2P via iroh or direct
   quinn). The git-hosting-relay-with-gossip-sync use case is this role:
   the hub is a full alknet peer that additionally serves browsers.

The pre-ADR-034 framing asked whether `PeerEntry` should be made
**symmetric** — i.e., whether the local node should hold a `PeerEntry`
for *every* remote it might dial, including pure-public-API servers it
has no P2P relationship with. This ADR answers **no**: the asymmetry is
correct and reflects a real difference in trust model. `PeerEntry` (and
the `PeerId` it produces) is the model for **peers in the call-protocol
peer graph** (ADR-029) — peers that get a stable logical identity, are
addressable via `PeerRef::Specific`, and whose ops land in the
peer-keyed overlay. A pure-client connection to a public HTTPS API is
not that.

This distinction matters because forcing a stable logical `peer_id`
onto "the operator of `api.example.com`" is wrong: a public domain's
operator can change hands, the cert can be reissued, and the local node
has no stable logical identity to attach — only "domain X verified by
CA Y today." That is a different trust model from "this Ed25519 key is
`worker-a`, and key rotation updates the fingerprint but not the
identity" (ADR-030).

## Decision

### 1. Name the three roles; stop using "relay" ambiguously

The architecture documents use three distinct terms:

| Role | Identity | Transport | alknet peer? | Example |
|------|----------|-----------|--------------|---------|
| **Public X.509 endpoint** | Domain + CA-issued X.509 | HTTPS / `alknet/call`-over-TLS | No (client only, unless also role 3) | `api.alk.dev`, `vast.ai` |
| **Transport relay** | iroh `NodeId` (Ed25519) | iroh's DERP-like protocol | No (infrastructure) | `relay.iroh.network` |
| **Hub / hosting node** | Ed25519 raw key **and/or** X.509 | iroh / direct quinn / HTTPS | Yes (full `PeerEntry`) | git-hosting hub, head node |

Existing specs that say "relay" when they mean "domain-hosted service"
or "hub" are amended by reference to this table. ADR-027's "domain-
hosted services" and ADR-030's "X.509 cert" credential path refer to
the **public X.509 endpoint** role and the **hub** role; iroh's
transport relay is a separate, inherited component referenced only in
the iroh transport path.

### 2. Outgoing-only X.509 is not a `PeerEntry` on the client side

When a `CallClient` (or `from_openapi` / `from_mcp`) dials a remote
that is a **public X.509 endpoint** and the local node has no P2P
relationship with it (no `PeerEntry` for the remote):

- The server is authenticated by **CA verification**
  (`rustls::WebPkiServerVerifier` with the platform root store or a
  configured CA bundle). There is no fingerprint to pin — pinning a
  `SHA256:<hex of DER>` fingerprint against an external CA-issued cert
  is brittle (cert renewal changes the fingerprint) and is not the
  WebPKI trust model. The trigger for CA verification is **the absence
  of a `PeerEntry` for the remote combined with an X.509 transport**;
  the verifier selection rule is stated in full in §3 below. The
  `CallCredentials.remote_identity: Option<RemoteIdentity>` field
  (ADR-017 §7) carries an expected fingerprint/cert when the caller has
  one to pin (`Some`); for a pure-client X.509 dial with no
  `PeerEntry`, `remote_identity` is `None` and the CA path applies. The
  `Option` is load-bearing — `None` is the public-X.509-endpoint state,
  not a missing field: an implementer must not default it to a
  placeholder, and must not treat `None` as "skip verification" (`None`
  + X.509 = CA verification; `None` + Ed25519 raw key = fail closed).
  (ADR-017 §7 specified `remote_identity` as "expected fingerprint or
  cert"; this ADR extends its semantics so that `remote_identity: None`
  + no `PeerEntry` + X.509 transport selects CA verification, and
  `remote_identity: None` + Ed25519 raw-key transport fails closed.)
- The client authenticates to the server by **bearer token**
  (`CallCredentials.auth_token`), carried in the call-protocol
  `auth_token` payload field (or the HTTP `Authorization` header for
  `from_openapi` / `from_mcp`). What the *server* does with that token
  depends on which kind of public X.509 endpoint it is:
  - **Third-party API** (`api.openai.com`, `vast.ai` — not an alknet
    node): the server applies its own auth scheme (its own API-key
    validation, its own ACL). Alknet's `PeerEntry` / `ApiKeyEntry` types
    do not apply on the far side; the alknet client just carries the
    token in the shape the remote expects (an HTTP header, a
    call-protocol `auth_token` payload) and treats the remote's
    response as authoritative.
  - **Alknet hub reached over its public X.509 path** (a role-3 hub
    dialed over the domain instead of P2P): the hub resolves the
    client's token via its own `PeerEntry.auth_token_hash` or
    `ApiKeyEntry` — the *server's* bookkeeping, not the client's. The
    client still holds no `PeerEntry` for the hub on its own side
    unless it also has a P2P trust relationship with that hub (in which
    case the §3 mixed-fingerprint path applies, not this one).
- The client may still present its TLS client cert (Ed25519 raw public
  key, per OQ-29) when one is configured; bearer token is the
  *authorization* credential, and TLS client-auth (when presented) is
  *additional* identity material the server may use. For a third-party
  API the cert is ignored; for an alknet hub it may be extracted as a
  fingerprint. Presenting or omitting the client cert is the caller's
  choice via `CallCredentials`; this ADR does not require disabling
  client-auth on this path.
- The connection does **not** get a `PeerId` on the client side. It is
  not added to `PeerCompositeEnv` (ADR-029). There is no
  `PeerRef::Specific` routing to it. The connection is a live
  `CallConnection` (or HTTP client session) the caller holds directly;
  ops discovered via `from_call` / `from_openapi` / `from_mcp` land in
  that connection's Layer 2 overlay (ADR-024) and are invoked through
  the connection handle, not through the peer-keyed routing layer.

This is the **asymmetry** OQ-37 worried about, stated as a deliberate
design property: `PeerEntry` is for peers in the call-protocol peer
graph. Pure-client connections to public X.509 endpoints are not in
that graph on the client side. The server may have a `PeerEntry` for
*us* (resolving our bearer token, in the alknet-hub sub-case); we
don't need one for *it*.

### 3. The hub case is already covered by ADR-030's mixed-fingerprint `PeerEntry`

A **hub / hosting node** that is reachable both P2P (Ed25519 raw key
via iroh or direct quinn) and via a public domain (X.509 for browsers)
is a single `PeerEntry` with mixed fingerprints:

```rust
PeerEntry {
    peer_id: "hub-a".into(),
    fingerprints: vec![
        "ed25519:<hex of hub's Ed25519 pub key>",   // P2P path
        "SHA256:<hex of hub's X.509 cert DER>",      // WebTransport / HTTPS path
    ],
    auth_token_hash: Some("<sha256 of peer's bearer token>"),
    scopes: vec![...],
    resources: {...},
    ...
}
```

When an alknet node dials this hub P2P, the Ed25519 fingerprint
matches; when it dials over the public X.509 path (e.g., because P2P
connectivity failed), the X.509 fingerprint matches — both resolve to
the same `peer_id` (`"hub-a"`). The X.509 path here uses
**fingerprint pinning** (the `SHA256:<hex>` is in `PeerEntry`), *not*
CA verification, because the local node has a prior P2P trust
relationship with this specific hub and has recorded its cert's
fingerprint. This is the one case where X.509 fingerprint pinning is
correct: the peer is a known alknet peer, not an arbitrary public API.

The choice between **CA verification** (role 1) and **fingerprint
pinning** (role 3, X.509 path) is driven by whether the local node has
a `PeerEntry` for the remote — this is the authoritative verifier
selection rule, referenced from §2:

| Local has `PeerEntry` for remote? | Remote cert type | Client verifier |
|----------------------------------|------------------|-----------------|
| No (public X.509 endpoint) | X.509 | `WebPkiServerVerifier` (CA verification) |
| No | Ed25519 raw key | fails closed (no CA to fall back to — raw-key remotes are always known peers; fingerprint IS identity) |
| Yes (hub, Ed25519 path) | Ed25519 raw key | fingerprint match (`ed25519:<hex>`) |
| Yes (hub, X.509 path) | X.509 | fingerprint match (`SHA256:<hex>`) |

This is the key-type-aware verifier from OQ-29, with the *peer-model*
criterion made explicit: the verifier choice is determined by whether
the remote is a known peer (`PeerEntry` present → pin) or an external
server (`PeerEntry` absent → CA, or fail closed for raw keys).

### 4. Browsers connecting to a hub are not alknet peers

A browser reaching a hub over WebTransport (or HTTPS) is served by the
hub's `alknet-http` handler. The browser authenticates by **bearer
token** (HTTP `Authorization`), resolved by the hub's
`IdentityProvider::resolve_from_token` against the hub's
`PeerEntry.auth_token_hash` or `ApiKeyEntry`. The browser is **not** an
alknet peer on the hub's side either — it does not get a `PeerId`, does
not enter `PeerCompositeEnv`, and its "ops" are HTTP routes / WebTransport
streams served by `alknet-http`, not entries in the call-protocol
peer-keyed overlay. The hub's `PeerEntry` for the browser (if any) is
about authorizing the bearer token, not about peer-graph membership.

This keeps the peer graph populated only by full alknet nodes (role 3
hubs and role-3-style spoke nodes), never by browsers or pure HTTP
clients.

### 5. WebTransport relay-as-proxy is deferred with h3 / WebTransport

A **WebTransport proxy** that terminates the browser's WebTransport
connection and proxies encrypted traffic to a hub's P2P endpoint
(avoiding the need for the hub itself to expose a public X.509 endpoint)
is a real feature, especially for the browser-to-P2P-peer case. It is
**not** load-bearing on the auth model resolved here:

- The proxy does not change how identities resolve. The browser still
  authenticates by bearer token; the hub still resolves it via
  `PeerEntry.auth_token_hash`. The proxy is transport-only.
- The fingerprint normalization committed in ADR-030 §6
  (`ed25519:<hex>` for raw keys across quinn and iroh) was already
  designed to keep the proxied path clean: a proxied connection's
  Ed25519 identity is the same `ed25519:<hex>` whether the client
  connected directly or through the proxy.

WebTransport support is already deferred past v1 in the alknet-http
Phase 0 findings (decision point DH-2, "h3/WebTransport — in v1 or
deferred?"). The WebTransport-relay-as-proxy feature
belongs in that same deferral bucket — it lands when `h3` /
WebTransport lands, and it does not require any change to the auth
model in this ADR. It is recorded here so it is not lost; it is not an
open question for the auth model.

### 6. On-chain / smart-contract peer discovery fits the OQ-36 adapter pattern

The downstream use case — storing relay/repo info and org/user ACL on a
smart-contract platform, with relays (hubs) syncing git repos via
iroh's gossip protocol — is a **discovery and ACL-source** concern, not
an auth-model concern. It does not change any of decisions 1–4:

- The hubs are role-3 `PeerEntry` peers (mixed fingerprints, full peer-
  graph membership, gossip-synced).
- The smart contract is a **source of `PeerEntry` records**. It maps
  cleanly onto the repo/adapter pattern (ADR-033): a future
  `alknet-peer-store-onchain` adapter implementing `IdentityProvider`
  against a smart contract is additive, exactly like
  `alknet-peer-store-sqlite`. The auth model (`PeerEntry`, `PeerId`,
  `Identity`) is unchanged; only the *source* of the records changes.
- The repo/ACL data on-chain is consumed by the hub's authorization
  layer (`AccessControl::check` against scopes/resources populated from
  the on-chain `PeerEntry`), not by the TLS / fingerprint path.

Designing that adapter now would be premature — it is downstream of
both the repo/adapter exploration (OQ-36) and the git crate (OQ-10).
It is noted here only to confirm it does not reopen OQ-37.

## What this does NOT change

- **`PeerEntry` struct shape** (ADR-030) — unchanged. Mixed
  fingerprints (Ed25519 + X.509) were already supported.
- **`Identity` / `IdentityProvider` trait** — unchanged. The verifier
  choice is a `CallClient` / `from_openapi` / `from_mcp` concern, not
  an `IdentityProvider` concern.
- **`CallCredentials` struct** — unchanged. `remote_identity` already
  carries the expected key type (OQ-29); this ADR specifies how the
  verifier is chosen from it (CA for unknown X.509 remotes, fingerprint
  match for known peers).
- **`PeerCompositeEnv` / `PeerRef`** (ADR-029) — unchanged. Pure-client
  X.509 connections simply do not enter the peer-keyed overlay.
- **`TlsIdentity`** (ADR-027) — unchanged. The server-side X.509 / ACME
  / RawKey modes are unaffected; this ADR is about the *client-side*
  verifier choice for outgoing connections.
- **The no-env-vars invariant** — unaffected. The bearer token for the
  outgoing X.509 case still comes from `Capabilities`, not env vars.

## Consequences

**Positive:**
- OQ-37 is resolved. The "make `PeerEntry` symmetric" instinct is
  rejected with a clear criterion: `PeerEntry` is for peers in the
  call-protocol peer graph; pure-client connections to public X.509
  endpoints are not in that graph on the client side.
- The three remote roles are named, so future specs and conversations
  can distinguish "public X.509 endpoint," "transport relay," and
  "hub / hosting node" instead of overloading "relay."
- The client-side verifier choice has a single rule: known peer
  (`PeerEntry` present) → fingerprint pin; unknown X.509 remote
  (`PeerEntry` absent) → CA verification. This closes the
  `AcceptAnyServerCertVerifier` security hole for X.509 that OQ-29
  flagged, with the peer-model criterion made explicit.
- The hub case (mixed Ed25519 + X.509 fingerprints, browser access via
  WebTransport/HTTPS) is confirmed to need no new types — ADR-030's
  `fingerprints: Vec<String>` already covers it.
- The WebTransport-relay-as-proxy and on-chain-discovery use cases are
  recorded with clear homes (h3/WebTransport deferral bucket; OQ-36
  adapter pattern) so they don't get lost and don't reopen the auth
  model.

**Negative:**
- The `alknet-http` and `alknet-call` client paths must branch on
  "is this remote a known `PeerEntry`?" when selecting a
  `ServerCertVerifier`. This is a small implementation cost and is
  local to the client connection-establishment code; it is not a
  structural change.
- Operators must understand the distinction between "I have a
  `PeerEntry` for this remote (pin its fingerprint)" and "I'm calling a
  public API (trust the CA)." In practice this is intuitive (it's the
  difference between `~/.ssh/known_hosts` and a browser's CA trust
  store), but the docs must state it clearly, which this ADR and the
  spec amendments do.
- Pure-client X.509 connections have no `PeerId` on the client side, so
  any future feature that wants to route to "the connection I opened to
  `api.alk.dev`" must hold the `CallConnection` handle directly rather
  than using `PeerRef::Specific`. This is the correct constraint —
  `PeerRef::Specific` is for known peers, not for arbitrary dials — but
  it is a constraint downstream code must respect.

## Assumptions

1. **A remote reachable by Ed25519 raw key is always a known peer.**
   Raw-key remotes have no CA; the fingerprint IS the trust anchor. An
   unknown Ed25519 remote cannot be verified at all (there is no CA to
   fall back to), so the connection fails closed. This means the
   "public X.509 endpoint" role is the *only* role where the local node
   dials a remote it has no `PeerEntry` for. This is correct and
   intended — it is the same model iroh uses.

2. **Browsers never enter the peer-keyed overlay.** A browser is
   served by `alknet-http` (HTTP routes / WebTransport streams) and
   authenticates by bearer token. The hub may have a `PeerEntry` for
   the browser's token (to authorize it), but the browser is not a
   `PeerId`-bearing peer. This is the explicit closure of the
   "browser as peer" path — browsers are clients, not peers.

3. **X.509 fingerprint pinning is only for known hubs.** Pinning an
   X.509 fingerprint for an arbitrary public API is brittle (cert
   renewal) and is not done. The `PeerEntry.fingerprints` X.509 entry
   is for the hub case where the local node has a P2P trust
   relationship and wants to also recognize the hub's domain-facing
   cert.

4. **The on-chain / smart-contract discovery use case does not change
   the auth model.** It is a source of `PeerEntry` records, implemented
   as an additive `IdentityProvider` adapter (ADR-033 / OQ-36). The
   hub-and-gossip topology it implies is built from role-3 hubs, which
   this ADR confirms are ordinary `PeerEntry` peers.

## References

- OQ-37 (resolved by this ADR) — the three auth types and how X.509
  server identity fits the peer model
- [ADR-027](027-tls-identity-redesign-acme-rawkey-decoupling.md) —
  `TlsIdentity` (RawKey / X509 / Acme), the browser limitation (no RFC
  7250), WebTransport requires X.509
- [ADR-029](029-peer-graph-routing-model.md) — the peer-keyed overlay
  model that `PeerEntry` / `PeerId` feed into; pure-client connections
  are not in this graph
- [ADR-030](030-peerentry-and-identity-id-decoupling.md) — `PeerEntry`
  with mixed fingerprints; fingerprint normalization (`ed25519:` across
  quinn/iroh); the `SHA256:<hex>` X.509 fingerprint format
- [ADR-033](033-storage-boundary-and-repo-adapter-pattern.md) — the
  repo/adapter pattern that an on-chain `IdentityProvider` adapter
  follows; OQ-36 (concrete adapter shapes deferred for exploration)
- [ADR-017](017-call-protocol-client-and-adapter-contract.md) §7 —
  `CallCredentials.remote_identity` (ADR-017 specified "expected
  fingerprint or cert"; this ADR §2 extends its semantics so that
  `remote_identity: None` + no `PeerEntry` + X.509 transport selects
  CA verification)
- [ADR-024](024-operation-registry-layering.md) — the Layer 2
  per-connection overlay where ops discovered via `from_call` /
  `from_openapi` / `from_mcp` on a pure-client X.509 connection land
- OQ-29 (resolved) — key-type-aware server cert verification; this ADR
  adds the peer-model criterion (known peer vs. public X.509 endpoint)
  that selects the verifier
- OQ-10 (deferred) — git adapter scope; the on-chain / gossip-synced
  git-hosting hub use case in §6 is downstream of the git crate
- OQ-36 (open, deferred for exploration) — concrete persistence adapter
  shapes; the on-chain `IdentityProvider` adapter in §6 follows this
  pattern
- `docs/research/alknet-http/phase-0-findings.md` — DH-2 (h3 /
  WebTransport deferred past v1); the WebTransport-relay-as-proxy
  feature noted in this ADR's §5 belongs in that deferral bucket
- `docs/research/references/iroh/iroh/04-sub-crates.md` — iroh's
  transport relay (`iroh-relay`), referenced to distinguish it from
  alknet's hub role
- `docs/architecture/crates/core/auth.md` — amended: three-role
  naming, the outgoing X.509 verifier selection rule
- `docs/architecture/crates/call/client-and-adapters.md` — amended:
  outgoing X.509 connection has no client-side `PeerId`; verifier
  selection by `PeerEntry` presence