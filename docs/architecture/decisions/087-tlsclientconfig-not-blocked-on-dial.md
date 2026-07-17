# ADR-087: `TlsClientConfig` Is Not Blocked on the Dial Seam

## Status

Accepted (resolves OQ-64; input framing amended 2026-07-16 by ADR-091 —
`ClientVerifierContext` is derived from `ConnectionCredentials.remote_identity`,
not `CallCredentials.remote_identity`; the `auth_token` is not in the
dial's credential bundle)

## Context

### The circular hedge

OQ-64 and OQ-55 were linked in a way that formed a circular dependency:

- **OQ-64** said: "the client-side TLS helper is blocked on the
  `AlknetClient` dial-seam extraction (OQ-55), because the TLS helper
  and the dial are the same seam."
- **OQ-55** said: "the dial seam is blocked on a second transport's
  real dial existing."
- A second transport's dial requires a `rustls::ClientConfig` to dial
  with — which is the client-side TLS helper.

This is circular: the prerequisite (TLS config) is deferred behind the
thing that needs it (the dial). No second transport can dial until it
has a TLS config; the TLS config is deferred until a second transport
dials. The dial never arrives because the config never arrives because
the dial never arrives. Schrödinger's code — required and not required
until observed.

This is the same pattern that stalled the server-side transport
generalization before ADR-065 broke it: "we can't generalize until we
have two transports, but we can't build the second transport until we
generalize." ADR-065 broke it by separating the *Connection* (the
take-over, transport-agnostic, built now) from the *dial* (the
establishment, transport-specific, per-transport). This ADR does the
same on the client side.

### The two things that were conflated

The "same seam" claim conflated two distinct concerns:

1. **`TlsClientConfig`** — the `rustls::ClientConfig` + ADR-034
   verifier selection (fingerprint pin for known peers, CA-verify for
   unknown X.509, fail-closed for unknown raw-key) + ADR-084 crypto
   provider (`aws_lc_rs`). This is **transport-agnostic**. The verifier
   selection rule (ADR-034 §3) is keyed on `PeerEntry` presence and
   remote cert type, not on transport. The crypto provider (ADR-084) is
   the same on all paths. The fingerprint normalization (ADR-030 §6,
   `ed25519:<hex>` / `SHA256:<hex>`) is transport-agnostic. All
   decisions are made. There is nothing to discover from a second
   transport's dial — the rule is the same regardless of whether the
   dial is QUIC, TCP+TLS, or iroh.

2. **The dial** (`AlknetClient::dial()`) — transport-specific
   connection establishment. QUIC dial (`quinn::Endpoint::connect`),
   TCP+TLS dial (`TcpStream::connect` + `TlsConnector::connect`), iroh
   dial (`iroh::Endpoint::connect`). Extracting a transport-polymorphic
   dial from one shape (QUIC) would bake QUIC in as *the* establishment
   shape — the same welding ADR-065 unwound on the server side. **This
   is the legitimate deferral** (OQ-55, unchanged).

The TLS config is a **prerequisite** for the dial, not a consequence of
it. You build the `rustls::ClientConfig` first, then you dial with it.
The dial passes the config to the transport-specific connector
(`quinn::Endpoint::connect_with` takes a `ClientConfig`;
`TlsConnector::connect` takes a `ClientConfig`; iroh takes a
`SecretKey` — the one exception, see "Iroh" below). The config does not
flow *from* the dial; it flows *into* it.

### The hub makes this non-optional

