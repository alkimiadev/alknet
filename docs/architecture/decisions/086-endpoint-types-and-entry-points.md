# ADR-086: Endpoint Types and Entry Points

## Status

Accepted (resolves OQ-62)

## Context

> **Terminology note.** "Endpoint" is used in three senses in the
> alknet docs, and this ADR uses all three: (1) **`AlknetEndpoint`** —
> the struct in `alknet-core` (ADR-010/083) that runs accept loops and
> dispatches by ALPN; (2) **endpoint type** — one of web, native, or
> iroh, a composition unit with its own identity/auth/transport
> (defined in §1 below); (3) **endpoint** in the narrow ALPN sense —
> an ALPN whose connections require identity resolution before
> dispatch, contrasted with an **entry point** (defined in §2 below).
> When the sense is ambiguous, this ADR uses "endpoint type" or
> "endpoint ALPN" to disambiguate from `AlknetEndpoint` the struct.

### The tangle

The hub/endpoint/TLS configuration has an unnamed distinction at its
core that, once named, dissolves several interlocking confusions:

1. **OQ-62** asked whether a hub passes the same ALPN list to both
   `TlsServerConfig`s (raw-key + X.509/ACME) or different
   transport-appropriate lists. The question could not be answered
   cleanly because the docs treated all ALPNs as the same kind of thing.
   There was no principled basis for *why* the lists would differ.

2. **The hub README** states a hub "**must** support TCP+TLS and QUIC
   endpoints simultaneously," framing it as a hard requirement. This is
   wrong as a general statement — a minimal hub can run on iroh alone
   (no public IP/port required), and the first real use case hosts a
   subset (web + native), not all three. The "must" framing obscures
   that a hub composes a *subset* of endpoint types, and which subset
   determines which ALPNs it advertises.

3. **The ALPN registry** lists `h2`/`http/1.1` alongside
   `alknet/channels` and `alknet/call` with no distinction in kind.
   But `h2`/`http/1.1` are structurally different: a connection
   negotiating `h2` is accepted without an established peer identity
   (the registration endpoint, browser API routes), while a connection
   negotiating `alknet/channels` requires identity resolution (fingerprint
   or bearer token on channel 0). Conflating these under one "ALPN" label
   is why the ALPN-list-split question had no answer.

4. **The foundational handlers** were listed as one undifferentiated
   backlog (ssh, tunnel, socks5, fs, sftp). In fact they fall into two
   structurally different categories: channels data-channel ALPNs (tunnel,
   socks5, fs, sftp — gated by channels, opened via `channel/open`, inherit
   ACL + bidirectionality) vs. an endpoint ALPN that wraps channels inside
   it (ssh — a legacy-client entry point that runs channels-over-SSH and
   uses RFC 7250 keys). SSH is not in the same category as the rest.

### The insight: endpoint types and entry points

A hub serves different **client classes**, each with a different
identity model, auth model, and transport. These are **endpoint
types** — independent, composable listeners. A hub composes a subset;
the subset determines the ALPN lists.

Within each endpoint type, there are two kinds of ALPNs:

- **Entry points** — ALPNs whose connections are accepted without an
  established peer identity. Per-request auth may apply (registration
  token, Bearer for API routes), but the connection itself is not
  identity-gated at the TLS layer. Examples: `h2`/`http/1.1` (HTTP
  registration, browser API, stealth decoy), the future `alknet/register`
  ALPN (worker registration over QUIC/TCP without HTTP). Entry points
  exist to bootstrap a peer relationship or serve non-peer clients
  (browsers).

- **Endpoints** (in the narrow sense) — ALPNs whose connections require
  identity resolution before the handler runs. No identity → the
  connection is rejected (or, for channels, identity is resolved on
  channel 0 before dispatch). Examples: `alknet/channels`, `alknet/call`
  (when used as a top-level ALPN rather than channel 0), `alknet/ssh`.

This distinction is not cosmetic. It is what makes the ALPN-list-split
question answerable: each endpoint type serves a client class, and the
ALPNs that client class can negotiate are the ones that endpoint type
advertises. A raw-key QUIC listener serving native clients advertises
the native ALPNs (`alknet/channels`, `alknet/call`); it does not
advertise `h2`/`http/1.1` because browsers cannot connect to a raw-key
listener and native clients do not use HTTP ALPNs. An X.509 TCP+TLS
listener serving browsers and registration advertises the entry-point
ALPNs plus `alknet/channels` (for the WebSocket-channels browser path,
per OQ-65).

