---
id: endpoint/accept-quinn
name: Implement quinn accept loop and extractors in alknet-endpoint
status: completed
depends_on: [endpoint/dispatch]
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Phase 2, Task 5 of the crate extraction. Implement the quinn accept loop and
transport-specific extractors in `crates/alknet-endpoint/src/accept/quinn.rs`.

Extract from `crates/alknet-core/src/endpoint.rs` lines 287-388:
`run_quinn_accept_loop`, `extract_quinn_alpn`, `extract_quinn_client_fingerprint`.

The old code **stays** in `endpoint.rs` (duplicated) — no breakage.

### Types to extract

From `endpoint.rs` lines 287-388:

| Type/Function | Lines | Destination |
|---------------|-------|-------------|
| `run_quinn_accept_loop()` | 288-327 | `accept/quinn.rs` |
| `extract_quinn_alpn()` | 368-378 | `accept/quinn.rs` |
| `extract_quinn_client_fingerprint()` | 381-388 | `accept/quinn.rs` |

### Adaptations

1. **`run_quinn_accept_loop` → `run_accept_loop`**: Rename to `run_accept_loop` since
   it's already in the `quinn` module. Takes `quinn::Endpoint`, `Arc<HandlerRegistry>`,
   `Arc<dyn IdentityProvider>`, `&mut watch::Receiver<bool>`.

2. **Connection conversion**: After the TLS handshake completes, convert the
   `quinn::Connection` to `alknet_core::Connection` via
   `Connection::from_quinn_with_alpn(connection, alpn.clone())` — same as the old code.

3. **Call `dispatch` instead of `dispatch_quinn`**: The accept loop extracts ALPN and
   fingerprint, then calls `AlknetEndpoint::dispatch(connection, alpn, fingerprint, remote_addr)`.
   The old code called `dispatch_quinn(connection, &handlers, &identity_provider)` which
   did the extraction internally. The new code extracts first, then dispatches.

4. **Imports**: Update `crate::` imports to `alknet_core::` and `crate::` (for
   `crate::dispatch::build_auth_context` — but `build_auth_context` is called inside
   `dispatch` now, not in the accept loop).

5. **Feature gates**: All code in this module is gated on `#[cfg(feature = "quinn")]`.

### Implementation sketch

```rust
// accept/quinn.rs
#[cfg(feature = "quinn")]
use std::net::SocketAddr;
#[cfg(feature = "quinn")]
use std::sync::Arc;
#[cfg(feature = "quinn")]
use tokio::sync::watch;
#[cfg(feature = "quinn")]
use tracing::{debug, warn};

#[cfg(feature = "quinn")]
use alknet_core::auth::IdentityProvider;
#[cfg(feature = "quinn")]
use alknet_core::types::Connection;

#[cfg(feature = "quinn")]
use crate::registry::HandlerRegistry;

#[cfg(feature = "quinn")]
pub(crate) async fn run_accept_loop(
    quinn: quinn::Endpoint,
    handlers: Arc<HandlerRegistry>,
    identity_provider: Arc<dyn IdentityProvider>,
    shutdown_rx: &mut watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                debug!("quinn accept loop: shutdown signaled");
                break;
            }
            incoming = quinn.accept() => {
                let Some(incoming) = incoming else {
                    debug!("quinn accept loop: endpoint closed");
                    break;
                };
                let connecting = match incoming.accept() {
                    Ok(c) => c,
                    Err(e) => {
                        warn!("quinn accept failed: {e}");
                        continue;
                    }
                };
                let handlers = handlers.clone();
                let identity_provider = identity_provider.clone();
                tokio::spawn(async move {
                    let connection = match connecting.await {
                        Ok(conn) => conn,
                        Err(e) => {
                            warn!("quinn TLS handshake failure: {e}");
                            return;
                        }
                    };
                    let alpn = extract_alpn(&connection);
                    let remote_addr = Some(connection.remote_address());
                    let fingerprint = extract_client_fingerprint(&connection);
                    let conn = Connection::from_quinn_with_alpn(connection, alpn.clone());
                    // dispatch is called by the endpoint — but the accept loop
                    // doesn't have a reference to the endpoint. Instead, the
                    // endpoint passes a dispatch function or the accept loop
                    // calls a free function.
                    //
                    // Design choice: the accept loop calls
                    // crate::dispatch::dispatch_connection(...) which takes
                    // the extracted values + handlers + identity_provider.
                    // This avoids coupling the accept loop to AlknetEndpoint.
                    crate::dispatch::dispatch_connection(
                        conn, alpn, fingerprint, remote_addr,
                        &handlers, &identity_provider,
                    );
                });
            }
        }
    }
}

#[cfg(feature = "quinn")]
pub(crate) fn extract_alpn(connection: &quinn::Connection) -> Vec<u8> {
    use quinn::crypto::rustls::HandshakeData;
    if let Some(data) = connection.handshake_data() {
        if let Ok(hs) = data.downcast::<HandshakeData>() {
            if let Some(protocol) = hs.protocol {
                return protocol;
            }
        }
    }
    Vec::new()
}

#[cfg(feature = "quinn")]
pub(crate) fn extract_client_fingerprint(connection: &quinn::Connection) -> Option<String> {
    let identity = connection.peer_identity()?;
    let certs = identity
        .downcast::<Vec<rustls::pki_types::CertificateDer>>()
        .ok()?;
    let leaf = certs.first()?;
    alknet_core::fingerprint::fingerprint_from_cert_der(leaf.as_ref())
}
```