A hub **has to** be a client. The hub dials out to workers it
supervises; a hub (B) that connects to another hub (A) is a client
from A's perspective. The hub README already has
`dial_worker_connection` and `supervise_worker` — those are client
operations that need a client-side TLS config. The first hub deployment
(web + native, per ADR-086) dials workers over QUIC (native endpoint,
raw key). That dial needs a `rustls::ClientConfig` with the ADR-034
verifier (fingerprint pin for the worker's known Ed25519 key).

There is no "later" for this. The first hub deployment needs the
client-side TLS config. Deferring it behind the dial-seam extraction
(OQ-55, which is genuinely blocked on a second transport) means the
first hub cannot be built — or, worse, each client rebuilds the
verifier selection + provider wiring standalone, and the duplicated
boilerplate drifts (one crate uses `aws_lc_rs`, another uses `ring`,
the convention breaks silently — exactly the ADR-084 consistency risk
the convention was supposed to prevent).

`alknet-worker` cannot exist without a client (it dials a hub). The
hub cannot exist without a client (it dials workers and other hubs).
The client-side TLS config is on the critical path for both. It is not
a future extraction; it is a present prerequisite.

## Decision

### 1. `alknet-tls` provides `TlsClientConfig`

`alknet-tls` grows a client-side config type alongside
`TlsServerConfig`:

```rust
pub struct TlsClientConfig {
    config: rustls::ClientConfig,
}

impl TlsClientConfig {
    /// Build a client TLS config for the given remote identity context.
    /// Applies ADR-034 verifier selection:
    ///   - known peer (PeerEntry present) + raw key → fingerprint pin
    ///   - known peer (PeerEntry present) + X.509 → fingerprint pin
    ///   - unknown remote + X.509 → CA verification (WebPkiServerVerifier)
    ///   - unknown remote + raw key → fail closed
    /// Applies ADR-084 crypto provider (aws_lc_rs::default_provider()).
    pub fn new(verifier_context: ClientVerifierContext) -> Result<Self, TlsError>;
}
```

The `ClientVerifierContext` carries the inputs to ADR-034's verifier
selection: whether a `PeerEntry` exists for the remote, the expected
fingerprint (if known), and the remote cert type (if known). The exact
shape of this context is an implementation detail (the decisions are
in ADR-034; the struct is a bag of already-decided inputs). It is
sketched lightly here; the full variant-granularity of `TlsError` is
OQ-63 (the next session).

> **Amendment 2026-07-16 (ADR-091):** `ClientVerifierContext` is derived
> from `ConnectionCredentials.remote_identity` (the transport-level
> credential bundle), not `CallCredentials.remote_identity`. The dial
> (`AlknetClient::dial_quic` / `dial_tcp_tls`) extracts
> `creds.remote_identity` from `ConnectionCredentials` and builds
> `ClientVerifierContext` from it. `CallCredentials` is no longer in the
> dial's path — its `auth_token` dimension is a call-protocol concept,
> not a transport credential. See
> [ADR-091](091-connectioncredentials-decouple-dial-from-call.md).

This is **not** the dial. `TlsClientConfig` produces a
`rustls::ClientConfig`; the caller (the transport-specific dial helper
— now `AlknetClient::dial_quic` / `dial_tcp_tls` per ADR-089; the
per-protocol `CallClient::connect` / `ChannelClient::connect_quic`
convenience constructors are removed per ADR-089 §5) passes it to the
transport's connector. The config is transport-agnostic; the dial is
not.

### 2. The dial seam (OQ-55) — subsequently resolved by ADR-089

> **Update (2026-07-16):** OQ-55 is now resolved by ADR-089. The text
> below is the original (pre-ADR-089) framing, preserved for context.
> `AlknetClient` is the extracted dial seam; the per-protocol
> convenience constructors are removed, not retained as wrappers.

OQ-55 (the `AlknetClient` transport-polymorphic dial extraction)
~~remains deferred~~ **is resolved by ADR-089**. The deferral was about
the *dial* — transport-specific connection establishment — not about
the TLS config. With `TlsClientConfig` in `alknet-tls`, each
transport-specific dial helper (now `AlknetClient::dial_quic` /
`dial_tcp_tls` / `dial_iroh`, ADR-089) builds its `TlsClientConfig`
and passes it to its transport's connector. The friction (each dial
helper calls `TlsClientConfig::new` + its transport's connect) is real
but narrow — it's duplicated `TlsClientConfig::new` calls, not
duplicated verifier
selection logic. When a second transport's dial exists, the dial
seam is extractable (OQ-55 unblocked); the TLS config is already
shared by then.

### 3. Iroh is the one exception (shares the key, not the config)

Iroh's client side, like its server side (ADR-082 §"Iroh: shares the
key, not the rustls config"), does not consume a `rustls::ClientConfig`
— it takes an `iroh::SecretKey` and handles TLS internally. The iroh
client dial does not use `TlsClientConfig`. The `Ed25519SecretKey`
(from `StaticConfig`, in core) feeds `iroh::SecretKey::from_bytes`
directly, same as the server side.

The verifier selection for iroh is also different: iroh's built-in TLS
verifies the remote's `NodeId` (Ed25519 public key) against the
expected `NodeId`. This is fingerprint-pinning by another name — the
`NodeId` IS the fingerprint. An unknown iroh remote fails closed (no
CA to fall back to — ADR-034 §3, Assumption 1). `TlsClientConfig` does
not cover the iroh path; the iroh dial helper applies the same
ADR-034 rule (known peer → pin, unknown → fail closed) via iroh's own
API.

### 4. `alknet-tls` is no longer "server-only"

The TLS README's "Server-only (for now)" section is removed.
`alknet-tls` provides both `TlsServerConfig` (inbound) and
`TlsClientConfig` (outbound). The server side is unchanged (ADR-082);
the client side is added by this ADR.

The provider-consistency convention (ADR-084: `aws_lc_rs` on all
paths) moves from "enforced by convention" to "enforced by
`TlsClientConfig::new`" for the rustls-consuming transports (quinn,
TCP+TLS). The iroh path uses iroh's built-in `tls-aws-lc-rs` feature
(already consistent).