### Why iroh is a separate endpoint type

Iroh is not "just another transport" alongside quinn and TCP+TLS. It is
a distinct endpoint type because it combines three things that neither
quinn nor TCP+TLS provide alone:

1. **No public IP/port required** — relay-assisted p2p. A minimal hub
   with no public presence runs iroh alone.
2. **RFC 7250 raw keys built in** — iroh's `Endpoint` handles TLS
   internally; it does not consume a `rustls::ServerConfig` (ADR-082
   §"Iroh: shares the key, not the rustls config"). The ALPN list is
   set via `iroh::Endpoint::builder().alpns()`, not via
   `TlsServerConfig::new`.
3. **Key-based auth (NodeId)** — same fingerprint model as the native
   quinn path, but the connectivity is p2p.

A hub that serves all three endpoint types (a "full hub") runs three
independent listeners. A hub that serves only iroh (a "minimal hub")
runs one. The composition is additive — each endpoint type is an
independent `with_*` on `AlknetEndpoint` (ADR-083).

## Decision

### 1. Three endpoint types

A hub composes a subset of three endpoint types. Each is an independent
listener with its own identity model, auth model, and transport(s).

| Endpoint type | Identity | Auth model | Transport(s) | Client class |
|---------------|----------|------------|--------------|--------------|
| **web** | X.509 (ACME or manual) | token-based (Bearer) | TCP+TLS (HTTP, WebSocket), QUIC (WebTransport — deferred per ADR-044) | browsers, curl, registration, HTTP API consumers |
| **native** | RFC 7250 raw key (Ed25519) | key-based (fingerprint) | QUIC (primary), TCP+TLS (fallback when UDP blocked) | alknet-native clients, workers (fingerprint auth) |
| **iroh** | RFC 7250 raw key (NodeId) | key-based (fingerprint) | iroh (relay-assisted QUIC) | p2p peers, NAT'd nodes, minimal-hub deployments |

A hub may run any subset. The subsets that make sense:

| Hub shape | Endpoint types | Public IP required? | Example |
|-----------|---------------|---------------------|---------|
| **full hub** | web + native + iroh | yes (web, native) | the general case — browsers, native clients, p2p |
| **web + native** | web + native | yes | the first real use case — public domain, native clients |
| **native + iroh** | native + iroh | yes (native only) | a hub without browser-facing services |
| **minimal hub** | iroh only | no | a p2p-only hub behind NAT, relay-assisted |

The first real use case is **web + native** (public domain with X.509
for browsers/registration + raw-key QUIC for native clients). Iroh is a
hard requirement for the project (the p2p, no-public-IP case) but is
not in the first deployed subset. All three are hard requirements for
the project as a whole — a full hub runs all three.

### 2. Entry points vs. endpoints (the ALPN-level distinction)

ALPNs fall into two categories:

**Entry points** — connections accepted without an established peer
identity. The TLS handshake succeeds without client identity; auth
happens per-request inside the handler (registration token, Bearer
header, or the call-protocol `auth_token` on channel 0 for a channels
connection that has not yet established identity). Entry points exist
to bootstrap a peer relationship (worker registration) or to serve
non-peer clients (browsers, curl).

| ALPN | Category | Handler | Purpose |
|------|----------|---------|---------|
| `h2` / `http/1.1` | entry point | `HttpAdapter` | HTTP registration, browser API routes, stealth decoy, WebSocket upgrade |
| `alknet/register` (future) | entry point | (registration handler) | Worker registration over QUIC/TCP without HTTP — a direct ALPN for enrollment, avoiding the HTTP layer. Not yet specced; tracked as a hub concern. |

**Endpoints** (narrow sense) — connections that require identity
resolution before the handler runs. The TLS handshake or the
call-protocol first frame must produce an identity (fingerprint or
token); no identity → rejected.

| ALPN | Category | Handler | Identity source |
|------|----------|---------|-----------------|
| `alknet/channels` | endpoint | `ChannelsAdapter` | Fingerprint (raw key / client cert) or bearer token on channel 0 (ADR-072) |
| `alknet/call` | endpoint | `CallAdapter` | Fingerprint or bearer token (first frame) — when used as a top-level ALPN; as channel 0 inside channels, identity is resolved before dispatch |
| `alknet/ssh` (future) | endpoint | (ssh handler) | RFC 7250 key fingerprint — SSH is a legacy-client entry point that wraps channels inside it (see §4 below) |