### Design note: accept loop → dispatch coupling

The accept loop needs to call `dispatch` but doesn't have a reference to `AlknetEndpoint`.
Two approaches:

**Option A (recommended):** The accept loop calls a free function in `crate::dispatch`
that takes `(Connection, alpn, fingerprint, remote_addr, &HandlerRegistry, &IdentityProvider)`.
This is the same pattern the old code used — `dispatch_quinn` was a free function that
took `&HandlerRegistry` and `&IdentityProvider`. The `AlknetEndpoint::dispatch` method
delegates to this same free function.

**Option B:** The accept loop receives a closure or `Arc<AlknetEndpoint>`. This couples
the accept loop to the endpoint struct, which is unnecessary — the accept loop only needs
the handler registry and identity provider.

Use **Option A** — it's the simplest, matches the old code's pattern, and keeps the
accept loop decoupled from the endpoint struct.

### What stays in core

The old `run_quinn_accept_loop`, `extract_quinn_alpn`, `extract_quinn_client_fingerprint`
in `endpoint.rs` are **not deleted** — they stay as duplicates. The prune happens in Phase 4.

## Acceptance Criteria

- [ ] `accept/quinn.rs` contains `run_accept_loop`, `extract_alpn`, `extract_client_fingerprint`
- [ ] `run_accept_loop` spawns a task per accepted connection, handles shutdown signal
- [ ] `extract_alpn` extracts the negotiated ALPN from quinn handshake data
- [ ] `extract_client_fingerprint` extracts the client cert fingerprint via `alknet_core::fingerprint`
- [ ] Accept loop calls `crate::dispatch::dispatch_connection()` (free function) with extracted values
- [ ] All code gated on `#[cfg(feature = "quinn")]`
- [ ] All imports use `alknet_core::` (not `crate::` from core)
- [ ] `cargo check -p alknet-endpoint --features quinn` succeeds
- [ ] `cargo clippy -p alknet-endpoint --features quinn` succeeds with no warnings
- [ ] `cargo test -p alknet-core` still passes (old code untouched)

## References

- docs/research/alknet-crate-extraction/findings.md — Phase 2, accept/quinn module
- docs/architecture/crates/endpoint/README.md — Accept loops (lines 177-193)
- crates/alknet-core/src/endpoint.rs — lines 287-388 (source code to extract)

## Notes

> This is a straightforward extraction — the quinn accept loop logic doesn't change.
> The main adaptation is calling `crate::dispatch::dispatch_connection()` instead of
> the old `dispatch_quinn()`. The accept loop is a free function, not a method on
> `AlknetEndpoint`, to keep it decoupled. The old code in `endpoint.rs` is NOT deleted.

## Summary

> To be filled on completion
