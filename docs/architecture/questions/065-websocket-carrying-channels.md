# OQ-65: Should WebSocket Carry the Channels Protocol (Not Just the Call Protocol)?

- **Origin**: `docs/architecture/crates/http/websocket.md` (ADR-048
  specifies WebSocket carries the native call-protocol session — the
  `EventEnvelope` wire format over a WebSocket text/binary frame
  stream); `docs/architecture/decisions/086-endpoint-types-and-entry-points.md`
  §3 (the web config advertises `alknet/channels` for the
  WebSocket-carrying-channels path — a path ADR-048 does not specify).
- **Status**: open
- **Door type**: one-way (this supersedes or extends ADR-048's
  "WebSocket carries the native call-protocol session" decision. If
  WebSocket carries channels, a browser opens one WebSocket and gets
  the full channels substrate — `channel/open`, data-channel ALPNs,
  the relay path through the hub — instead of only the call-protocol
  session. This is a browser-wire-format one-way door: once browsers
  depend on the channels-over-WebSocket framing, changing it is a
  breaking change for every browser client.)
- **Priority**: medium (the browser story works without this —
  ADR-048's call-protocol-only WebSocket is functional — but
  channels-over-WebSocket makes the browser a first-class channels
  participant, which simplifies the hub relay and the browser's access
  to data-channel ALPNs like TTY and tunnels. The question is whether
  the simplification is worth the one-way-door commitment now, or
  whether the call-protocol-only path suffices until a concrete
  browser-needs-a-data-channel use case arrives.)
- **Impacts**: Does not block the browser path (ADR-048 works). Would
  simplify the hub relay (one model, not two) and unblock browser
  access to data-channel ALPNs (TTY, tunnels) over a single WebSocket
  if resolved to "WebSocket carries channels."
- **Resolution**: Not yet decided. The two options:

  **Option A — WebSocket carries call only (ADR-048 unchanged).** The
  browser opens a WebSocket and gets a call-protocol session
  (`EventEnvelope` over WebSocket frames). Data-channel ALPNs (TTY,
  tunnels) are not accessible from the browser — the browser can invoke
  call operations but cannot open a `alknet/tty` channel. A browser
  needing a TTY would use a separate WebSocket per session, each
  carrying the TTY wire format directly (not via `channel/open`).
  Simpler; the browser is a call-protocol client, not a channels
  client.

  **Option B — WebSocket carries channels (extends/supersedes
  ADR-048).** The browser opens a WebSocket and gets a channels
  connection — the 9-byte chunk format (ADR-071) over WebSocket binary
  frames, with channel 0 as `alknet/call` and data channels opened via
  `channel/open`. The browser is a full channels participant: it can
  open TTY channels, tunnels, etc. through the same WebSocket. The
  hub relay (ADR-079) works unchanged — the browser leg is a channels
  connection, same as a native leg. This is the "browser as
  channels client" path.

  The trade-off: Option B commits to the channels-over-WebSocket
  framing as a browser wire format (one-way door), but makes the
  browser a first-class channels participant and simplifies the hub
  (one relay model, not two). Option A keeps the browser simpler but
  means the browser cannot use data-channel ALPNs over the same
  connection — each data-channel ALPN needs its own WebSocket and its
  own browser-side implementation.

  This question is decision-ready when the first browser-needs-a-
  data-channel use case arrives (e.g., a browser-based terminal that
  needs `alknet/tty` over the hub). Until then, ADR-048's
  call-protocol-only path is the implemented browser path, and the
  web config advertises `alknet/channels` by default (per ADR-086 §3,
  so the hub is ready for Option B if chosen).

  Note: this is distinct from the WebTransport path (deferred per
  ADR-044). WebTransport, when revived, would carry channels natively
  (a WT bidi stream = a channels connection). WebSocket-carrying-
  channels is the WebSocket equivalent — same substrate, different
  transport. If Option B is chosen, the WebSocket-channels framing
  and the WebTransport-channels framing share the channels wire
  format (ADR-071); only the transport differs.
- **Cross-references**: ADR-048 (WebSocket carries the native
  call-protocol session — the current decision this OQ may supersede
  or extend), ADR-071 (channels wire format — the 9-byte chunk format
  that would ride over WebSocket binary frames), ADR-079 (hub relay —
  unchanged if the browser is a channels client), ADR-086 §3 (the web
  config advertises `alknet/channels` for this path), ADR-044
  (WebTransport deferred; WebSocket is the v1 browser path)