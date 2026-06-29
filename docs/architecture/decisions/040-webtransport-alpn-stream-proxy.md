# ADR-040: WebTransport ALPN-Stream-Proxy

## Status

Proposed

## Context

`alknet-http`'s `h3` handler serves browsers over WebTransport. The
existing specs ([webtransport.md](../crates/http/webtransport.md),
[ADR-038](038-http3-and-webtransport-as-first-class.md)) describe two
stream destinations within a WebTransport session:

1. Call-protocol `EventEnvelope` → the call protocol's `Dispatcher`
2. HTTP/3 requests → the axum `Router` (ADR-036)

But there is a third, more important use case that the specs did not
capture: **a browser opening a WebTransport stream to speak a different
ALPN protocol directly** — SSH, git, SFTP — with a WASM parser on the
browser side. This is the "VPN-like without being a VPN" use case the
project was originally built for, now on a clean architectural
foundation.

### The use case

A browser connects to a hub over WebTransport (`h3`, X.509). It wants
to reach the hub's SSH service (or git, or SFTP). It cannot open a
`quinn` connection on ALPN `alknet/ssh` from the browser — browsers
don't speak raw QUIC with arbitrary ALPNs, they speak WebTransport. But
a WebTransport bidirectional stream is a QUIC bidirectional stream
(ADR-012), and the `BiStream` trait (`AsyncRead + AsyncWrite + Send +
Unpin`, ADR-007) was designed so a browser can implement it over a
WebTransport stream. So the browser:

1. Opens a WebTransport session to the hub.
2. Creates a bidirectional stream.
3. Runs a WASM parser for the target protocol (SSH, SFTP, etc.) that
   reads/writes the WebTransport stream as a `BiStream`.

The hub's `h3` handler needs to hand that WebTransport stream to the
target ALPN handler (e.g., `SshAdapter`) as if it were a QUIC stream
arriving on that ALPN. The `h3` handler becomes an **ALPN-stream-proxy**:
a browser-side gateway that gives browsers access to any ALPN handler
via WebTransport.

### Why this matters

- **SSH is hard to block.** SSH can run over TLS, QUIC streams, or any
  stream. A browser running a WASM SSH client over WebTransport is
  indistinguishable from normal HTTPS traffic at the network layer
  (WebTransport is HTTP/3 over QUIC over UDP, the same as HTTP/3). This
  is the anti-censorship property: the protocol that governments most
  want to block (VPN-like connectivity) rides on the protocol they
  can't block (HTTPS/HTTP/3) without breaking the web.
- **The browser is the universal client.** With WASM parsers for the
  target protocols, a browser becomes a full alknet client — SSH shell,
  SFTP file browser, git client — without installing anything. The
  `h3` handler's ALPN-stream-proxy is what makes this possible: it
  bridges the browser's WebTransport streams to the server's ALPN
  handlers.
- **WASM parsers are feasible.** `russh-sftp`'s protocol parsing
  already targets WASM; there's no conceptual reason SSH itself can't.
  The `BiStream` trait's design (ADR-007) preserves the WASM door
  specifically for this — a browser implements `BiStream` over a
  WebTransport stream, and the WASM parser speaks the protocol over
  it.

### The structural question

The `h3` handler proxying a WebTransport stream to another ALPN
handler requires the `HttpAdapter` to have access to the
`HandlerRegistry` (or a subset of it) — to look up the target ALPN
handler and hand the stream to its `handle()` method. The current
`HttpAdapter` (per [http-server.md](../crates/http/http-server.md)) has
`Arc<dyn IdentityProvider>` and `Arc<OperationRegistry>`, but not the
`HandlerRegistry`. This is a structural relationship the HTTP handler
didn't need before; the ALPN-stream-proxy requires it.

This is a one-way door: once browsers build WASM clients that reach SSH
(or git, or SFTP) via WebTransport, removing the proxy path breaks
them. The stream-routing contract (how the `h3` handler decides which
ALPN handler a WebTransport stream targets) is the published interface
that WASM clients build against.

## Decision

### 1. The `h3` handler is an ALPN-stream-proxy for browser access to any ALPN handler

A WebTransport session opened by a browser can carry streams targeting
any ALPN handler, not just the call protocol. The `h3` handler's
stream-routing within a WebTransport session has three destinations:

1. **Call-protocol `EventEnvelope`** → the call protocol's `Dispatcher`
   (the existing path, [webtransport.md](../crates/http/webtransport.md)).
2. **ALPN-handler proxy** → the `h3` handler looks up the target ALPN
   handler in the `HandlerRegistry`, wraps the WebTransport stream as a
   `Connection`, and calls the handler's `handle()` method — as if the
   stream had arrived on that ALPN. The browser's WASM parser speaks
   the target protocol directly over the stream. This is the
   ALPN-stream-proxy.