The distinction is structural, not cosmetic: it determines which
`TlsServerConfig` advertises which ALPNs (§3 below) and which
connections require identity at the TLS layer vs. per-request.

### 3. ALPN lists are split per endpoint type (resolves OQ-62)

**Option B — split list, transport-appropriate.** Each `TlsServerConfig`
advertises only the ALPNs its client class can negotiate. The assembly
layer filters `registry.alpn_strings()` by endpoint type.

| `TlsServerConfig` / iroh builder | Endpoint type | ALPNs advertised |
|----------------------------------|---------------|------------------|
| raw-key config (`for_quinn`) | native | `alknet/channels`, `alknet/call`, `alknet/ssh` (future) — the native ALPNs |
| raw-key config (`for_tcp_tls`) | native (TCP fallback) | `alknet/channels`, `alknet/call` — native clients using TCP+TLS when UDP is blocked |
| X.509/ACME config (`for_tcp_tls`) | web | `h2`, `http/1.1`, `alknet/channels` (for WebSocket-carrying-channels, per OQ-65), `acme-tls/1` (appended automatically by `TlsServerConfig::new`) |
| X.509/ACME config (`for_quinn`) | web (WebTransport — deferred) | `h2`, `http/1.1`, `h3` (when WebTransport revives per ADR-044) |
| iroh `Endpoint::builder().alpns()` | iroh | `alknet/channels`, `alknet/call` — iroh uses raw keys only; native ALPNs |

Rationale for split (not same-list):

- **No dead negotiation.** A raw-key QUIC listener does not advertise
  `h2`/`http/1.1` — browsers cannot connect to a raw-key listener
  (RFC 7250 unsupported), and native clients do not negotiate HTTP
  ALPNs. Advertising them is harmless but misleading: it implies the
  listener serves a client class it cannot.
- **No misleading advertisement.** An X.509 TCP+TLS listener does not
  advertise `alknet/call` as a top-level ALPN unless the deployment
  explicitly serves native call-over-TCP clients. The web endpoint
  serves browsers and registration, not raw call-protocol clients. If
  a deployment wants call-over-TCP on the web endpoint, it adds
  `alknet/call` to the X.509 config's list — an explicit choice, not a
  default.
- **The `alknet/channels` exception.** The web endpoint advertises
  `alknet/channels` so that WebSocket-carrying-channels (OQ-65) works:
  a browser opens a WebSocket (HTTP upgrade on `h2`/`http/1.1`), and
  the WebSocket stream carries the channels protocol. The channels
  ALPN is advertised on the X.509 config for this path, not for
  native-channels-over-TCP (which is the native endpoint's concern).
- **`alknet/register` (future).** When the direct-registration ALPN
  exists, it is an entry point advertised on both the native and web
  configs (workers may register over either transport). It is not
  advertised on iroh (iroh peers establish identity via NodeId, not
  registration tokens). Revisiting the iroh exclusion requires a new
  ADR.

The assembly layer builds the ALPN lists. The pattern (split by
endpoint type) is the one-way door — downstream consumers copy it.
The specific ALPNs in each list are two-way (additive — adding
`alknet/register` or `alknet/ssh` to a config's list is a config
change, not a structural one).

### 4. Foundational handlers — two categories

The foundational handlers are not one undifferentiated backlog. They
fall into two structurally different categories:

**Channels data-channel ALPNs** — gated by the channels substrate,
opened via `channel/open` on channel 0, inherit ACL + bidirectionality
from channels + call. These are NOT in any `TlsServerConfig`'s ALPN
list — they are negotiated inside channels, not at the TLS layer.

| ALPN (inside channels) | Crate | Status |
|------------------------|-------|--------|
| `alknet/tty` | `alknet-tty` | specced (ADR-052–057), implemented |
| `alknet/tunnel` | (in `alknet-channels` or sibling) | POC-validated, not yet specced |
| `alknet/socks5` | (TBD) | not yet specced — SOCKS5 proxy over channels |
| `alknet/fs` | (TBD) | not yet specced — filesystem access over channels |
| `alknet/sftp` | (TBD) | not yet specced — SFTP over channels |

**SSH — an endpoint ALPN that wraps channels.** SSH is structurally
different from the channels data-channel ALPNs. It is an endpoint ALPN
(negotiated at the TLS layer on the native config), and it runs
channels *inside* it (channels-over-SSH): the SSH server accepts a
connection, and each SSH channel becomes a channels data-channel ALPN.
SSH uses the same RFC 7250 keys as the native endpoint — it is a
legacy-client entry point for git/sftp compatibility, not a new
identity model. SSH is gated by channels (the channels run inside it)
but is itself an endpoint ALPN, not a data-channel ALPN.

| ALPN | Crate | Category | Status |
|------|-------|----------|--------|
| `alknet/ssh` | `alknet-ssh` | endpoint ALPN (wraps channels) | not yet specced — russh server channels wrapper for git/sftp compat; legacy clients; comes after tunnel/sftp/etc. |

SSH is advertised on the **native** config's ALPN list (raw-key QUIC
or TCP+TLS), not the web config. It is a legacy-native-client path,
not a browser path. It comes later in the roadmap — tunnels, sftp, and
other channels data-channel ALPNs are prioritized first because they
serve the primary use cases; SSH serves legacy compatibility.

