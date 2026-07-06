# OQ-09: WASM Target Boundaries

- **Origin**: [overview.md](overview.md)
- **Status**: deferred
- **Door type**: One-way (when applicable)
- **Priority**: low
- **Blocked on**: A concrete server-side WASM use case, or a deliberate confirmation that WASM stays a client-side design constraint. Tracked as `architecture/oq-09-wasm-server-use-case` in `tasks/architecture/`.
- **Resolution**: Not an active question — WASM compatibility is a design constraint (see ADR-009, overview.md design principles), not a deliverable. Specific WASM targeting decisions will be made when individual crates are implemented. **BiStream being a trait preserves the *client-side* stream door** — a browser can implement BiStream over WebTransport streams. **The *server-side* dispatch door is NOT preserved by ADR-007 and is a known, accepted closure**: `Connection` is a concrete quinn-bound struct (not a trait), the accept loop uses `tokio::spawn` (tokio does not run on WASM), and the call-protocol dispatch internals (`PendingRequestMap`, `CallAdapter`) use tokio `oneshot`/`mpsc` channels. A WASM server-side peer would require a `Connection` trait and a runtime-abstracted accept loop — not planned. The browser path is client-side via a JS SDK, not server-side Rust-to-WASM. This is an explicit one-way door, not an oversight.
- **Cross-references**: ADR-007, ADR-009