### 5. The hub-as-client requirement is a first-class use case

The hub's `dial_worker_connection` / `supervise_worker` (hub README
§"Dial (outbound workers)") are client operations. They need a
`TlsClientConfig` for the outbound dial. The hub-as-client case is
not a "future use case the resolved helper must cover" — it is a
present requirement that drives the resolution. A hub that supervises
workers dials them over the native endpoint (QUIC, raw key) using a
`TlsClientConfig` with the worker's fingerprint pinned (ADR-034 §3,
known peer + raw key). A hub that connects to another hub dials it
the same way.

## What this does NOT change

- **`TlsServerConfig` (ADR-082)** — the server-side config is
  unchanged. `TlsClientConfig` is a separate type, same crate.
- **The dial seam (OQ-55)** — the transport-polymorphic dial extraction
  remains deferred. This ADR extracts the TLS config, not the dial.
  When OQ-55 resolves, `AlknetClient::dial()` will call
  `TlsClientConfig::new` + the transport-specific connector; the
  config is already shared by then.
- **ADR-034 (verifier selection)** — the rule is unchanged. This ADR
  centralizes its implementation in `TlsClientConfig::new` instead of
  each client rebuilding it.
- **ADR-084 (crypto provider)** — the provider is unchanged. This ADR
  moves enforcement from convention to code for the client side.
- **`CallClient` / `ChannelClient` take-over APIs** —
  `spawn_dispatch` / `from_connection` are transport-agnostic and
  decided (ADR-017, ADR-080). They take a pre-established `Connection`.
  This ADR is about how the caller builds the TLS config *before*
  establishing that `Connection`, not about the take-over.
- **`FingerprintPinVerifier`** — the existing verifier in
  `alknet-call` is the current implementation of ADR-034's
  fingerprint-pin path. It **moves to `alknet-tls`** (amended by
  ADR-089 §5): with `CallClient::connect` removed, it has no remaining
  home in `alknet-call`, and it is a TLS concern (implements
  `rustls::client::danger::ServerCertVerifier`). `TlsClientConfig::new`
  constructs it. Moving it lets `alknet-call` shed its direct `rustls`
  dep entirely — `CallClient` becomes a pure protocol crate.

