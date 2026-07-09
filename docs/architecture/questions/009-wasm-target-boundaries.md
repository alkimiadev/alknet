# OQ-09: WASM Target Boundaries

- **Origin**: [overview.md](overview.md)
- **Status**: deferred
- **Door type**: One-way (when applicable)
- **Priority**: low
- **Blocked on**: A concrete server-side WASM use case, or a deliberate confirmation that WASM stays a client-side design constraint. Tracked as `architecture/oq-09-wasm-server-use-case` in `tasks/architecture/`.
- **Resolution**: Not an active question — WASM compatibility is a design constraint (see ADR-009, overview.md design principles), not a deliverable. Specific WASM targeting decisions will be made when individual crates are implemented. **BiStream being a trait preserves the *client-side* stream door** — a browser can implement BiStream over WebTransport streams. **The *server-side* connection door is now open via `Connection::from_stream` (ADR-065):** `from_stream` accepts any `AsyncRead + AsyncWrite` pair, including wasm-compatible streams, so a `Connection` can be constructed from a wasm stream and dispatched through the `HandlerRegistry` like any QUIC connection. What *remains* closed is the **accept-loop runtime**: the `AlknetEndpoint` accept loops use `tokio::spawn` (tokio does not run on WASM), and the call-protocol dispatch internals (`PendingRequestMap`, `CallAdapter`) use tokio `oneshot`/`mpsc` channels. A WASM server-side peer would require a runtime-abstracted accept loop (not `tokio::spawn`) and a runtime-abstracted channel set — the `Connection` door is open, the runtime door is not. The browser path is client-side via a JS SDK, not server-side Rust-to-WASM. This is an explicit one-way door (the runtime), not an oversight; the `Connection` door was a one-way door that ADR-065 opened (additively, without a trait change).
- **Cross-references**: ADR-007 (Amendment 1 — `from_stream` opens the server-side door), ADR-009, ADR-065

### Amendment (2026-07-09)

The original resolution (above) stated: "**The *server-side* dispatch door
is NOT preserved by ADR-007 and is a known, accepted closure**:
`Connection` is a concrete quinn-bound struct (not a trait), the accept
loop uses `tokio::spawn` (tokio does not run on WASM)..." That was accurate
when written: until ADR-065, `Connection` had only QUIC variants
(`ConnectionKind::Quinn` / `ConnectionKind::Iroh`) plus a
`ConnectionKind::Mock` test stub — no way to construct a `Connection` from a
non-QUIC stream.

**ADR-065 opens the `Connection` door.** `Connection::from_stream` /
`from_bidi` accept any `AsyncRead + AsyncWrite` pair, including
wasm-compatible streams. A `Connection` can now be constructed from a wasm
stream and dispatched through the `HandlerRegistry` like any QUIC
connection — the *connection* door is open. The resolution text above has
been updated to reflect this.

**What is still closed:** the **accept-loop runtime.** The
`AlknetEndpoint` accept loops use `tokio::spawn`, and the call-protocol
dispatch internals (`PendingRequestMap`, `CallAdapter`) use tokio
`oneshot`/`mpsc` channels. Tokio does not run on WASM. A WASM server-side
peer would require a runtime-abstracted accept loop and a
runtime-abstracted channel set — the `Connection` door is open, the
runtime door is not. This is the remaining one-way door, and it is still a
*runtime* door, not a *connection* door. The blocking condition (a concrete
server-side WASM use case) is unchanged.