3. **Other sub-protocols** — sessions may carry other framing
   conventions; the session's purpose is declared at CONNECT time.

### 2. Stream routing: the session's CONNECT path declares the target

The `h3` handler determines the target ALPN for a WebTransport session
at CONNECT time, from the session request's path/origin — not by
peeking the first application frame. A browser opens a WebTransport
session to:

- `https://hub.example.com/` (or `/alknet/call`) → call-protocol
  session (destination 1).
- `https://hub.example.com/alknet/ssh` → SSH-proxy session
  (destination 2); streams within this session are handed to the
  `SshAdapter`.
- `https://hub.example.com/alknet/git` → git-proxy session;
  streams → `GitAdapter`.

The path is the routing key. The first-frame tag (EventEnvelope vs. raw
SSH bytes) is a belt-and-suspenders check, not the routing mechanism —
the session's CONNECT path already declared the target. This is the
same principle as the HTTP/3-request-vs-WebTransport-session
distinction (framing layer, not application bytes).

### 3. The `HttpAdapter` gains a `HandlerRegistry` reference

The `HttpAdapter` struct gains `Arc<HandlerRegistry>` (or an
equivalent mechanism for looking up ALPN handlers) so the `h3` handler
can dispatch WebTransport streams to the target ALPN handler. This is
the structural change the ALPN-stream-proxy requires. The `h2`/
`http/1.1` path does not use it (those handlers serve HTTP, not
ALPN-proxy streams); the `HandlerRegistry` reference is only used by
the `h3` handler's WebTransport session routing.

```rust
pub struct HttpAdapter {
    identity_provider: Arc<dyn IdentityProvider>,
    registry: Arc<OperationRegistry>,
    handlers: Arc<HandlerRegistry>,   // NEW — for the h3 ALPN-stream-proxy
    decoy: DecoyConfig,
}
```

The assembly layer constructs the `HttpAdapter` with the
`HandlerRegistry` it already builds for the endpoint — no new registry,
no new construction path. The `HttpAdapter` is registered in the same
`HandlerRegistry` it holds a reference to (a reference cycle that is
broken by the endpoint owning both, not by the handler owning itself).

### 4. The browser's WASM parser is the client-side implementation

The `h3` handler's ALPN-stream-proxy hands a WebTransport stream to the
target ALPN handler as a `Connection`. The browser side runs a WASM
parser for the target protocol (SSH, SFTP, git) that reads/writes the
WebTransport stream as a `BiStream`. The `BiStream` trait (ADR-007) is
the contract: a browser implements `BiStream` over a WebTransport
stream, and the WASM parser speaks the protocol over it.

The WASM parsers are not part of `alknet-http` — they are separate
artifacts (the SSH WASM client, the SFTP WASM client, the git WASM
client) built against the `BiStream` contract and the target protocol's
wire format. `alknet-http`'s job is the server-side proxy path; the
browser-side WASM is downstream. The `russh-sftp` protocol parsing
already targets WASM, demonstrating feasibility; SSH is the same
pattern.

### 5. Auth for proxied ALPN sessions

A browser opening a WebTransport session to `/alknet/ssh` authenticates
by bearer token on the WebTransport session request (the HTTP
`Authorization` header on the CONNECT request), resolved by the hub's
`IdentityProvider::resolve_from_token` — same as any other browser
connection (ADR-034 §4). The browser is not an alknet peer (no
`PeerId`). The target ALPN handler (`SshAdapter`) receives the
`Connection` and `AuthContext` from the `h3` handler; the `AuthContext`
carries the bearer-token-resolved `Identity`. The SSH session then
proceeds with its own auth (the browser's WASM SSH client does SSH
key exchange over the WebTransport stream, same as a native SSH client
would over a QUIC stream).

The bearer token gates the WebTransport session (does the browser
have access to this hub at all?); the SSH protocol's own auth gates
the SSH session (does this SSH identity have access to this shell?).
Two layers, same as a native `alknet/ssh` connection.

## Consequences

**Positive:**
- The browser is a universal alknet client. With WASM parsers for the
  target protocols, a browser can SSH, SFTP, git, and call — all over
  WebTransport, all through the `h3` handler's ALPN-stream-proxy. No
  install, no native client, no VPN.
- The anti-censorship property is real: SSH-over-WebTransport is
  HTTPS-shaped at the network layer. Blocking it requires blocking
  HTTP/3, which breaks the web. This is the "VPN-like without being a
  VPN" use case, now on a clean architectural foundation.