## Consequences

**Positive:**

- The circular dependency is broken. `TlsClientConfig` is buildable
  today; the dial seam (OQ-55) is no longer blocking it. A second
  transport's dial can be built using `TlsClientConfig` + the
  transport's connector, without waiting for the dial-seam extraction.
- The hub-as-client requirement is met. The hub's
  `dial_worker_connection` / `supervise_worker` use
  `TlsClientConfig::new` for the outbound dial's TLS config. The first
  hub deployment (web + native) can dial workers over QUIC with the
  worker's fingerprint pinned.
- `alknet-worker` is unblocked on the TLS front. A worker dials a hub
  using `TlsClientConfig::new` + the transport-specific connector. The
  dial seam (OQ-55) is about extracting the shared dial, not about
  blocking the worker from dialing.
- Provider consistency (ADR-084) is enforced by code, not convention,
  for the client side. `TlsClientConfig::new` uses
  `aws_lc_rs::default_provider()`; every client that uses it gets the
  right provider. The convention-based risk (one crate drifting to
  `ring`) is removed.
- The duplicated boilerplate (each client rebuilding verifier
  selection + provider wiring) is centralized. `TlsClientConfig::new`
  is the single point where ADR-034's rule and ADR-084's provider are
  applied.

**Negative:**

- `alknet-tls` grows a client-side type. The crate is no longer
  "server-only." This is correct — the crate's purpose is shared TLS
  config, and the client side is shared across all outbound-dialing
  crates (hub, worker, `CallClient`, `ChannelClient`).
- The iroh client path does not use `TlsClientConfig`. This is
  unavoidable — iroh has its own TLS. The iroh dial helper applies the
  same ADR-034 rule via iroh's API. The consistency is in the rule,
  not in the type.
- `TlsError` (OQ-63) now covers both server and client errors. The
  variant granularity is slightly larger (client-side variants:
  verifier construction, provider init, unknown-remote fail-closed).
  OQ-63 is the next session and will account for both.

## Door type

**One-way.** `TlsClientConfig` as the shared client-side TLS config in
`alknet-tls` is structural — every outbound-dialing crate depends on
it. Reversing would mean re-distributing verifier selection + provider
wiring across crates, reintroducing the convention-based consistency
risk. The `TlsClientConfig::new` signature (takes a verifier context,
returns a `rustls::ClientConfig`) is one-way — changing it after
consumers exist is a rewrite. The internal implementation (how the
verifier context struct is shaped, how `FingerprintPinVerifier` relates
to the CA-verify path) is two-way.

## References

- OQ-64 (resolved by this ADR) — should `alknet-tls` provide a
  client-side TLS config helper?
- OQ-55 (unaffected — the dial seam remains deferred; this ADR
  extracts the TLS config, not the dial)
- [ADR-034](034-outgoing-only-x509-and-three-peer-roles.md) §3 —
  verifier selection rule (known peer → fingerprint pin; unknown X.509
  → CA verify; unknown raw-key → fail closed)
- [ADR-084](084-aws-lc-rs-crypto-provider.md) — aws-lc-rs crypto
  provider on all paths
- [ADR-082](082-alknet-tls-extraction.md) — `TlsServerConfig`
  (server-side; this ADR adds the client-side analogue)
- [ADR-065](065-connection-from-stream-generic-single-stream.md) — the
  server-side precedent: separate the take-over (transport-agnostic,
  built now) from the dial (transport-specific, per-transport). This
  ADR is the client-side analogue.
- [ADR-086](086-endpoint-types-and-entry-points.md) — the hub composes
  endpoint types and dials workers (hub-as-client)
- OQ-63 — `TlsError` shape (next session; now covers both server and
  client variants)
- `docs/architecture/crates/hub/README.md` §"Dial (outbound workers)" —
  the hub-as-client operations that need `TlsClientConfig`