---
id: endpoint/accept-iroh
name: Implement iroh accept loop and extractors in alknet-endpoint
status: completed
depends_on: [endpoint/dispatch]
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Phase 2, Task 6 of the crate extraction. Implement the iroh accept loop and
transport-specific extractors in `crates/alknet-endpoint/src/accept/iroh.rs`.

Extract from `crates/alknet-core/src/endpoint.rs` lines 390-472:
`run_iroh_accept_loop`, `extract_iroh_client_fingerprint`.

The old code **stays** in `endpoint.rs` (duplicated) — no breakage.

### Types to extract

From `endpoint.rs` lines 390-472:

| Type/Function | Lines | Destination |
|---------------|-------|-------------|
| `run_iroh_accept_loop()` | 391-437 | `accept/iroh.rs` |
| `extract_iroh_client_fingerprint()` | 469-472 | `accept/iroh.rs` |

### Adaptations

1. **`run_iroh_accept_loop` → `run_accept_loop`**: Rename since it's already in the
   `iroh` module. Takes `iroh::Endpoint`, `Arc<HandlerRegistry>`,
   `Arc<dyn IdentityProvider>`, `&mut watch::Receiver<bool>`.

2. **Connection conversion**: After the handshake completes, convert the
   `iroh::endpoint::Connection` to `alknet_core::Connection` via
   `Connection::from_iroh(connection)` — same as the old code.

3. **Call `dispatch` instead of `dispatch_iroh`**: The accept loop extracts ALPN and
   fingerprint, then calls `crate::dispatch::dispatch_connection()` (the free function).
   The old code called `dispatch_iroh(connection, alpn, &handlers, &identity_provider)`.

4. **Imports**: Update `crate::` imports to `alknet_core::` and `crate::`.

5. **Feature gates**: All code in this module is gated on `#[cfg(feature = "iroh")]`.

### Implementation sketch

```rust
// accept/iroh.rs
#[cfg(feature = "iroh")]
use std::sync::Arc;
#[cfg(feature = "iroh")]
use tokio::sync::watch;
#[cfg(feature = "iroh")]
use tracing::{debug, warn};

#[cfg(feature = "iroh")]
use alknet_core::auth::IdentityProvider;
#[cfg(feature = "iroh")]
use alknet_core::types::Connection;

#[cfg(feature = "iroh")]
use crate::registry::HandlerRegistry;

#[cfg(feature = "iroh")]
pub(crate) async fn run_accept_loop(
    iroh: iroh::Endpoint,
    handlers: Arc<HandlerRegistry>,
    identity_provider: Arc<dyn IdentityProvider>,
    shutdown_rx: &mut watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                debug!("iroh accept loop: shutdown signaled");
                break;
            }
            incoming = iroh.accept() => {
                let Some(incoming) = incoming else {
                    debug!("iroh accept loop: endpoint closed");
                    break;
                };
                let handlers = handlers.clone();
                let identity_provider = identity_provider.clone();
                tokio::spawn(async move {
                    let mut connecting = match incoming.accept() {
                        Ok(c) => c,
                        Err(e) => {
                            warn!("iroh accept failed: {e}");
                            return;
                        }
                    };
                    let alpn = match connecting.alpn().await {
                        Ok(alpn) => alpn,
                        Err(e) => {
                            warn!("iroh ALPN negotiation failed: {e}");
                            return;
                        }
                    };
                    let connection = match connecting.await {
                        Ok(conn) => conn,
                        Err(e) => {
                            warn!("iroh handshake completion failed: {e}");
                            return;
                        }
                    };
                    let fingerprint = extract_client_fingerprint(&connection);
                    let conn = Connection::from_iroh(connection);
                    crate::dispatch::dispatch_connection(
                        conn, alpn, fingerprint, None, // iroh has no SocketAddr
                        &handlers, &identity_provider,
                    );
                });
            }
        }
    }
}

#[cfg(feature = "iroh")]
pub(crate) fn extract_client_fingerprint(connection: &iroh::endpoint::Connection) -> Option<String> {
    let node_id = connection.remote_id();
    Some(format!("ed25519:{}", node_id))
}
```

### What stays in core

The old `run_iroh_accept_loop` and `extract_iroh_client_fingerprint` in `endpoint.rs` are
**not deleted** — they stay as duplicates. The prune happens in Phase 4.

## Acceptance Criteria

- [ ] `accept/iroh.rs` contains `run_accept_loop`, `extract_client_fingerprint`
- [ ] `run_accept_loop` spawns a task per accepted connection, handles shutdown signal
- [ ] `run_accept_loop` negotiates ALPN via `connecting.alpn().await`
- [ ] `extract_client_fingerprint` extracts the iroh `NodeId` as `ed25519:<node_id>`
- [ ] Accept loop calls `crate::dispatch::dispatch_connection()` with extracted values
- [ ] `remote_addr` is `None` for iroh (no `SocketAddr`)
- [ ] All code gated on `#[cfg(feature = "iroh")]`
- [ ] All imports use `alknet_core::` (not `crate::` from core)
- [ ] `cargo check -p alknet-endpoint --features iroh` succeeds
- [ ] `cargo clippy -p alknet-endpoint --features iroh` succeeds with no warnings
- [ ] `cargo test -p alknet-core` still passes (old code untouched)

## References

- docs/research/alknet-crate-extraction/findings.md — Phase 2, accept/iroh module
- docs/architecture/crates/endpoint/README.md — Accept loops (lines 177-193)
- crates/alknet-core/src/endpoint.rs — lines 390-472 (source code to extract)

## Notes

> This is a straightforward extraction — the iroh accept loop logic doesn't change.
> The main adaptation is calling `crate::dispatch::dispatch_connection()` instead of
> the old `dispatch_iroh()`. Iroh connections don't have a `SocketAddr`, so
> `remote_addr` is always `None`. The old code in `endpoint.rs` is NOT deleted.

## Summary

> To be filled on completion