### 5. A hub composes a subset; "must support TCP+TLS and QUIC" is corrected

The hub README's "a hub **must** support TCP+TLS and QUIC endpoints
simultaneously" is corrected. A hub composes a subset of endpoint
types. The subset determines which transports and ALPN lists the hub
uses. A minimal hub (iroh only) has no TCP+TLS and no quinn listener;
a web+native hub has both but no iroh; a full hub has all three.

The "must" applied to the first real use case (web + native), not to
all hubs. The correction is structural: the hub crate's composition API
takes a subset of endpoint types, not a fixed pair.

## What this does NOT change

- **`AlknetEndpoint` (ADR-083)** — the endpoint struct is unchanged.
  It takes transports via `with_quinn` / `with_iroh` / `with_tcp_tls`
  and runs their accept loops. The endpoint-types model is a
  composition pattern at the assembly layer, not a new endpoint struct
  field. The assembly layer builds the `TlsServerConfig`s and the
  transports per endpoint type and hands them to the endpoint.
- **`TlsServerConfig` (ADR-082)** — the `new(identity, alpns)` signature
  is unchanged. The caller (assembly layer) decides the ALPN list; this
  ADR specifies *how* the caller decides — by endpoint type, not by
  "same list or split" guesswork.
- **`HandlerRegistry` (ADR-010)** — all ALPNs (entry-point and endpoint)
  are registered on the same registry. The distinction is in which
  `TlsServerConfig` advertises them, not in which registry holds them.
  A connection negotiating `h2` and a connection negotiating
  `alknet/channels` both dispatch through the same `HandlerRegistry`;
  the difference is which listener accepted them and whether identity
  was required at the TLS layer.
- **The channels substrate (ADR-071)** — channels data-channel ALPNs
  are unchanged. They are negotiated inside channels, not at the TLS
  layer.
- **ADR-034 (three peer roles)** — the three peer roles (public X.509
  endpoint, transport relay, hub/hosting node) are about *client-side
  outbound* identity. This ADR is about *server-side inbound* endpoint
  composition. They are orthogonal: a hub (role 3) composes endpoint
  types for inbound; a client dialing a public X.509 endpoint (role 1)
  is an outbound concern. The two decisions compose without conflict.
- **ADR-044 (WebSocket for browsers)** — the browser bidirectional path
  uses WebSocket (unchanged). ADR-048 is **not superseded** by this
  ADR; OQ-65 is a separate question that may extend ADR-048 (WebSocket
  carrying channels, not just call). Whether WebSocket carries the call
  protocol only (ADR-048) or carries channels (OQ-65) is a separate
  question; this ADR's ALPN-list-split accounts for either outcome by
  advertising `alknet/channels` on the web config by default (the
  WebSocket-carrying-channels path needs it; the
  WebSocket-carrying-call-only path does not, but the advertisement is
  harmless if the hub also serves native channels-over-TCP on the web
  config).

## Consequences

**Positive:**

- **OQ-62 is resolved.** The ALPN-list question has a principled
  answer: split by endpoint type, because each endpoint type serves a
  different client class with different negotiable ALPNs. The assembly
  layer's wiring pattern is now guessable, not a fork in the docs.
- **The hub's composition is explicit.** A hub composes a subset of
  endpoint types; the subset determines transports, identity models,
  auth models, and ALPN lists. "Must support TCP+TLS and QUIC" is
  corrected to "composes the subset its deployment needs." A minimal
  hub (iroh only) is a first-class shape, not a degenerate case.