- The `BiStream` trait (ADR-007) pays off. The WASM door it preserved
  is exactly what the browser-side WASM parsers use. The design
  decision to keep `BiStream` a trait (not a concrete quinn type) was
  made for this use case; this ADR is where it's exercised.
- The `h3` handler's stream-routing is path-based (the CONNECT path
  declares the target ALPN), not first-frame-peeking. This is the same
  principle as ALPN dispatch (ADR-001 — the TLS layer routes, no
  byte-peeking) applied to WebTransport sessions.

**Negative:**
- The `HttpAdapter` gains a `HandlerRegistry` reference. This is a
  structural change to the handler's construction (the assembly layer
  passes the registry) and a reference cycle (the handler is registered
  in the registry it holds). The cycle is benign (the endpoint owns
  both; the handler doesn't look itself up), but it's a structural
  property worth noting.
- The ALPN-stream-proxy path is only available over `h3` (WebTransport),
  not `h2`/`http/1.1`. Browsers that don't support WebTransport cannot
  use it. This is inherent — `h2`/`http/1.1` don't have bidirectional
  streams that map to `BiStream`. The SSE projection (ADR-036) is the
  `h2`/`http/1.1` fallback for the call protocol; there is no
  `h2`/`http/1.1` fallback for ALPN-stream-proxy.
- The WASM parsers (SSH, SFTP, git) are downstream artifacts not built
  by `alknet-http`. The server-side proxy path is in scope; the
  browser-side WASM is a separate build per protocol. `russh-sftp`'s
  WASM targeting demonstrates feasibility; SSH is the next target.

## Assumptions

1. **The session's CONNECT path is the routing key.** A browser opens
   a WebTransport session to `/alknet/ssh` to target the SSH handler.
   The path declares the target; the first-frame tag is a confirmation,
   not the routing mechanism. If a future use case requires
   path-independent routing (a session that multiplexes ALPNs by
   first-frame), the model needs extension.

2. **The target ALPN handler accepts a proxied `Connection`.** The
   `SshAdapter` (or `GitAdapter`, `SftpAdapter`) receives a
   `Connection` wrapped from a WebTransport stream and an `AuthContext`
   with the bearer-token-resolved `Identity`. The handler's `handle()`
   method works the same as on a native QUIC connection — the
   `Connection` abstraction (ADR-007) is what makes this work. If a
   handler assumes its `Connection` came from a specific QUIC source
   (quinn vs iroh vs WebTransport-proxied), it breaks the proxy. The
   `Connection` type must remain source-agnostic.

3. **The WASM parsers are feasible for the target protocols.**
   `russh-sftp` demonstrates WASM targeting for SFTP. SSH is the next
   target; the protocol parsing is stream-based and should target
   WASM. Git (gix) is a larger question (git's smart protocol is more
   complex). The assumption is that the protocols worth proxying
   (SSH, SFTP) have WASM-feasible parsers; if a protocol doesn't, its
   ALPN-stream-proxy path is not usable from a browser (but is still
   usable from a non-browser WebTransport client).

4. **The `HandlerRegistry` reference is read-only for the `h3` handler.**
   The `h3` handler looks up ALPN handlers in the registry; it does not
   mutate the registry. The `HandlerRegistry` is static at startup
   (ADR-010, OQ-04), so the `h3` handler's lookup is against an
   immutable registry — no `ArcSwap`, no hot-reload concern.

## References

- [ADR-001](001-alpn-protocol-dispatch.md) — ALPN dispatch (the
  principle the WebTransport path-based routing mirrors: the framing
  layer routes, no byte-peeking)
- [ADR-007](007-bistream-type-definition.md) — `BiStream` trait (the
  contract the browser-side WASM parsers implement over WebTransport
  streams)
- [ADR-010](010-alpn-router-and-endpoint.md) — `HandlerRegistry`
  (the registry the `h3` handler looks up ALPN handlers in; static at
  startup)
- [ADR-012](012-call-protocol-stream-model.md) — stream-agnostic
  correlation (a WebTransport stream is a QUIC bidirectional stream)
- [ADR-027](027-tls-identity-redesign-acme-rawkey-decoupling.md) —
  browsers require X.509 (the `h3` handler is domain-hosted)
- [ADR-034](034-outgoing-only-x509-and-three-peer-roles.md) §4 —
  browsers are not alknet peers (bearer token, no `PeerId`)
- [ADR-038](038-http3-and-webtransport-as-first-class.md) — `h3` is
  first-class (this ADR adds the ALPN-stream-proxy as the third stream
  destination)
- `crates/http/webtransport.md` — the spec that implements this proxy
- `crates/core/endpoint.md` — `HandlerRegistry` (the registry the
  `h3` handler gains a reference to)