- **Entry points are named.** The structural difference between
  `h2`/`http/1.1` (accepted without identity, per-request auth) and
  `alknet/channels` (identity required) is explicit. This unblocks the
  `alknet/register` ALPN design (worker registration over QUIC/TCP
  without HTTP) — it is an entry point, not an endpoint, and its
  semantics follow from the distinction.
- **SSH is correctly categorized.** SSH is an endpoint ALPN that wraps
  channels, not a channels data-channel ALPN. It is advertised on the
  native config, not the web config, and it uses RFC 7250 keys (same
  as the native endpoint). The foundational handler backlog is no
  longer an undifferentiated list.
- **The first real use case is clear.** Web + native (public domain
  with X.509 + raw-key QUIC) is the first deployed subset. Iroh is a
  hard requirement for the project but not the first deployed subset.
  The roadmap is: web + native first, iroh, then foundational handlers
  (tunnel, sftp, socks5, fs), then SSH (legacy compat, last).

**Negative:**

- **The assembly layer has more logic.** Splitting ALPN lists by
  endpoint type requires the assembly layer to filter
  `registry.alpn_strings()` per `TlsServerConfig`. This is a small
  amount of code (a filter per config) but is more than "pass the same
  list to both." The pattern is documented here; downstream consumers
  copy it.
- **`alknet/channels` appears on the web config.** This is correct
  (WebSocket-carrying-channels needs it, per OQ-65) but means the web
  config advertises an ALPN that is also on the native config. A
  deployment that does not serve WebSocket-channels and does not serve
  native-channels-over-TCP on the web endpoint can omit
  `alknet/channels` from the web config. The default (include it) is
  safer; the omission is an explicit assembly-layer choice.
- **The `alknet/register` ALPN is not yet specced.** This ADR names it
  as a future entry point but does not design it. Worker registration
  over HTTP (the current OQ-58 path) remains the first implementation;
  `alknet/register` is a later simplification that removes the HTTP
  dependency from the registration flow.

## Door type

**One-way.** The three-endpoint-type model, the entry-point/endpoint
distinction, and the split-by-endpoint-type ALPN list pattern are
structural. The assembly-layer wiring pattern is what downstream
consumers copy; reversing it (back to same-list, or back to
undifferentiated ALPNs) would break the composition model and re-introduce
the tangle this ADR resolves. The specific ALPNs in each list are
two-way (additive); the pattern (split by endpoint type) is one-way.

## References

- OQ-62 (resolved by this ADR) — does a hub pass the same ALPN list to
  both `TlsServerConfig`s?
- [ADR-082](082-alknet-tls-extraction.md) — `TlsServerConfig::new(identity, alpns)`;
  the caller decides the ALPN list; this ADR specifies how
- [ADR-083](083-endpoint-as-accept-loop-runner.md) — `AlknetEndpoint` as
  multi-transport accept-loop runner; the endpoint-types model is a
  composition pattern on top of it
- [ADR-085](085-workspace-scope-core-vs-consumer-repos.md) — workspace
  scope; the foundational handler categorization (§4) amends the scope
  table's handler list
- [ADR-071](071-channels-wire-format.md) — channels data-channel ALPNs
  are negotiated inside channels, not at the TLS layer
- [ADR-072](072-channel-0-pre-negotiated-call.md) — channel 0 identity
  resolution (why `alknet/channels` is an endpoint, not an entry point)
- [ADR-044](044-defer-webtransport-browsers-use-websocket.md) —
  WebSocket for browsers; WebTransport deferred
- [ADR-048](048-websocket-native-session-not-gateway.md) — WebSocket
  carries the native call-protocol session (OQ-65 may extend this to
  channels)
- [ADR-034](034-outgoing-only-x509-and-three-peer-roles.md) — three
  peer roles (client-side outbound); orthogonal to this ADR's
  server-side inbound endpoint composition
- [ADR-027](027-tls-identity-redesign-acme-rawkey-decoupling.md) —
  `TlsIdentity` (RawKey / X509 / Acme); the identity models per
  endpoint type
- OQ-58 — worker registration flow (the entry point that
  `alknet/register` will eventually serve directly)
- OQ-65 — WebSocket carrying channels (the browser path that requires
  `alknet/channels` on the web config)